//! Abstract indexing contracts used by the pst-pst-pst indexing stack.
//!
//! This crate intentionally provides only stable interfaces and value types.
//! Concrete storage/search engines are expected to implement these traits.

use pst_pst_pst_core::{
    FilterField, MailboxId, MatchSource, MessageId, FolderId, PageInfo, SearchFilter, SearchMode,
};

/// Configuration for a build operation.
#[derive(Debug, Clone)]
pub struct IndexBuildRequest {
    /// Mailbox that should be (re)indexed.
    pub mailbox_id: MailboxId,
    /// Whether the build must be performed deterministically.
    pub deterministic: bool,
    /// Whether existing index data should be discarded before building.
    pub replace_existing: bool,
    /// Optional source mode requested by the caller.
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

/// Query request used by index readers.
#[derive(Debug, Clone)]
pub struct IndexQuery {
    pub mailbox_id: MailboxId,
    pub text: Option<String>,
    pub filter: SearchFilter,
    pub page: PageInfo,
    pub mode: SearchMode,
    pub include_unindexed: bool,
    pub deterministic: bool,
    pub segment_ids: Vec<String>,
}

/// Immutable segment metadata.
#[derive(Debug, Clone)]
pub struct IndexSegment {
    pub segment_id: String,
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
    pub message_id: MessageId,
    pub folder_id: FolderId,
    pub metadata: IndexMatchMetadata,
}

/// Segment-based query result envelope.
#[derive(Debug, Clone)]
pub struct IndexedSegmentResult {
    pub segment_id: String,
    pub matches: Vec<IndexMatch>,
}

/// Full deterministic query output.
#[derive(Debug, Clone)]
pub struct IndexQueryResult {
    pub mailbox_id: MailboxId,
    pub query: IndexQuery,
    pub matches: Vec<IndexMatch>,
    pub segments: Vec<IndexedSegmentResult>,
    pub total: u64,
    pub returned: usize,
    pub deterministic: bool,
}

/// Read-only index interface contract.
pub trait IndexReader {
    type Error;

    /// Execute a query against the current index snapshot.
    fn query(&self, query: &IndexQuery) -> Result<IndexQueryResult, Self::Error>;

    /// Resolve a segment by identifier.
    fn get_segment(&self, segment_id: &str) -> Result<Option<IndexSegment>, Self::Error>;

    /// List all available segments for a mailbox.
    fn list_segments(&self, mailbox_id: MailboxId) -> Result<Vec<IndexSegment>, Self::Error>;

    /// Return known build state for a mailbox index.
    fn build_state(&self, mailbox_id: MailboxId) -> IndexBuildState;
}

/// Write/maintenance index interface contract.
pub trait IndexWriter {
    type Error;

    /// Request a build for the given mailbox.
    fn request_build(&mut self, request: IndexBuildRequest) -> Result<IndexBuildState, Self::Error>;

    /// Remove an entire segment.
    fn remove_segment(&mut self, segment_id: &str) -> Result<IndexBuildState, Self::Error>;

    /// Replace segment metadata.
    fn replace_segment(&mut self, segment: IndexSegment) -> Result<(), Self::Error>;

    /// Mark a build as completed with the final state.
    fn finish_build(&mut self, state: IndexBuildState) -> Result<(), Self::Error>;
}

/// Combined contract implemented by concrete engines.
pub trait IndexEngine {
    type Error;
    type Reader: IndexReader<Error = Self::Error>;
    type Writer: IndexWriter<Error = Self::Error>;

    /// Access the reader side of the engine.
    fn reader(&self) -> &Self::Reader;

    /// Access the writer side of the engine.
    fn writer(&mut self) -> &mut Self::Writer;
}
