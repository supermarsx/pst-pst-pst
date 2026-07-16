//! Export contracts, progress channels, and resumable checkpoint models.

use std::{
    collections::HashMap,
    fmt,
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pst_pst_pst_core::{AttachmentId, CoreError, FolderId, MailboxId, MessageId};

/// Export error used by this crate.
pub type ExportError = CoreError;
/// Export result alias for the crate.
pub type ExportResult<T> = std::result::Result<T, ExportError>;
/// Manifest result alias.
pub type ExportManifestResult = ExportResult<ExportManifest>;

/// Request options for a concrete export run.
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Source mailbox/container path.
    pub source_path: PathBuf,
    /// Destination path for produced exports.
    pub destination: PathBuf,
    /// Mailbox scope override.
    pub mailbox_id: Option<MailboxId>,
    /// Folder scope if the caller uses scoped export.
    pub folder_ids: Vec<FolderId>,
    /// Message scope for extract-like operations.
    pub message_ids: Vec<MessageId>,
    /// Attachment scope for attachment-only exports.
    pub attachment_ids: Vec<AttachmentId>,
    /// Target format.
    pub format: ExportFormat,
    /// Deterministic naming and scheduler behavior.
    pub deterministic: bool,
    /// Abort on non-fatal failures.
    pub strict: bool,
    /// Optional checkpoint path hint.
    pub checkpoint_path: Option<PathBuf>,
    /// Worker hint for progress/throughput shaping.
    pub workers: usize,
    /// Fallback count when explicit IDs are not available.
    pub max_messages: Option<u64>,
}

impl ExportConfig {
    /// Resolve request count from explicit identifiers or fallback synthetic count.
    pub fn requested_count(&self) -> u64 {
        if self.message_ids.is_empty() {
            self.max_messages.unwrap_or(0)
        } else {
            self.message_ids.len() as u64
        }
    }
}

/// Export serialization format.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ExportFormat {
    /// RFC 5322-like per-message body output.
    Eml,
    /// Unix mbox output.
    Mbox,
    /// Generic JSON lines output.
    Json,
    /// Newline-delimited JSON output.
    Jsonl,
    /// Binary payload output.
    Binary,
}

impl ExportFormat {
    /// String token for this export format.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eml => "eml",
            Self::Mbox => "mbox",
            Self::Json => "json",
            Self::Jsonl => "jsonl",
            Self::Binary => "binary",
        }
    }
}

impl Default for ExportFormat {
    fn default() -> Self {
        Self::Eml
    }
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExportFormat {
    type Err = ExportError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "eml" => Ok(Self::Eml),
            "mbox" => Ok(Self::Mbox),
            "json" => Ok(Self::Json),
            "jsonl" => Ok(Self::Jsonl),
            "binary" => Ok(Self::Binary),
            _ => Err(ExportError::invalid_input(format!(
                "unsupported export format `{value}`"
            ))),
        }
    }
}

/// Progress stages.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExportStage {
    /// Preparing source and scope.
    Prepare,
    /// Writing/exporting items.
    Export,
    /// Finalization/flush.
    Finalize,
}

/// Result for an exported unit.
#[derive(Debug, Clone)]
pub enum ExportItemOutcome {
    Exported {
        bytes: u64,
    },
    Skipped {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

/// Event bus payload for async-like progress sinks.
#[derive(Debug, Clone)]
pub enum ExportProgressEvent {
    Started {
        manifest_id: String,
        source: PathBuf,
        requested: u64,
    },
    Progress {
        manifest_id: String,
        stage: ExportStage,
        processed: u64,
        total: u64,
    },
    Item {
        manifest_id: String,
        index: u64,
        message_id: Option<MessageId>,
        outcome: ExportItemOutcome,
    },
    CheckpointPersisted {
        manifest_id: String,
        checkpoint: ExportCheckpoint,
    },
    Completed {
        manifest_id: String,
        manifest: ExportManifest,
    },
    Failed {
        manifest_id: String,
        message: String,
    },
}

/// Atomic snapshot for multithreaded readers.
#[derive(Debug, Clone, Default)]
pub struct ExportProgressSnapshot {
    pub requested: u64,
    pub processed: u64,
    pub exported: u64,
    pub skipped: u64,
    pub failed: u64,
}

#[derive(Debug, Default)]
struct SharedExportProgress {
    requested: AtomicU64,
    processed: AtomicU64,
    exported: AtomicU64,
    skipped: AtomicU64,
    failed: AtomicU64,
}

impl SharedExportProgress {
    fn snapshot(&self) -> ExportProgressSnapshot {
        ExportProgressSnapshot {
            requested: self.requested.load(Ordering::Relaxed),
            processed: self.processed.load(Ordering::Relaxed),
            exported: self.exported.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }

    fn set_requested(&self, value: u64) {
        self.requested.store(value, Ordering::Relaxed);
    }

    fn inc_processed(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_exported(&self) {
        self.exported.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_skipped(&self) {
        self.skipped.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }
}

/// Thread-safe emitter for progress event consumers.
#[derive(Debug, Clone)]
pub struct ExportProgressHandle {
    sender: Option<SyncSender<ExportProgressEvent>>,
    state: Arc<SharedExportProgress>,
    closed: Arc<AtomicBool>,
}

impl ExportProgressHandle {
    /// Publish one progress event.
    pub fn emit(&self, event: ExportProgressEvent) -> ExportResult<()> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }

        match &event {
            ExportProgressEvent::Started { requested, .. } => {
                self.state.set_requested(*requested);
            }
            ExportProgressEvent::Item { outcome, .. } => {
                self.state.inc_processed();
                match outcome {
                    ExportItemOutcome::Exported { .. } => self.state.inc_exported(),
                    ExportItemOutcome::Skipped { .. } => self.state.inc_skipped(),
                    ExportItemOutcome::Failed { .. } => self.state.inc_failed(),
                }
            }
            ExportProgressEvent::Completed { .. } | ExportProgressEvent::Failed { .. } => {
                self.closed.store(true, Ordering::Release);
            }
            ExportProgressEvent::Progress { .. } | ExportProgressEvent::CheckpointPersisted { .. } => {}
        }

        if let Some(sender) = &self.sender {
            sender
                .send(event)
                .map_err(|_| ExportError::unsupported("progress receiver closed".to_string()))?;
        }

        Ok(())
    }

    /// Read a thread-safe snapshot of all counters.
    pub fn snapshot(&self) -> ExportProgressSnapshot {
        self.state.snapshot()
    }

    /// Manually close the event stream.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

/// Async-like consumer for progress events.
#[derive(Debug)]
pub struct ExportProgressReceiver {
    receiver: mpsc::Receiver<ExportProgressEvent>,
}

impl ExportProgressReceiver {
    /// Receive next progress event.
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<ExportProgressEvent, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

/// Construct a bounded event channel for concurrent producers.
pub fn progress_channel(capacity: usize) -> (ExportProgressHandle, ExportProgressReceiver) {
    let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
    let state = Arc::new(SharedExportProgress::default());
    let closed = Arc::new(AtomicBool::new(false));
    (
        ExportProgressHandle {
            sender: Some(sender),
            state,
            closed,
        },
        ExportProgressReceiver { receiver },
    )
}

/// Checkpoint persistence contract.
pub trait CheckpointStore {
    /// Load one checkpoint.
    fn load(&self, manifest_id: &str) -> ExportResult<Option<ExportCheckpoint>>;
    /// Save one checkpoint.
    fn save(&self, checkpoint: &ExportCheckpoint) -> ExportResult<()>;
    /// Remove checkpoint state.
    fn clear(&self, manifest_id: &str) -> ExportResult<()>;
}

/// In-memory checkpoint store used for wiring and tests.
#[derive(Clone, Default)]
pub struct InMemoryCheckpointStore {
    inner: Arc<Mutex<HashMap<String, ExportCheckpoint>>>,
}

impl CheckpointStore for InMemoryCheckpointStore {
    fn load(&self, manifest_id: &str) -> ExportResult<Option<ExportCheckpoint>> {
        let lock = self
            .inner
            .lock()
            .map_err(|err| ExportError::invalid_input(format!("checkpoint lock poisoned: {err}")))?;
        Ok(lock.get(manifest_id).cloned())
    }

    fn save(&self, checkpoint: &ExportCheckpoint) -> ExportResult<()> {
        let mut lock = self
            .inner
            .lock()
            .map_err(|err| ExportError::invalid_input(format!("checkpoint lock poisoned: {err}")))?;
        lock.insert(checkpoint.manifest_id.clone(), checkpoint.clone());
        Ok(())
    }

    fn clear(&self, manifest_id: &str) -> ExportResult<()> {
        let mut lock = self
            .inner
            .lock()
            .map_err(|err| ExportError::invalid_input(format!("checkpoint lock poisoned: {err}")))?;
        lock.remove(manifest_id);
        Ok(())
    }
}

/// Persistable checkpoint model for resumable exports.
#[derive(Debug, Clone)]
pub struct ExportCheckpoint {
    /// Unique manifest identity.
    pub manifest_id: String,
    /// Source path this checkpoint belongs to.
    pub source_path: PathBuf,
    /// Destination path this checkpoint belongs to.
    pub destination: PathBuf,
    /// Output format in effect.
    pub format: ExportFormat,
    /// Next message index to process.
    pub next_message_index: u64,
    /// Last processed message id.
    pub last_message_id: Option<MessageId>,
    /// Requested item count.
    pub requested: u64,
    /// Counters.
    pub exported: u64,
    /// Counters.
    pub skipped: u64,
    /// Counters.
    pub failed: u64,
    /// Exported bytes.
    pub bytes_exported: u64,
    /// Milliseconds since UNIX epoch.
    pub updated_at_millis: u64,
    /// Completion marker.
    pub completed: bool,
}

impl ExportCheckpoint {
    fn new(config: &ExportConfig, manifest_id: impl Into<String>, requested: u64) -> Self {
        Self {
            manifest_id: manifest_id.into(),
            source_path: config.source_path.clone(),
            destination: config.destination.clone(),
            format: config.format,
            next_message_index: 0,
            last_message_id: None,
            requested,
            exported: 0,
            skipped: 0,
            failed: 0,
            bytes_exported: 0,
            updated_at_millis: now_millis(),
            completed: false,
        }
    }
}

/// Final output summary contract for one export run.
#[derive(Debug, Clone)]
pub struct ExportManifest {
    /// Stable manifest identity.
    pub manifest_id: String,
    /// Source path.
    pub source_path: PathBuf,
    /// Destination path.
    pub destination: PathBuf,
    /// Mailbox scoped by caller.
    pub mailbox_id: Option<MailboxId>,
    /// Export format.
    pub format: ExportFormat,
    /// Requested count.
    pub requested: u64,
    /// Exported count.
    pub exported: u64,
    /// Skipped count.
    pub skipped: u64,
    /// Failed count.
    pub failed: u64,
    /// Deterministic mode.
    pub deterministic: bool,
    /// Checkpoint path reference.
    pub checkpoint_path: Option<PathBuf>,
    /// Last checkpoint after completion or interruption.
    pub final_checkpoint: Option<ExportCheckpoint>,
    /// Completion marker.
    pub completed: bool,
    /// Optional alignment with core contract.
    pub core_summary: Option<pst_pst_pst_core::ExportResult>,
}

/// Engine contract implemented by concrete exporters.
pub trait ExportEngine {
    /// Execute one export run using shared progress/cp semantics.
    fn execute(
        &self,
        config: &ExportConfig,
        resume: Option<ExportCheckpoint>,
        progress: &ExportProgressHandle,
        checkpoint_store: &dyn CheckpointStore,
    ) -> ExportManifestResult;
}

/// Deterministic mock used for CLI wiring and test scaffolding.
#[derive(Debug, Clone)]
pub struct MockExportEngine {
    /// Synthetic work delay between items (ms).
    pub tick_ms: u64,
    /// Synthetic export count used when IDs are not provided.
    pub synthetic_messages: u64,
}

impl Default for MockExportEngine {
    fn default() -> Self {
        Self {
            tick_ms: 20,
            synthetic_messages: 12,
        }
    }
}

impl MockExportEngine {
    fn resolve_total_messages(&self, config: &ExportConfig) -> u64 {
        let requested = config.requested_count();
        if requested > 0 {
            requested
        } else {
            self.synthetic_messages
        }
    }

    fn manifest_id(config: &ExportConfig) -> String {
        let file_name = config
            .source_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown-source");
        format!("pst-pst-pst-export-{file_name}-{}", config.format.as_str())
    }

    fn manifest_from_checkpoint(
        &self,
        config: &ExportConfig,
        checkpoint: &ExportCheckpoint,
        completed: bool,
    ) -> ExportManifest {
        ExportManifest {
            manifest_id: checkpoint.manifest_id.clone(),
            source_path: config.source_path.clone(),
            destination: config.destination.clone(),
            mailbox_id: config.mailbox_id,
            format: config.format,
            requested: checkpoint.requested,
            exported: checkpoint.exported,
            skipped: checkpoint.skipped,
            failed: checkpoint.failed,
            deterministic: config.deterministic,
            checkpoint_path: config.checkpoint_path.clone(),
            final_checkpoint: Some(checkpoint.clone()),
            completed,
                core_summary: Some(pst_pst_pst_core::ExportResult {
                mailbox_id: config.mailbox_id.unwrap_or_else(MailboxId::new),
                requested: checkpoint.requested,
                exported: checkpoint.exported,
                skipped: checkpoint.skipped,
                failed: checkpoint.failed,
                destination: config.destination.clone(),
                manifest_path: config.checkpoint_path.clone(),
                deterministic: config.deterministic,
            }),
        }
    }
}

impl ExportEngine for MockExportEngine {
    fn execute(
        &self,
        config: &ExportConfig,
        mut resume: Option<ExportCheckpoint>,
        progress: &ExportProgressHandle,
        checkpoint_store: &dyn CheckpointStore,
    ) -> ExportManifestResult {
        let manifest_id = Self::manifest_id(config);
        let total = self.resolve_total_messages(config);
        let mut checkpoint = resume
            .take()
            .filter(|value| value.manifest_id == manifest_id)
            .unwrap_or_else(|| ExportCheckpoint::new(config, manifest_id.clone(), total));

        checkpoint.completed = false;
        progress.emit(ExportProgressEvent::Started {
            manifest_id: manifest_id.clone(),
            source: config.source_path.clone(),
            requested: total,
        })?;

        if total == 0 {
            checkpoint.next_message_index = 0;
            checkpoint.updated_at_millis = now_millis();
            checkpoint_store.save(&checkpoint)?;
            let manifest = self.manifest_from_checkpoint(config, &checkpoint, true);
            progress.emit(ExportProgressEvent::Completed {
                manifest_id: manifest_id.clone(),
                manifest: manifest.clone(),
            })?;
            checkpoint_store.clear(&manifest_id)?;
            return Ok(manifest);
        }

        for index in checkpoint.next_message_index..total {
            let message_id = config.message_ids.get(index as usize).copied();
            let outcome = if index % 7 == 0 && index > 0 {
                ExportItemOutcome::Failed {
                    reason: "mock strict failure".to_string(),
                }
            } else if index % 5 == 0 && index > 0 {
                ExportItemOutcome::Skipped {
                    reason: "mock filter skip".to_string(),
                }
            } else {
                ExportItemOutcome::Exported {
                    bytes: 1024 + (index * 16),
                }
            };

            progress.emit(ExportProgressEvent::Item {
                manifest_id: manifest_id.clone(),
                index,
                message_id,
                outcome: outcome.clone(),
            })?;

            match &outcome {
                ExportItemOutcome::Exported { bytes } => {
                    checkpoint.exported += 1;
                    checkpoint.bytes_exported += *bytes;
                }
                ExportItemOutcome::Skipped { .. } => {
                    checkpoint.skipped += 1;
                }
                ExportItemOutcome::Failed { .. } => {
                    checkpoint.failed += 1;
                    if config.strict {
                        checkpoint.next_message_index = index + 1;
                        checkpoint.last_message_id = message_id;
                        checkpoint.updated_at_millis = now_millis();
                        checkpoint.completed = false;
                        checkpoint_store.save(&checkpoint)?;
                        progress.emit(ExportProgressEvent::Failed {
                            manifest_id: manifest_id.clone(),
                            message: format!(
                                "strict failure requested; aborting after index {index}"
                            ),
                        })?;
                        return Err(ExportError::invalid_input(format!(
                            "strict mode abort at index {index}"
                        )));
                    }
                }
            }

            checkpoint.next_message_index = index + 1;
            checkpoint.last_message_id = message_id;
            checkpoint.updated_at_millis = now_millis();
            progress.emit(ExportProgressEvent::Progress {
                manifest_id: manifest_id.clone(),
                stage: ExportStage::Export,
                processed: index + 1,
                total,
            })?;
            checkpoint_store.save(&checkpoint)?;
            progress.emit(ExportProgressEvent::CheckpointPersisted {
                manifest_id: manifest_id.clone(),
                checkpoint: checkpoint.clone(),
            })?;

            if self.tick_ms > 0 {
                thread::sleep(Duration::from_millis(self.tick_ms));
            }
        }

        let final_checkpoint = ExportCheckpoint {
            completed: true,
            next_message_index: total,
            updated_at_millis: now_millis(),
            ..checkpoint
        };

        progress.emit(ExportProgressEvent::Progress {
            manifest_id: manifest_id.clone(),
            stage: ExportStage::Finalize,
            processed: total,
            total,
        })?;
        checkpoint_store.save(&final_checkpoint)?;
        checkpoint_store.clear(&manifest_id)?;

        let manifest = self.manifest_from_checkpoint(config, &final_checkpoint, true);
        progress.emit(ExportProgressEvent::Completed {
            manifest_id,
            manifest: manifest.clone(),
        })?;
        Ok(manifest)
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
