//! Terminal-oriented UI primitives for command dispatch, state, and rendering.
//! This crate provides lightweight contracts and helper implementations with
//! optional rich/plain output.
//!
//! Contract:
//! - Native-Rust only: no mandatory FFI dependency surface and no `unsafe` blocks.
//! - UI runtime and payload types are designed for multithreaded host orchestration.
//! - Supports command-driven search and export workflows without assuming external
//!   platform UI frameworks.
#![forbid(unsafe_code)]

use pst_pst_pst_core::{CommandPayload, CoreError, CoreResult};

/// UI module result alias shared across implementations.
///
/// Error surfaces are shared with core so callers can route UI and export/runtime
/// failures uniformly without custom adapters.
pub type UiResult<T> = CoreResult<T>;
/// UI module error alias.
///
/// Errors are native, structured values from the shared core crate.
pub type UiError = CoreError;

/// Supported interaction modes.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UiMode {
    /// Line-oriented terminal session.
    Terminal,
    /// Embeddable non-interactive host mode.
    Embedded,
}

/// Supported terminal output styles.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UiOutput {
    /// ASCII/plain text output.
    Text,
    /// Minimal ANSI/Unicode rich formatting.
    Rich,
    /// One JSON object per line.
    Jsonl,
    /// Alias kept for CLI symmetry.
    Ndjson,
}

/// Runtime configuration for terminal rendering.
#[derive(Debug, Clone)]
pub struct UiConfig {
    /// Selected runtime interaction mode.
    pub mode: UiMode,
    /// Selected output transport style.
    pub output: UiOutput,
    /// Request deterministic output ordering.
    ///
    /// Use deterministic sessions for reproducible manifest/test expectations.
    pub deterministic: bool,
    /// Max number of command entries to keep in-memory for session continuity.
    pub max_history: usize,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            mode: UiMode::Terminal,
            output: UiOutput::Text,
            deterministic: true,
            max_history: 128,
        }
    }
}

/// Normalized UI command model.
#[derive(Debug, Clone)]
pub struct UiCommand {
    /// Raw source line parsed to produce this command.
    pub raw: String,
    /// Normalized command kind.
    pub kind: UiCommandKind,
    /// Parsed command arguments.
    pub args: Vec<String>,
}

/// Canonical command kinds supported by terminal UI.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UiCommandKind {
    /// Show usage/help.
    Help,
    /// Show environment/session info.
    Info,
    /// Enumerate folders for the active mailbox/session.
    Folders,
    /// Enumerate messages for the active mailbox/session.
    Messages,
    /// Execute a search query.
    Search,
    /// Execute an export request.
    Extract,
    /// Execute an export operation.
    Export,
    /// Validate the active session/context.
    Validate,
    /// Trigger/re-run index operations.
    Index,
    /// Begin/refresh watch notifications.
    Watch,
    /// Exit current UI runtime loop.
    Quit,
    /// Parsed command fallback.
    Unknown(String),
}

impl UiCommand {
    /// Parse a single command line into a normalized command object.
    pub fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        let mut parts = trimmed.split_whitespace();
        let kind = match parts.next().unwrap_or_default().to_ascii_lowercase().as_str() {
            "help" | "h" | "?" => UiCommandKind::Help,
            "info" => UiCommandKind::Info,
            "folders" => UiCommandKind::Folders,
            "messages" => UiCommandKind::Messages,
            "search" => UiCommandKind::Search,
            "extract" => UiCommandKind::Extract,
            "export" => UiCommandKind::Export,
            "validate" => UiCommandKind::Validate,
            "index" => UiCommandKind::Index,
            "watch" => UiCommandKind::Watch,
            "quit" | "exit" | "q" => UiCommandKind::Quit,
            other => UiCommandKind::Unknown(other.to_string()),
        };

        let args = parts.map(ToString::to_string).collect();
        Self {
            raw: trimmed.to_string(),
            kind,
            args,
        }
    }
}

/// Event stream for UI command pipeline.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// Terminal session started.
    Started {
        command_id: u64,
        command: UiCommand,
    },
    /// Command produced a progress message.
    Progress {
        command_id: u64,
        stage: String,
        done: u64,
        total: Option<u64>,
    },
    /// Command output payload is ready.
    Output {
        command_id: u64,
        payload: UiPayload,
    },
    /// Command finished successfully.
    Completed {
        command_id: u64,
        message: Option<String>,
    },
    /// Command encountered an error.
    Failed {
        command_id: u64,
        message: String,
    },
    /// Terminal requested to stop.
    Exit {
        command_id: u64,
        reason: Option<String>,
    },
}

/// Runtime state for terminal UI.
#[derive(Debug, Clone)]
pub struct UiState {
    /// UI config driving rendering and determinism choices.
    pub config: UiConfig,
    /// Monotonic command id counter.
    pub command_counter: u64,
    /// Retained command history for deterministic replay/debug.
    pub command_history: Vec<String>,
    /// Last rendered status for a compact session snapshot.
    pub last_status: Option<String>,
}

impl UiState {
    pub fn new(config: UiConfig) -> Self {
        Self {
            config,
            command_counter: 0,
            command_history: Vec::new(),
            last_status: None,
        }
    }

    pub fn next_command_id(&mut self) -> u64 {
        let id = self.command_counter;
        self.command_counter = self.command_counter.saturating_add(1);
        id
    }

    pub fn push_history(&mut self, command: &UiCommand) {
        self.command_history.push(command.raw.clone());
        if self.command_history.len() > self.config.max_history {
            let trim = self.command_history.len() - self.config.max_history;
            self.command_history.drain(0..trim);
        }
    }

    pub fn apply(&mut self, event: &UiEvent) {
        self.last_status = Some(match event {
            UiEvent::Started { .. } => "running".to_string(),
            UiEvent::Progress { stage, .. } => format!("progress:{stage}"),
            UiEvent::Output { .. } => "output".to_string(),
            UiEvent::Completed { .. } => "completed".to_string(),
            UiEvent::Failed { .. } => "failed".to_string(),
            UiEvent::Exit { .. } => "exit".to_string(),
        });
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self::new(UiConfig::default())
    }
}

/// Command output carrier to keep UI rendering independent from runtime logic.
#[derive(Debug, Clone)]
pub enum UiPayload {
    /// Plain text output payload.
    Text(String),
    /// Shared typed payload from shared core domain types.
    Core(CommandPayload),
}

/// Command execution result shape.
#[derive(Debug, Clone)]
pub struct UiCommandResult {
    /// Command id echoed back for correlation.
    pub command_id: u64,
    /// Request explicit terminal exit.
    pub exit: bool,
    /// Optional completion status.
    pub status: Option<String>,
    /// Optional user-facing payload.
    pub payload: Option<UiPayload>,
}

/// Hook point for concrete command implementations.
///
/// Implementations should avoid FFI and platform-specific UI dependencies so the
/// CLI and host runtime can remain native and deterministic.
pub trait UiCommandBus: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Handle a parsed UI command.
    ///
    /// Implementations may be called from concurrent scheduling layers; all
    /// mutations must therefore be internally synchronized by the host.
    fn execute(
        &mut self,
        state: &UiState,
        command: &UiCommand,
    ) -> Result<UiCommandResult, Self::Error>;
}

/// Hook point for rendering events.
pub trait UiRenderer: Send + Sync {
    /// Render a single event using the given runtime snapshot.
    ///
    /// Implementations should be side-effect free to make concurrent replay
    /// and logging pipelines deterministic.
    fn render_event(&self, event: &UiEvent, state: &UiState) -> String;
}

/// Minimal terminal renderer for text/plain or rich output.
pub struct TerminalRenderer {
    pub output: UiOutput,
}

impl TerminalRenderer {
    pub fn new(output: UiOutput) -> Self {
        Self { output }
    }

    fn render_payload_text(payload: &UiPayload) -> String {
        match payload {
            UiPayload::Text(v) => v.clone(),
            UiPayload::Core(payload) => match payload {
                CommandPayload::Mailbox(v) => format!("Mailbox: {}", v.id),
                CommandPayload::Folders(v) => format!("Folders: {} ({} scanned)", v.folders.len(), v.scanned),
                CommandPayload::Messages(v) => format!(
                    "Messages: {} in {:?} ({} scanned)",
                    v.messages.len(),
                    v.folder_id,
                    v.scanned
                ),
                CommandPayload::Search(v) => format!(
                    "Search hits: {} / {} (mailbox={})",
                    v.hits.len(),
                    v.total,
                    v.mailbox_id
                ),
                CommandPayload::Export(v) => format!(
                    "Export requested={} exported={} skipped={} failed={}",
                    v.requested, v.exported, v.skipped, v.failed
                ),
                CommandPayload::Validation(v) => format!(
                    "Validation passed={} warnings={} errors={}",
                    v.passed, v.warnings, v.errors
                ),
                CommandPayload::Index(v) => format!(
                    "Index mailbox={} db={:?} docs={} segments={} deterministic={} policy={:?} mode={:?}",
                    v.mailbox_id
                        .as_ref()
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string()),
                    v.db_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "<memory>".to_string()),
                    v.documents,
                    v.segments,
                    v.deterministic,
                    v.policy,
                    v.mode
                ),
                CommandPayload::Watch(v) => format!(
                    "Watch dir={} matched={} processed={} failed={}",
                    v.watched_dir.display(),
                    v.matched_files,
                    v.processed_events,
                    v.failed
                ),
                CommandPayload::Ui(v) => format!(
                    "Ui session={} bind={} started={}",
                    v.session_id,
                    v.bind,
                    v.started
                ),
            },
        }
    }
}

impl UiRenderer for TerminalRenderer {
    fn render_event(&self, event: &UiEvent, state: &UiState) -> String {
        let tag = if state.config.deterministic { "det" } else { "nondet" };
        match self.output {
            UiOutput::Text => match event {
                UiEvent::Started { command_id, command } => {
                    format!("[{tag}] {command_id}: {:?}", command.kind)
                }
                UiEvent::Progress {
                    command_id,
                    stage,
                    done,
                    total,
                } => match total {
                    Some(total) => format!(
                        "[{tag}] {command_id}: {stage} {done}/{total}",
                        stage = stage
                    ),
                    None => format!("[{tag}] {command_id}: {stage} {done}"),
                },
                UiEvent::Output {
                    command_id,
                    payload,
                } => format!("[{tag}] {command_id}: {}", Self::render_payload_text(payload)),
                UiEvent::Completed {
                    command_id,
                    message,
                } => format!(
                    "[{tag}] {command_id}: done {}",
                    message.as_deref().unwrap_or("ok")
                ),
                UiEvent::Failed {
                    command_id,
                    message,
                } => format!("[{tag}] {command_id}: failed: {message}"),
                UiEvent::Exit {
                    command_id,
                    reason,
                } => format!(
                    "[{tag}] {command_id}: exit {}",
                    reason.as_deref().unwrap_or("requested")
                ),
            },
            UiOutput::Rich => match event {
                UiEvent::Started { command_id, command } => {
                    format!("▶ [{tag}] #{command_id}: {:?}", command.kind)
                }
                UiEvent::Progress {
                    command_id,
                    stage,
                    done,
                    total,
                } => match total {
                    Some(total) => {
                        let pct = (*done as f64 / *total.max(&1) as f64) * 100.0;
                        format!("◴ [{tag}] #{command_id}: {stage} ({done}/{total}) {pct:.1}%")
                    }
                    None => format!("◴ [{tag}] #{command_id}: {stage} ({done})"),
                },
                UiEvent::Output {
                    command_id,
                    payload,
                } => format!("⟂ [{tag}] #{command_id}: {}", Self::render_payload_text(payload)),
                UiEvent::Completed {
                    command_id,
                    message,
                } => format!(
                    "✔ [{tag}] #{command_id}: {}",
                    message.as_deref().unwrap_or("done")
                ),
                UiEvent::Failed {
                    command_id,
                    message,
                } => format!("✖ [{tag}] #{command_id}: {message}"),
                UiEvent::Exit {
                    command_id,
                    reason,
                } => format!(
                    "◂ [{tag}] #{command_id}: {}",
                    reason.as_deref().unwrap_or("exit")
                ),
            },
            UiOutput::Jsonl | UiOutput::Ndjson => {
                match event {
                    UiEvent::Progress {
                        command_id,
                        stage,
                        done,
                        total,
                    } => {
                        let total = total.map_or_else(|| "null".to_string(), |value| value.to_string());
                        format!(
                            "{{\"command_id\":{command_id},\"type\":\"progress\",\"stage\":\"{}\",\"done\":{done},\"total\":{total}}}",
                            stage.replace('"', "\\\"")
                        )
                    }
                    UiEvent::Output {
                        command_id,
                        payload,
                    } => {
                        format!(
                            "{{\"command_id\":{command_id},\"type\":\"output\",\"payload\":\"{}\"}}",
                            Self::render_payload_text(payload).replace('"', "\\\"")
                        )
                    }
                    _ => format!(
                        "{{\"command_id\":{},\"type\":\"{:?}\"}}",
                        match event {
                            UiEvent::Started { command_id, .. } => *command_id,
                            UiEvent::Progress { command_id, .. } => *command_id,
                            UiEvent::Completed { command_id, .. } => *command_id,
                            UiEvent::Failed { command_id, .. } => *command_id,
                            UiEvent::Exit { command_id, .. } => *command_id,
                        },
                        event_type_name(event)
                    ),
                }
            }
        }
    }
}

fn event_type_name(event: &UiEvent) -> &'static str {
    match event {
        UiEvent::Started { .. } => "started",
        UiEvent::Progress { .. } => "progress",
        UiEvent::Output { .. } => "output",
        UiEvent::Completed { .. } => "completed",
        UiEvent::Failed { .. } => "failed",
        UiEvent::Exit { .. } => "exit",
    }
}

/// Runtime container for terminal dispatch loop.
///
/// Host-facing runtime contract remains thread-aware: stateful execution is confined
/// to owned instances, while individual events and buses can be shared by design.
pub trait UiRuntime: Send + Sync {
    fn state(&self) -> &UiState;
    fn state_mut(&mut self) -> &mut UiState;

    /// Submit one command line and receive rendered lines plus emitted events.
    fn submit(&mut self, input: &str) -> (Vec<UiEvent>, Vec<String>);
}

/// Concise default terminal runtime using injected command bus + renderer.
pub struct TerminalUi<B>
where
    B: UiCommandBus<Error = UiError>,
{
    bus: B,
    renderer: TerminalRenderer,
    state: UiState,
}

impl<B> TerminalUi<B>
where
    B: UiCommandBus<Error = UiError>,
{
    pub fn new(bus: B, config: UiConfig) -> Self {
        let renderer = TerminalRenderer::new(config.output);
        Self {
            bus,
            renderer,
            state: UiState::new(config),
        }
    }

    pub fn with_renderer(bus: B, renderer: TerminalRenderer, config: UiConfig) -> Self {
        Self {
            bus,
            renderer,
            state: UiState::new(config),
        }
    }
}

impl<B> UiRuntime for TerminalUi<B>
where
    B: UiCommandBus<Error = UiError>,
{
    fn state(&self) -> &UiState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut UiState {
        &mut self.state
    }

    fn submit(&mut self, input: &str) -> (Vec<UiEvent>, Vec<String>) {
        let command = UiCommand::parse(input);
        let id = self.state.next_command_id();
        let mut events = Vec::new();
        let mut lines = Vec::new();

        let started = UiEvent::Started {
            command_id: id,
            command: command.clone(),
        };
        self.state.apply(&started);
        self.state.push_history(&command);
        lines.push(self.renderer.render_event(&started, &self.state));
        events.push(started);

        if let UiCommandKind::Unknown(_) = command.kind {
            let failed = UiEvent::Failed {
                command_id: id,
                message: "unknown command".to_string(),
            };
            self.state.apply(&failed);
            lines.push(self.renderer.render_event(&failed, &self.state));
            events.push(failed);
            return (events, lines);
        }

        match self.bus.execute(&self.state, &command) {
            Ok(result) => {
                if let Some(payload) = result.payload {
                    let output = UiEvent::Output {
                        command_id: id,
                        payload,
                    };
                    self.state.apply(&output);
                    lines.push(self.renderer.render_event(&output, &self.state));
                    events.push(output);
                }

                let completed = UiEvent::Completed {
                    command_id: id,
                    message: result.status,
                };
                self.state.apply(&completed);
                lines.push(self.renderer.render_event(&completed, &self.state));
                events.push(completed);

                if command.kind == UiCommandKind::Quit || result.exit {
                    let exit = UiEvent::Exit {
                        command_id: id,
                        reason: Some("quit".to_string()),
                    };
                    self.state.apply(&exit);
                    lines.push(self.renderer.render_event(&exit, &self.state));
                    events.push(exit);
                }
            }
            Err(err) => {
                let failed = UiEvent::Failed {
                    command_id: id,
                    message: err.to_string(),
                };
                self.state.apply(&failed);
                lines.push(self.renderer.render_event(&failed, &self.state));
                events.push(failed);
            }
        }

        (events, lines)
    }
}

/// Optional helper bus for hosts that need a no-op implementation.
pub struct NullCommandBus;

impl UiCommandBus for NullCommandBus {
    type Error = UiError;

    fn execute(&mut self, _state: &UiState, command: &UiCommand) -> Result<UiCommandResult, Self::Error> {
        Ok(UiCommandResult {
            command_id: 0,
            exit: matches!(command.kind, UiCommandKind::Quit),
            status: Some("no backend attached".to_string()),
            payload: Some(UiPayload::Text(format!("received {:?}", command.kind))),
        })
    }
}
