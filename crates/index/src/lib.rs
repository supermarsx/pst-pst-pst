//! In-memory indexed engine used by the CLI-facing indexing contracts.
//!
//! Contract:
//! - Native-Rust only: no mandatory FFI dependency surface and no `unsafe` blocks.
//! - Query/build operations are designed to be orchestrated from parallel runners.
//! - Compatible with native PST/OST/MSG extraction pipelines and deterministic
//!   search/export materialization.
//! - Supports `SearchMode` policy steering (`full`, `indexed`, `hybrid`, `auto`) through
//!   caller-provided query metadata and planner snapshots.
//! - Exposes deterministic metadata for restartable export/search pipelines.
#![forbid(unsafe_code)]

use pst_pst_pst_core::{
    FilterExpr, FilterField, FilterOperator, FilterPredicate, FilterValue, MailboxId, MatchSource,
    MessageId, FolderId, PageInfo, PaginationToken, SearchFilter, SearchMode,
};
use rayon::prelude::*;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt,
};

/// Optional parallelism for build/query stages.
#[derive(Debug, Clone, Copy)]
pub struct IndexConcurrency {
    /// Suggested worker count for preprocessing/upsert pipelines.
    ///
    /// Used by callers to control rayon fan-out width when rebuilding records
    /// before they are committed to the engine.
    pub build_workers: usize,
    /// Suggested worker count for filter/verification passes.
    ///
    /// Used by callers to control query-time matching parallelism.
    pub query_workers: usize,
}

impl Default for IndexConcurrency {
    fn default() -> Self {
        Self {
            build_workers: 1,
            query_workers: 1,
        }
    }
}

/// Operation result type used by this engine.
///
/// The error channel must be shared for read/write contracts so readers and writers
/// can be exercised across host scheduling boundaries without adapter glue.
pub type IndexResult<T> = std::result::Result<T, IndexError>;

/// Configuration for a build operation.
#[derive(Debug, Clone)]
pub struct IndexBuildRequest {
    /// Mailbox that should be (re)indexed.
    pub mailbox_id: MailboxId,
    /// Whether the build must be performed deterministically.
    ///
    /// Deterministic builds should preserve stable ordering so downstream export
    /// manifests remain repeatable across runs.
    pub deterministic: bool,
    /// Whether existing index data should be discarded before building.
    ///
    /// Multi-threaded hosts should set this explicitly to avoid accidental
    /// cross-run accumulation.
    pub replace_existing: bool,
    /// Optional source mode requested by the caller.
    ///
    /// `Some` values guide which search strategy is preferred for rebuild.
    pub source_mode: Option<SearchMode>,
}

/// Progress snapshot exposed while an index build is running.
#[derive(Debug, Clone, Default)]
pub struct IndexBuildProgress {
    /// Number of document batches processed.
    pub batches_processed: u64,
    /// Number of documents staged for indexing.
    pub documents_seen: u64,
    /// Total number of documents expected, if known.
    pub documents_total: Option<u64>,
    /// Segment count completed.
    pub segments_completed: u64,
    /// Total number of segments to build, if known.
    pub segments_total: Option<u64>,
}

/// Current build lifecycle state for a mailbox index.
#[derive(Debug, Clone)]
pub enum IndexBuildState {
    /// No build state is known for the mailbox.
    Unknown,
    /// Build is not yet requested.
    NotRequested,
    /// Build is queued but not started.
    Queued,
    /// Build is running with progress details.
    Running(IndexBuildProgress),
    /// Build completed successfully.
    Completed {
        mailbox_id: MailboxId,
        deterministic: bool,
        total_documents: u64,
        total_segments: u64,
    },
    /// Build ended with an error.
    Failed {
        mailbox_id: MailboxId,
        message: String,
    },
}

impl IndexBuildState {
    fn is_fresh(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }
}

/// Query request used by index readers.
#[derive(Debug, Clone)]
pub struct IndexQuery {
    /// Target mailbox identifier.
    pub mailbox_id: MailboxId,
    /// Optional full-text expression.
    pub text: Option<String>,
    /// Structured search filter expression.
    pub filter: SearchFilter,
    /// Pager snapshot for deterministic/streaming reads.
    pub page: PageInfo,
    /// Search execution mode requested by the caller.
    pub mode: SearchMode,
    /// Include non-indexed/unindexed material when executing hybrid queries.
    pub include_unindexed: bool,
    /// Request stable ordering for export/search reproducibility.
    pub deterministic: bool,
    /// Optional segment allowlist for incremental and resumable reads.
    pub segment_ids: Vec<String>,
}

/// Immutable segment metadata.
#[derive(Debug, Clone)]
pub struct IndexSegment {
    pub segment_id: String,
    /// Mailbox this segment belongs to.
    pub mailbox_id: MailboxId,
    /// Monotonic number so readers can reason over index epochs.
    pub generation: u64,
    pub document_count: u64,
    pub checksum: Option<String>,
    pub deterministic: bool,
}

/// Deterministic metadata for an individual query match.
#[derive(Debug, Clone)]
pub struct IndexMatchMetadata {
    /// How this match was produced.
    pub match_source: MatchSource,
    /// Stable numeric ranking signal, if computed.
    pub score: Option<f64>,
    /// Fields used to determine relevance.
    pub matched_fields: Vec<FilterField>,
    /// Optional snippet, if supported.
    pub snippet: Option<String>,
}

/// A single deterministic match returned by an index query.
#[derive(Debug, Clone)]
pub struct IndexMatch {
    /// Message identity for downstream export pipeline materialization.
    pub message_id: MessageId,
    /// Folder identity for hierarchical session correlation.
    pub folder_id: FolderId,
    /// Transport-agnostic search metadata.
    pub metadata: IndexMatchMetadata,
}

/// Segment-based query result envelope.
#[derive(Debug, Clone)]
pub struct IndexedSegmentResult {
    /// Stable segment key used by checkpoint/export consumers.
    pub segment_id: String,
    /// Matches produced from this segment.
    pub matches: Vec<IndexMatch>,
}

/// Full deterministic query output.
#[derive(Debug, Clone)]
pub struct IndexQueryResult {
    /// Mailbox being queried.
    pub mailbox_id: MailboxId,
    /// Canonicalized request metadata used to generate this response.
    pub query: IndexQuery,
    /// Flattened matches from all segments in deterministic order when requested.
    pub matches: Vec<IndexMatch>,
    /// Segment partitioning for resumable export/search workflows.
    pub segments: Vec<IndexedSegmentResult>,
    /// Total available matches before pagination.
    pub total: u64,
    /// Number of results emitted in this page.
    pub returned: usize,
    /// Whether result ordering is stable under re-run.
    pub deterministic: bool,
}

/// Snapshot of planner decisions for a request.
#[derive(Debug, Clone)]
pub struct FreshQueryPlan {
    /// Mailbox under plan evaluation.
    pub mailbox_id: MailboxId,
    /// Mode requested by caller.
    pub requested_mode: SearchMode,
    /// Mode selected by planner based on index readiness.
    pub effective_mode: SearchMode,
    pub index_fresh: bool,
    pub index_was_used: bool,
    pub include_unindexed: bool,
    pub reason: &'static str,
    /// Segment ids selected by the planner, if any.
    pub selected_segments: Vec<String>,
    /// Optional candidate count estimate for quota/progress accounting.
    pub estimated_candidates: Option<u64>,
}

/// Read-only index interface contract.
pub trait IndexReader: Send + Sync {
    /// Shared error contract for all read operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Execute a query against the current index snapshot.
    ///
    /// Implementations should be free of native/FFI calls and safe to run under
    /// scheduler-driven concurrent readers.
    fn query(&self, query: &IndexQuery) -> Result<IndexQueryResult, Self::Error>;

    /// Resolve a segment by identifier.
    fn get_segment(&self, segment_id: &str) -> Result<Option<IndexSegment>, Self::Error>;

    /// List all available segments for a mailbox.
    fn list_segments(&self, mailbox_id: MailboxId) -> Result<Vec<IndexSegment>, Self::Error>;

    /// Return known build state for a mailbox index.
    fn build_state(&self, mailbox_id: MailboxId) -> IndexBuildState;
}

/// Write/maintenance index interface contract.
pub trait IndexWriter: Send + Sync {
    /// Shared error contract for all write operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Request a build for the given mailbox.
    ///
    /// Implementations should preserve idempotency for equivalent requests.
    fn request_build(&mut self, request: IndexBuildRequest) -> Result<IndexBuildState, Self::Error>;

    /// Remove an entire segment.
    fn remove_segment(&mut self, segment_id: &str) -> Result<IndexBuildState, Self::Error>;

    /// Replace segment metadata.
    fn replace_segment(&mut self, segment: IndexSegment) -> Result<(), Self::Error>;

    /// Mark a build as completed with the final state.
    fn finish_build(&mut self, state: IndexBuildState) -> Result<(), Self::Error>;
}

/// Combined contract implemented by concrete engines.
///
/// Readers and writers share a common, thread-aware error surface so clients can
/// route failures uniformly across CLI, parser, and UI layers.
pub trait IndexEngine: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
    type Reader: IndexReader<Error = Self::Error> + Send + Sync;
    type Writer: IndexWriter<Error = Self::Error> + Send + Sync;

    /// Access the reader side of the engine.
    ///
    /// Readers should be usable from parallel query workers by cloning/owning
    /// reader handles through the host's own synchronization model.
    fn reader(&self) -> &Self::Reader;

    /// Access the writer side of the engine.
    ///
    /// Writer access is intentionally mutable and must be serialized by the caller.
    fn writer(&mut self) -> &mut Self::Writer;
}

/// Upsert payload for indexed message material.
#[derive(Debug, Clone)]
pub struct InMemoryIndexRecord {
    pub mailbox_id: MailboxId,
    pub message_id: MessageId,
    pub folder_id: FolderId,
    pub subject: Option<String>,
    pub sender: Option<String>,
    pub recipients: Vec<String>,
    pub folder_path: Option<String>,
    pub body: Option<String>,
    pub has_attachment: bool,
    pub size: u64,
    pub message_class: Option<String>,
    pub raw_fields: HashMap<String, String>,
    pub segment_id: Option<String>,
}

/// Error surface for the in-memory implementation.
#[derive(Debug)]
pub enum IndexError {
    MailboxMissing(MailboxId),
    SegmentMissing(MailboxId, String),
    MessageMissing(MailboxId, MessageId),
    InvalidInput(String),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MailboxMissing(mailbox_id) => {
                write!(f, "mailbox index missing: {mailbox_id}")
            }
            Self::SegmentMissing(mailbox_id, segment_id) => {
                write!(f, "segment `{segment_id}` not found for mailbox `{mailbox_id}`")
            }
            Self::MessageMissing(mailbox_id, message_id) => {
                write!(f, "message `{message_id}` not found for mailbox `{mailbox_id}`")
            }
            Self::InvalidInput(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for IndexError {}

#[derive(Debug, Clone)]
struct InMemoryIndexedDocument {
    mailbox_id: MailboxId,
    message_id: MessageId,
    folder_id: FolderId,
    subject: Option<String>,
    sender: Option<String>,
    recipients: Vec<String>,
    folder_path: Option<String>,
    body: Option<String>,
    has_attachment: bool,
    size: u64,
    message_class: Option<String>,
    raw_fields: HashMap<String, String>,
    segment_id: String,
    position: u64,
    indexed_tokens: HashSet<String>,
    text_blob: String,
}

#[derive(Debug)]
struct PreparedRecord {
    record: InMemoryIndexRecord,
    indexed_tokens: HashSet<String>,
    text_blob: String,
}

impl InMemoryIndexedDocument {
    fn index_text(&self) -> String {
        let mut value = String::new();
        if let Some(subject) = &self.subject {
            value.push_str(subject);
            value.push(' ');
        }
        if let Some(sender) = &self.sender {
            value.push_str(sender);
            value.push(' ');
        }
        value.push_str(&self.recipients.join(" "));
        if !self.recipients.is_empty() {
            value.push(' ');
        }
        if let Some(folder_path) = &self.folder_path {
            value.push_str(folder_path);
            value.push(' ');
        }
        if let Some(body) = &self.body {
            value.push_str(body);
            value.push(' ');
        }
        if let Some(message_class) = &self.message_class {
            value.push_str(message_class);
            value.push(' ');
        }
        for value in self.raw_fields.values() {
            value.push_str(value);
            value.push(' ');
        }
        value.to_lowercase()
    }

    fn matches_full_text_terms(&self, terms: &[String]) -> bool {
        let haystack = &self.text_blob;
        let mut matched = 0usize;
        for term in terms {
            if haystack.contains(term) {
                matched += 1;
            }
        }
        matched == terms.len()
    }

}

impl InMemoryIndexRecord {
    fn segment_or_default(&self) -> &str {
        self.segment_id.as_deref().unwrap_or("default")
    }
}

#[derive(Debug)]
struct SegmentState {
    meta: IndexSegment,
    document_ids: HashSet<MessageId>,
}

#[derive(Debug)]
struct MailboxIndex {
    mailbox_id: MailboxId,
    documents: HashMap<MessageId, InMemoryIndexedDocument>,
    segments: HashMap<String, SegmentState>,
    postings: HashMap<String, HashSet<MessageId>>,
    build_state: IndexBuildState,
    generation: u64,
    next_position: u64,
}

impl MailboxIndex {
    fn new(mailbox_id: MailboxId) -> Self {
        Self {
            mailbox_id,
            documents: HashMap::new(),
            segments: HashMap::new(),
            postings: HashMap::new(),
            build_state: IndexBuildState::NotRequested,
            generation: 0,
            next_position: 0,
        }
    }
}

#[derive(Debug)]
struct MatchedHit {
    message_id: MessageId,
    folder_id: FolderId,
    source: MatchSource,
    score: Option<f64>,
    matched_fields: Vec<FilterField>,
    snippet: Option<String>,
    segment_id: String,
    position: u64,
}

impl MatchedHit {
    fn as_match(self) -> IndexMatch {
        IndexMatch {
            message_id: self.message_id,
            folder_id: self.folder_id,
            metadata: IndexMatchMetadata {
                match_source: self.source,
                score: self.score,
                matched_fields: self.matched_fields,
                snippet: self.snippet,
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct InMemoryIndexEngine {
    mailboxes: HashMap<MailboxId, MailboxIndex>,
    segment_to_mailbox: HashMap<String, MailboxId>,
}

impl InMemoryIndexEngine {
    fn ensure_mailbox(&mut self, mailbox_id: MailboxId) -> &mut MailboxIndex {
        self.mailboxes
            .entry(mailbox_id)
            .or_insert_with(|| MailboxIndex::new(mailbox_id))
    }

    fn normalize_query_text(text: Option<&str>) -> Vec<String> {
        text.unwrap_or("")
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(ToString::to_string)
            .collect()
    }

    fn ensure_segment(
        &mut self,
        mailbox: &mut MailboxIndex,
        segment_id: &str,
        deterministic: bool,
    ) -> &mut SegmentState {
        if let Some(existing) = mailbox.segments.get(segment_id) {
            return mailbox
                .segments
                .get_mut(segment_id)
                .expect("segment exists after prior check");
        }

        mailbox.generation = mailbox.generation.saturating_add(1);
        let meta = IndexSegment {
            segment_id: segment_id.to_string(),
            mailbox_id: mailbox.mailbox_id,
            generation: mailbox.generation,
            document_count: 0,
            checksum: None,
            deterministic,
        };
        mailbox
            .segments
            .insert(segment_id.to_string(), SegmentState { meta, document_ids: HashSet::new() });
        self.segment_to_mailbox
            .insert(segment_id.to_string(), mailbox.mailbox_id);

        mailbox
            .segments
            .get_mut(segment_id)
            .expect("segment inserted above")
    }

    fn tokenize_for_index(value: &str) -> HashSet<String> {
        value
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(ToString::to_string)
            .collect()
    }

    fn collect_terms(record: &InMemoryIndexRecord) -> HashSet<String> {
        let mut terms = HashSet::new();
        if let Some(subject) = &record.subject {
            terms.extend(Self::tokenize_for_index(subject));
        }
        if let Some(sender) = &record.sender {
            terms.extend(Self::tokenize_for_index(sender));
        }
        for recipient in &record.recipients {
            terms.extend(Self::tokenize_for_index(recipient));
        }
        if let Some(folder_path) = &record.folder_path {
            terms.extend(Self::tokenize_for_index(folder_path));
        }
        if let Some(body) = &record.body {
            terms.extend(Self::tokenize_for_index(body));
        }
        if let Some(message_class) = &record.message_class {
            terms.extend(Self::tokenize_for_index(message_class));
        }
        for raw in record.raw_fields.values() {
            terms.extend(Self::tokenize_for_index(raw));
        }
        terms
    }

    fn rebuild_index_blob_and_tokens(record: &InMemoryIndexRecord) -> (String, HashSet<String>) {
        let doc = InMemoryIndexedDocument {
            mailbox_id: record.mailbox_id,
            message_id: record.message_id,
            folder_id: record.folder_id,
            subject: record.subject.clone(),
            sender: record.sender.clone(),
            recipients: record.recipients.clone(),
            folder_path: record.folder_path.clone(),
            body: record.body.clone(),
            has_attachment: record.has_attachment,
            size: record.size,
            message_class: record.message_class.clone(),
            raw_fields: record.raw_fields.clone(),
            segment_id: record.segment_or_default().to_string(),
            position: 0,
            indexed_tokens: HashSet::new(),
            text_blob: String::new(),
        };
        let text_blob = doc.index_text();
        let tokens = Self::collect_terms(record);
        (text_blob, tokens)
    }

    fn prepare_records(
        records: Vec<InMemoryIndexRecord>,
        workers: usize,
    ) -> IndexResult<Vec<PreparedRecord>> {
        if workers <= 1 {
            return Ok(records
                .into_iter()
                .map(|record| {
                    let (text_blob, indexed_tokens) = Self::rebuild_index_blob_and_tokens(&record);
                    PreparedRecord {
                        record,
                        indexed_tokens,
                        text_blob,
                    }
                })
                .collect());
        }

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers.max(1))
            .build()
            .map_err(|error| IndexError::InvalidInput(format!("failed to create build pool: {error}")))?;
        Ok(pool.install(|| {
            records
                .into_par_iter()
                .map(|record| {
                    let (text_blob, indexed_tokens) = Self::rebuild_index_blob_and_tokens(&record);
                    PreparedRecord {
                        record,
                        indexed_tokens,
                        text_blob,
                    }
                })
                .collect()
        }))
    }

    fn unindex_document(&mut self, mailbox_id: MailboxId, document: &InMemoryIndexedDocument) {
        if let Some(mailbox) = self.mailboxes.get_mut(&mailbox_id) {
            if let Some(segment) = mailbox.segments.get_mut(&document.segment_id) {
                segment.document_ids.remove(&document.message_id);
                segment.meta.document_count = segment.meta.document_count.saturating_sub(1);
            }
            for token in &document.indexed_tokens {
                if let Some(bucket) = mailbox.postings.get_mut(token) {
                    bucket.remove(&document.message_id);
                    if bucket.is_empty() {
                        mailbox.postings.remove(token);
                    }
                }
            }
            mailbox.documents.remove(&document.message_id);
        }
    }

    fn upsert_internal(
        &mut self,
        record: InMemoryIndexRecord,
        require_existing: bool,
    ) -> IndexResult<()> {
        let (text_blob, tokens) = Self::rebuild_index_blob_and_tokens(&record);
        self.upsert_prepared_internal(record, text_blob, tokens, require_existing)
    }

    fn upsert_prepared_internal(
        &mut self,
        record: InMemoryIndexRecord,
        text_blob: String,
        indexed_tokens: HashSet<String>,
        require_existing: bool,
    ) -> IndexResult<()> {
        let segment_id = record.segment_or_default().to_string();
        let mailbox_id = record.mailbox_id;
        let mailbox = self.ensure_mailbox(mailbox_id);
        if require_existing && !mailbox.documents.contains_key(&record.message_id) {
            return Err(IndexError::MessageMissing(mailbox_id, record.message_id));
        }

        if let Some(old) = mailbox.documents.get(&record.message_id).cloned() {
            self.unindex_document(mailbox_id, &old);
        }

        let mut doc = InMemoryIndexedDocument {
            mailbox_id,
            message_id: record.message_id,
            folder_id: record.folder_id,
            subject: record.subject,
            sender: record.sender,
            recipients: record.recipients,
            folder_path: record.folder_path,
            body: record.body,
            has_attachment: record.has_attachment,
            size: record.size,
            message_class: record.message_class,
            raw_fields: record.raw_fields,
            segment_id: segment_id.clone(),
            position: mailbox.next_position,
            indexed_tokens,
            text_blob,
        };

        let deterministic = match mailbox.build_state {
            IndexBuildState::Running(_) => false,
            IndexBuildState::Unknown
            | IndexBuildState::NotRequested
            | IndexBuildState::Queued
            | IndexBuildState::Completed { deterministic, .. } => deterministic,
            IndexBuildState::Failed { .. } => false,
        };
        let segment = self.ensure_segment(mailbox, &segment_id, deterministic);
        segment.document_ids.insert(doc.message_id);
        segment.meta.document_count = segment.meta.document_count.saturating_add(1);

        for term in &doc.indexed_tokens {
            mailbox.postings.entry(term.to_string()).or_default().insert(doc.message_id);
        }

        mailbox.documents.insert(doc.message_id, doc);
        mailbox.next_position = mailbox.next_position.saturating_add(1);

        if let IndexBuildState::Running(ref mut progress) = mailbox.build_state {
            progress.documents_seen = progress.documents_seen.saturating_add(1);
        }

        Ok(())
    }

    pub fn upsert_many(
        &mut self,
        mailbox_id: MailboxId,
        records: Vec<InMemoryIndexRecord>,
        concurrency: Option<IndexConcurrency>,
    ) -> IndexResult<usize> {
        if records.iter().any(|record| record.mailbox_id != mailbox_id) {
            return Err(IndexError::InvalidInput(
                "upsert_many received records for multiple mailboxes".to_string(),
            ));
        }
        let concurrency = concurrency.unwrap_or_default();
        let workers = concurrency.build_workers.max(1);
        let prepared = Self::prepare_records(records, workers)?;
        let count = prepared.len();
        for item in prepared {
            self.upsert_prepared_internal(item.record, item.text_blob, item.indexed_tokens, false)?;
        }
        Ok(count)
    }

    /// Convenience in-memory replacement of a single mailbox message.
    pub fn replace(&mut self, record: InMemoryIndexRecord) -> IndexResult<()> {
        self.upsert_internal(record, true)
    }

    /// Return a canonical request pipeline plan (for UI/debugging visibility).
    pub fn query_plan(&self, query: &IndexQuery) -> FreshQueryPlan {
        self.plan_query(query)
    }

    /// Alias kept for callers expecting a "fresh" planner naming convention.
    pub fn fresh_query_plan(&self, query: &IndexQuery) -> FreshQueryPlan {
        self.plan_query(query)
    }

    fn remove_message_internal(
        &mut self,
        mailbox_id: MailboxId,
        message_id: MessageId,
    ) -> IndexResult<bool> {
        let removed = if let Some(mailbox) = self.mailboxes.get_mut(&mailbox_id) {
            if let Some(old_doc) = mailbox.documents.get(&message_id).cloned() {
                self.unindex_document(mailbox_id, &old_doc);
                true
            } else {
                false
            }
        } else {
            false
        };
        if !removed {
            return Err(IndexError::MessageMissing(mailbox_id, message_id));
        }
        Ok(true)
    }

    fn mailbox_documents_for_segments<'a>(
        &self,
        mailbox: &'a MailboxIndex,
        segment_filter: &[String],
    ) -> Vec<&'a InMemoryIndexedDocument> {
        if segment_filter.is_empty() {
            return mailbox.documents.values().collect();
        }

        let wanted: HashSet<&str> = segment_filter.iter().map(String::as_str).collect();
        mailbox
            .documents
            .values()
            .filter(|doc| wanted.contains(doc.segment_id.as_str()))
            .collect()
    }

    fn candidate_ids_by_index(
        &self,
        mailbox: &MailboxIndex,
        terms: &[String],
        segment_filter: &[String],
    ) -> Option<HashSet<MessageId>> {
        if terms.is_empty() {
            return None;
        }

        let mut result: Option<HashSet<MessageId>> = None;
        for term in terms {
            let current = match mailbox.postings.get(term) {
                Some(current) => current,
                None => return Some(HashSet::new()),
            };
            result = Some(match result {
                Some(mut acc) => {
                    acc.retain(|message_id| current.contains(message_id));
                    acc
                }
                None => current.clone(),
            });
        }

        if let Some(mut candidates) = result {
            if !segment_filter.is_empty() {
                let filter: HashSet<&str> = segment_filter.iter().map(String::as_str).collect();
                candidates.retain(|message_id| mailbox
                    .documents
                    .get(message_id)
                    .is_some_and(|doc| filter.contains(doc.segment_id.as_str())));
            }
            return Some(candidates);
        }
        Some(HashSet::new())
    }

    fn evaluate_filter(expr: &FilterExpr, doc: &InMemoryIndexedDocument) -> bool {
        match expr {
            FilterExpr::True => true,
            FilterExpr::False => false,
            FilterExpr::Not(inner) => !Self::evaluate_filter(inner, doc),
            FilterExpr::And(list) => list.iter().all(|item| Self::evaluate_filter(item, doc)),
            FilterExpr::Or(list) => list.iter().any(|item| Self::evaluate_filter(item, doc)),
            FilterExpr::Predicate(predicate) => {
                Self::evaluate_predicate(predicate, doc)
            }
        }
    }

    fn field_values(field: &FilterField, doc: &InMemoryIndexedDocument) -> Vec<FilterValue> {
        match field {
            FilterField::Subject => doc
                .subject
                .as_ref()
                .map(|value| FilterValue::Text(value.to_lowercase()))
                .into_iter()
                .collect(),
            FilterField::Sender => doc
                .sender
                .as_ref()
                .map(|value| FilterValue::Text(value.to_lowercase()))
                .into_iter()
                .collect(),
            FilterField::Recipient => doc
                .recipients
                .iter()
                .map(|value| FilterValue::Text(value.to_lowercase()))
                .collect(),
            FilterField::Folder => doc
                .folder_path
                .as_ref()
                .map(|value| FilterValue::Text(value.to_lowercase()))
                .into_iter()
                .collect(),
            FilterField::Body => doc
                .body
                .as_ref()
                .map(|value| FilterValue::Text(value.to_lowercase()))
                .into_iter()
                .collect(),
            FilterField::HasAttachment => vec![FilterValue::Bool(doc.has_attachment)],
            FilterField::Size => vec![FilterValue::UInt(doc.size)],
            FilterField::Id => vec![FilterValue::Uuid(doc.message_id.into())],
            FilterField::MessageClass => doc
                .message_class
                .as_ref()
                .map(|value| FilterValue::Text(value.to_lowercase()))
                .into_iter()
                .collect(),
            FilterField::Raw(name) => doc
                .raw_fields
                .get(name)
                .map(|value| FilterValue::Text(value.to_lowercase()))
                .into_iter()
                .collect(),
            FilterField::SentAt | FilterField::ReceivedAt | FilterField::ModifiedAt => {
                Vec::new()
            }
        }
    }

    fn compare_field_value(
        op: FilterOperator,
        field_value: &FilterValue,
        filter_value: &FilterValue,
    ) -> bool {
        match op {
            FilterOperator::Exists => {
                matches!(
                    filter_value,
                    FilterValue::Bool(true) | FilterValue::Null
                ) && !matches!(field_value, FilterValue::Null)
            }
            FilterOperator::Eq => field_value == filter_value,
            FilterOperator::NotEq => field_value != filter_value,
            FilterOperator::Lt => match (field_value, filter_value) {
                (FilterValue::Int(lhs), FilterValue::Int(rhs)) => lhs < rhs,
                (FilterValue::UInt(lhs), FilterValue::UInt(rhs)) => lhs < rhs,
                (FilterValue::Float(lhs), FilterValue::Float(rhs)) => lhs < rhs,
                (FilterValue::DateTime(lhs), FilterValue::DateTime(rhs)) => lhs < rhs,
                (FilterValue::Text(lhs), FilterValue::Text(rhs)) => {
                    lhs.to_lowercase() < rhs.to_lowercase()
                }
                _ => false,
            },
            FilterOperator::Lte => match (field_value, filter_value) {
                (FilterValue::Int(lhs), FilterValue::Int(rhs)) => lhs <= rhs,
                (FilterValue::UInt(lhs), FilterValue::UInt(rhs)) => lhs <= rhs,
                (FilterValue::Float(lhs), FilterValue::Float(rhs)) => lhs <= rhs,
                (FilterValue::DateTime(lhs), FilterValue::DateTime(rhs)) => lhs <= rhs,
                (FilterValue::Text(lhs), FilterValue::Text(rhs)) => {
                    lhs.to_lowercase() <= rhs.to_lowercase()
                }
                _ => false,
            },
            FilterOperator::Gt => match (field_value, filter_value) {
                (FilterValue::Int(lhs), FilterValue::Int(rhs)) => lhs > rhs,
                (FilterValue::UInt(lhs), FilterValue::UInt(rhs)) => lhs > rhs,
                (FilterValue::Float(lhs), FilterValue::Float(rhs)) => lhs > rhs,
                (FilterValue::DateTime(lhs), FilterValue::DateTime(rhs)) => lhs > rhs,
                (FilterValue::Text(lhs), FilterValue::Text(rhs)) => {
                    lhs.to_lowercase() > rhs.to_lowercase()
                }
                _ => false,
            },
            FilterOperator::Gte => match (field_value, filter_value) {
                (FilterValue::Int(lhs), FilterValue::Int(rhs)) => lhs >= rhs,
                (FilterValue::UInt(lhs), FilterValue::UInt(rhs)) => lhs >= rhs,
                (FilterValue::Float(lhs), FilterValue::Float(rhs)) => lhs >= rhs,
                (FilterValue::DateTime(lhs), FilterValue::DateTime(rhs)) => lhs >= rhs,
                (FilterValue::Text(lhs), FilterValue::Text(rhs)) => {
                    lhs.to_lowercase() >= rhs.to_lowercase()
                }
                _ => false,
            },
            FilterOperator::Between => match (field_value, filter_value) {
                (FilterValue::Int(lhs), FilterValue::List(rhs)) => rhs.as_slice().len() == 2
                    && match (&rhs[0], &rhs[1]) {
                        (FilterValue::Int(min), FilterValue::Int(max)) => lhs >= min && lhs <= max,
                        _ => false,
                    },
                (FilterValue::UInt(lhs), FilterValue::List(rhs)) => rhs.as_slice().len() == 2
                    && match (&rhs[0], &rhs[1]) {
                        (FilterValue::UInt(min), FilterValue::UInt(max)) => {
                            lhs >= min && lhs <= max
                        }
                        _ => false,
                    },
                _ => false,
            },
            FilterOperator::Contains => match (field_value, filter_value) {
                (FilterValue::Text(lhs), FilterValue::Text(rhs)) => {
                    lhs.to_lowercase().contains(&rhs.to_lowercase())
                }
                (FilterValue::Uuid(lhs), FilterValue::Text(rhs)) => {
                    lhs.to_string().to_lowercase().contains(&rhs.to_lowercase())
                }
                _ => false,
            },
            FilterOperator::NotContains => match (field_value, filter_value) {
                (FilterValue::Text(lhs), FilterValue::Text(rhs)) => {
                    !lhs.to_lowercase().contains(&rhs.to_lowercase())
                }
                (FilterValue::Uuid(lhs), FilterValue::Text(rhs)) => {
                    !lhs.to_string().to_lowercase().contains(&rhs.to_lowercase())
                }
                _ => false,
            },
            FilterOperator::StartsWith => match (field_value, filter_value) {
                (FilterValue::Text(lhs), FilterValue::Text(rhs)) => {
                    lhs.to_lowercase().starts_with(&rhs.to_lowercase())
                }
                _ => false,
            },
            FilterOperator::EndsWith => match (field_value, filter_value) {
                (FilterValue::Text(lhs), FilterValue::Text(rhs)) => {
                    lhs.to_lowercase().ends_with(&rhs.to_lowercase())
                }
                _ => false,
            },
            FilterOperator::In => match filter_value {
                FilterValue::List(candidates) => candidates
                    .iter()
                    .any(|candidate| Self::compare_field_value(
                        FilterOperator::Eq,
                        field_value,
                        candidate,
                    )),
                _ => false,
            },
        }
    }

    fn evaluate_predicate(predicate: &FilterPredicate, doc: &InMemoryIndexedDocument) -> bool {
        let mut result = false;
        for value in Self::field_values(&predicate.field, doc) {
            if Self::compare_field_value(predicate.op, &value, &predicate.value) {
                result = true;
                break;
            }
        }
        result
    }

    fn collect_filter_fields(expr: &FilterExpr, out: &mut Vec<FilterField>) {
        match expr {
            FilterExpr::True | FilterExpr::False => {}
            FilterExpr::Not(inner) => Self::collect_filter_fields(inner, out),
            FilterExpr::And(list) | FilterExpr::Or(list) => {
                for item in list {
                    Self::collect_filter_fields(item, out);
                }
            }
            FilterExpr::Predicate(predicate) => out.push(predicate.field),
        }
    }

    fn snippet_for_match(doc: &InMemoryIndexedDocument, terms: &[String]) -> Option<String> {
        if terms.is_empty() {
            return None;
        }
        let source = doc
            .body
            .as_deref()
            .or_else(|| doc.subject.as_deref())
            .unwrap_or_default()
            .to_string();
        let lower = source.to_lowercase();
        for term in terms {
            if let Some(pos) = lower.find(term) {
                let start = pos.saturating_sub(48);
                let end = (pos + term.len() + 48).min(source.len());
                return Some(format!("{}...", source[start..end].to_string()));
            }
        }
        Some(source.chars().take(80).collect())
    }

    fn sort_hits(&self, hits: &mut Vec<MatchedHit>, query: &IndexQuery) {
        if query.deterministic {
            hits.sort_by(|left, right| {
                let lhs_score = left.score.unwrap_or(0.0);
                let rhs_score = right.score.unwrap_or(0.0);
                match rhs_score.partial_cmp(&lhs_score).unwrap_or(Ordering::Equal) {
                    Ordering::Equal => {
                        left.message_id.to_string().cmp(&right.message_id.to_string())
                    }
                    other => other,
                }
            });
            return;
        }
        hits.sort_by_key(|hit| hit.position);
    }

    fn build_segments(
        &self,
        mailbox: &MailboxIndex,
        hits: &[MatchedHit],
    ) -> Vec<IndexedSegmentResult> {
        let mut grouped: HashMap<String, Vec<IndexMatch>> = HashMap::new();
        for hit in hits {
            grouped
                .entry(hit.segment_id.clone())
                .or_default()
                .push(hit.clone().as_match());
        }
        let mut segments: Vec<_> = grouped
            .into_iter()
            .map(|(segment_id, matches)| IndexedSegmentResult { segment_id, matches })
            .collect();
        segments.sort_by(|left, right| left.segment_id.cmp(&right.segment_id));
        segments
    }

    fn match_document(
        &self,
        doc: &InMemoryIndexedDocument,
        plan: &FreshQueryPlan,
        query: &IndexQuery,
        normalized_terms: &[String],
    ) -> Option<MatchedHit> {
        if !Self::evaluate_filter(&query.filter.expression, doc) {
            return None;
        }

        let matched_text = if normalized_terms.is_empty() {
            true
        } else if matches!(plan.effective_mode, SearchMode::Indexed) {
            true
        } else {
            doc.matches_full_text_terms(normalized_terms)
        };
        if !matched_text {
            return None;
        }

        let source = match plan.effective_mode {
            SearchMode::Indexed => MatchSource::Indexed,
            SearchMode::Hybrid => MatchSource::Hybrid,
            SearchMode::Full | SearchMode::Auto => MatchSource::Full,
        };

        let mut matched_fields = Vec::new();
        Self::collect_filter_fields(&query.filter.expression, &mut matched_fields);
        if !normalized_terms.is_empty() {
            matched_fields.push(FilterField::Body);
        }
        matched_fields.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
        matched_fields.dedup_by(|left, right| left == right);

        let score = if normalized_terms.is_empty() {
            None
        } else {
            let term_match_ratio = normalized_terms
                .iter()
                .filter(|term| doc.text_blob.contains(*term))
                .count() as f64
                / normalized_terms.len() as f64;
            Some(100.0 * term_match_ratio
                + if doc.has_attachment { 1.0 } else { 0.0 }
                + doc.size as f64 * 1e-9)
        };

        Some(MatchedHit {
            message_id: doc.message_id,
            folder_id: doc.folder_id,
            source,
            score,
            matched_fields,
            snippet: Self::snippet_for_match(doc, normalized_terms),
            segment_id: doc.segment_id.clone(),
            position: doc.position,
        })
    }

    /// Return planned execution mode and freshness for a request.
    pub fn plan_query(&self, query: &IndexQuery) -> FreshQueryPlan {
        let terms = Self::normalize_query_text(query.text.as_deref());
        let mailbox = match self.mailboxes.get(&query.mailbox_id) {
            Some(mailbox) => mailbox,
            None => {
                return FreshQueryPlan {
                    mailbox_id: query.mailbox_id,
                    requested_mode: query.mode,
                    effective_mode: SearchMode::Full,
                    index_fresh: false,
                    index_was_used: false,
                    include_unindexed: query.include_unindexed,
                    reason: "mailbox does not exist",
                    selected_segments: query.segment_ids.clone(),
                    estimated_candidates: Some(0),
                };
            }
        };

        let has_index = !mailbox.postings.is_empty();
        let index_fresh = mailbox.build_state.is_fresh();
        let effective_mode = match query.mode {
            SearchMode::Auto => {
                if !terms.is_empty() && has_index && index_fresh {
                    SearchMode::Indexed
                } else {
                    SearchMode::Full
                }
            }
            SearchMode::Full => SearchMode::Full,
            SearchMode::Indexed => {
                if has_index {
                    SearchMode::Indexed
                } else {
                    SearchMode::Full
                }
            }
            SearchMode::Hybrid => {
                if has_index || query.include_unindexed {
                    SearchMode::Hybrid
                } else {
                    SearchMode::Full
                }
            }
        };
        let index_was_used = matches!(effective_mode, SearchMode::Indexed | SearchMode::Hybrid) && has_index;

        let reason = match effective_mode {
            SearchMode::Indexed => "query planner selected indexed execution",
            SearchMode::Hybrid => "query planner selected hybrid execution",
            SearchMode::Full | SearchMode::Auto => "query planner selected full scan execution",
        };
        let estimated_candidates = if matches!(effective_mode, SearchMode::Indexed | SearchMode::Hybrid) {
            if terms.is_empty() {
                Some(mailbox.documents.len() as u64)
            } else {
                self.candidate_ids_by_index(mailbox, &terms, &query.segment_ids)
                    .map(|ids| ids.len() as u64)
                    .or_else(|| Some(0))
            }
        } else {
            Some(mailbox.documents.len() as u64)
        };

        FreshQueryPlan {
            mailbox_id: query.mailbox_id,
            requested_mode: query.mode,
            effective_mode,
            index_fresh,
            index_was_used,
            include_unindexed: query.include_unindexed,
            reason,
            selected_segments: query.segment_ids.clone(),
            estimated_candidates,
        }
    }

    /// Upsert (insert or replace) an indexed record.
    pub fn upsert(&mut self, record: InMemoryIndexRecord) -> IndexResult<()> {
        self.upsert_internal(record, false)
    }

    /// Remove an indexed message; returns an error if the target mailbox/message pair does not exist.
    pub fn remove(
        &mut self,
        mailbox_id: MailboxId,
        message_id: MessageId,
    ) -> IndexResult<bool> {
        self.remove_message_internal(mailbox_id, message_id)
    }

    fn build_query_matches(
        &self,
        mailbox: &MailboxIndex,
        query: &IndexQuery,
        plan: FreshQueryPlan,
        normalized_terms: &[String],
        query_workers: usize,
    ) -> Vec<MatchedHit> {
        let mut candidate_ids: Option<HashSet<MessageId>> = None;
        if matches!(plan.effective_mode, SearchMode::Indexed | SearchMode::Hybrid) {
            candidate_ids = self.candidate_ids_by_index(mailbox, normalized_terms, &query.segment_ids);
            if matches!(plan.effective_mode, SearchMode::Hybrid) && query.include_unindexed {
                let mut all = HashSet::new();
                for doc in self.mailbox_documents_for_segments(mailbox, &query.segment_ids) {
                    all.insert(doc.message_id);
                }
                if let Some(indexed) = candidate_ids {
                    candidate_ids = Some(all.union(&indexed).copied().collect());
                } else {
                    candidate_ids = Some(all);
                }
            }
        }

        let docs: Vec<&InMemoryIndexedDocument> = match candidate_ids {
            Some(ref ids) => ids
                .iter()
                .filter_map(|message_id| mailbox.documents.get(message_id))
                .collect(),
            None => self.mailbox_documents_for_segments(mailbox, &query.segment_ids),
        };

        let worker_count = query_workers.max(1);
        let use_parallel = worker_count > 1 && docs.len() > 1_024 && query.deterministic == false;
        let mut hits: Vec<MatchedHit> = if use_parallel {
            docs.par_iter()
                .filter_map(|doc| self.match_document(*doc, &plan, query, normalized_terms))
                .collect()
        } else {
            docs.iter()
                .filter_map(|doc| self.match_document(*doc, &plan, query, normalized_terms))
                .collect()
        };

        self.sort_hits(&mut hits, query);
        hits
    }

    /// Create a new in-memory engine with no stored segments.
    pub fn new() -> Self {
        Self::default()
    }
}

impl InMemoryIndexEngine {
    /// Execute a query with an optional search worker override.
    pub fn query_with_workers(
        &self,
        query: &IndexQuery,
        concurrency: Option<IndexConcurrency>,
    ) -> IndexResult<IndexQueryResult> {
        let concurrency = concurrency.unwrap_or_default();

        let plan = self.plan_query(query);
        let mailbox = self
            .mailboxes
            .get(&query.mailbox_id)
            .ok_or(IndexError::MailboxMissing(query.mailbox_id))?;

        let normalized_terms = Self::normalize_query_text(query.text.as_deref());
        let mut hits = self.build_query_matches(
            mailbox,
            query,
            plan,
            &normalized_terms,
            concurrency.query_workers,
        );

        let total = hits.len();
        let limit = if query.page.limit == 0 { usize::MAX } else { query.page.limit as usize };
        let start = if let Some(token) = &query.page.page_token {
            token
                .0
                .parse::<usize>()
                .map_err(|error| IndexError::InvalidInput(format!("invalid page token `{token}`: {error}")))?
        } else {
            query.page.offset as usize
        };
        let end = total.min(start.saturating_add(limit));
        let page_slice = if start >= total {
            &[]
        } else {
            &hits[start..end]
        };
        let matches: Vec<_> = page_slice.iter().cloned().map(MatchedHit::as_match).collect();
        let segments = self.build_segments(mailbox, page_slice);

        let mut page = query.page.clone();
        page.next_page_token = if end < total {
            Some(PaginationToken(end.to_string()))
        } else {
            None
        };
        page.has_more = end < total;

        let mut normalized_query = query.clone();
        normalized_query.page = page;

        Ok(IndexQueryResult {
            mailbox_id: query.mailbox_id,
            query: normalized_query,
            matches,
            segments,
            total: total as u64,
            returned: page_slice.len(),
            deterministic: query.deterministic,
        })
    }
}

impl IndexReader for InMemoryIndexEngine {
    type Error = IndexError;

    fn query(&self, query: &IndexQuery) -> Result<IndexQueryResult, Self::Error> {
        self.query_with_workers(query, None)
    }

    fn get_segment(&self, segment_id: &str) -> Result<Option<IndexSegment>, Self::Error> {
        if let Some(mailbox_id) = self.segment_to_mailbox.get(segment_id) {
            if let Some(mailbox) = self.mailboxes.get(mailbox_id) {
                return Ok(mailbox
                    .segments
                    .get(segment_id)
                    .map(|segment| segment.meta.clone()));
            }
        }
        Ok(None)
    }

    fn list_segments(&self, mailbox_id: MailboxId) -> Result<Vec<IndexSegment>, Self::Error> {
        let mailbox = self
            .mailboxes
            .get(&mailbox_id)
            .ok_or(IndexError::MailboxMissing(mailbox_id))?;
        let mut segments: Vec<_> = mailbox
            .segments
            .values()
            .map(|segment| segment.meta.clone())
            .collect();
        segments.sort_by(|left, right| left.segment_id.cmp(&right.segment_id));
        Ok(segments)
    }

    fn build_state(&self, mailbox_id: MailboxId) -> IndexBuildState {
        self.mailboxes
            .get(&mailbox_id)
            .map(|mailbox| mailbox.build_state.clone())
            .unwrap_or(IndexBuildState::Unknown)
    }
}

impl IndexWriter for InMemoryIndexEngine {
    type Error = IndexError;

    fn request_build(&mut self, request: IndexBuildRequest) -> Result<IndexBuildState, Self::Error> {
        let request_mailbox = self.ensure_mailbox(request.mailbox_id);
        let current_document_count = request_mailbox.documents.len() as u64;
        request_mailbox.build_state = IndexBuildState::Queued;
        let mut documents_total = current_document_count;
        let mut segments_total = request_mailbox.segments.len() as u64;
        if request.replace_existing {
            request_mailbox.documents.clear();
            request_mailbox.postings.clear();
            request_mailbox.next_position = 0;
            let old_segments = request_mailbox.segments.drain().map(|(segment_id, _)| segment_id);
            for segment_id in old_segments {
                self.segment_to_mailbox.remove(&segment_id);
            }
            request_mailbox.build_state = IndexBuildState::NotRequested;
            documents_total = 0;
            segments_total = 0;
        }

        let progress = IndexBuildProgress {
            batches_processed: 0,
            documents_seen: 0,
            documents_total: Some(documents_total),
            segments_completed: 0,
            segments_total: Some(segments_total),
        };
        let state = IndexBuildState::Running(progress);
        request_mailbox.build_state = state.clone();
        let _ = request.source_mode;
        Ok(state)
    }

    fn remove_segment(&mut self, segment_id: &str) -> Result<IndexBuildState, Self::Error> {
        let mailbox_id = self
            .segment_to_mailbox
            .get(segment_id)
            .copied()
            .ok_or_else(|| IndexError::InvalidInput(format!("segment {segment_id} not found")))?;
        let mailbox = self.mailboxes.get_mut(&mailbox_id).ok_or_else(|| {
            IndexError::MailboxMissing(mailbox_id)
        })?;

        let removed = mailbox.segments.remove(segment_id).ok_or_else(|| {
            IndexError::SegmentMissing(mailbox_id, segment_id.to_string())
        })?;
        for message_id in removed.document_ids {
            if let Some(document) = mailbox.documents.remove(&message_id) {
                for token in document.indexed_tokens {
                    if let Some(bucket) = mailbox.postings.get_mut(&token) {
                        bucket.remove(&message_id);
                        if bucket.is_empty() {
                            mailbox.postings.remove(&token);
                        }
                    }
                }
            }
        }
        self.segment_to_mailbox.remove(segment_id);
        mailbox.build_state = IndexBuildState::Queued;
        Ok(mailbox.build_state.clone())
    }

    fn replace_segment(&mut self, segment: IndexSegment) -> Result<(), Self::Error> {
        let mailbox = self
            .mailboxes
            .get_mut(&segment.mailbox_id)
            .ok_or_else(|| IndexError::MailboxMissing(segment.mailbox_id))?;

        let existing = mailbox.segments.entry(segment.segment_id.clone()).or_insert(SegmentState {
            meta: segment.clone(),
            document_ids: HashSet::new(),
        });
        existing.meta = segment;
        self.segment_to_mailbox
            .insert(existing.meta.segment_id.clone(), segment.mailbox_id);
        Ok(())
    }

    fn finish_build(&mut self, state: IndexBuildState) -> Result<(), Self::Error> {
        match &state {
            IndexBuildState::Completed {
                mailbox_id,
                deterministic,
                total_documents,
                total_segments,
            } => {
                let mailbox = self
                    .mailboxes
                    .get_mut(mailbox_id)
                    .ok_or_else(|| IndexError::MailboxMissing(*mailbox_id))?;
                mailbox.build_state = state;
                if let IndexBuildState::Completed { .. } = mailbox.build_state {
                    if *deterministic {
                        let mut ids: Vec<_> = mailbox.segments.keys().cloned().collect();
                        ids.sort_unstable();
                        for segment_id in ids {
                            if let Some(segment) = mailbox.segments.get_mut(&segment_id) {
                                segment.meta.deterministic = true;
                                segment.meta.document_count = segment
                                    .document_ids
                                    .iter()
                                    .filter(|id| mailbox.documents.contains_key(id))
                                    .count() as u64;
                            }
                        }
                    }
                    mailbox.build_state = IndexBuildState::Completed {
                        mailbox_id: *mailbox_id,
                        deterministic: *deterministic,
                        total_documents: *total_documents,
                        total_segments: *total_segments,
                    };
                }
            }
            IndexBuildState::Failed { mailbox_id, message } => {
                let mailbox = self
                    .mailboxes
                    .get_mut(mailbox_id)
                    .ok_or_else(|| IndexError::MailboxMissing(*mailbox_id))?;
                mailbox.build_state = IndexBuildState::Failed {
                    mailbox_id: *mailbox_id,
                    message: message.clone(),
                };
            }
            _ => {}
        }
        Ok(())
    }
}

impl IndexEngine for InMemoryIndexEngine {
    type Error = IndexError;
    type Reader = Self;
    type Writer = Self;

    fn reader(&self) -> &Self::Reader {
        self
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self
    }
}
