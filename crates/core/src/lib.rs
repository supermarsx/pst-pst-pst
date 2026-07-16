//! Shared domain primitives for the `pst-pst-pst` workspace.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use std::{fmt, path::{Path, PathBuf}, str::FromStr};

pub type CoreResult<T> = std::result::Result<T, CoreError>;

macro_rules! define_id_type {
    ($name:ident) => {
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        #[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
        pub struct $name(Uuid);

        impl $name {
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub const fn into_uuid(self) -> Uuid {
                self.0
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self::from_uuid)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

define_id_type!(MailboxId);
define_id_type!(FolderId);
define_id_type!(MessageId);
define_id_type!(AttachmentId);
define_id_type!(ParseEventId);

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ErrorClass {
    Io,
    Parse,
    Decode,
    Integrity,
    Index,
    Export,
    Ui,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum Severity {
    Warn,
    Error,
    Fatal,
}

impl Severity {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Fatal)
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error for `{path:?}`: {message}")]
    Io {
        path: Option<PathBuf>,
        message: String,
        details: Option<String>,
    },
    #[error("parse error at `{location:?}`: {message}")]
    Parse {
        location: Option<ParsedItemId>,
        message: String,
        details: Option<String>,
    },
    #[error("decode error at `{location:?}`: {message}")]
    Decode {
        location: Option<ParsedItemId>,
        message: String,
        details: Option<String>,
    },
    #[error("integrity error: {message}")]
    Integrity {
        location: Option<ParsedItemId>,
        message: String,
        details: Option<String>,
    },
    #[error("indexing error: {message}")]
    Index {
        dataset: Option<String>,
        message: String,
        details: Option<String>,
    },
    #[error("export error for `{destination:?}`: {message}")]
    Export {
        destination: Option<PathBuf>,
        message: String,
        details: Option<String>,
    },
    #[error("ui error: {message}")]
    Ui {
        message: String,
        details: Option<String>,
    },
    #[error("unsupported operation: {message}")]
    Unsupported { message: String },
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
}

impl CoreError {
    pub fn io(path: Option<PathBuf>, message: impl Into<String>) -> Self {
        Self::Io {
            path,
            message: message.into(),
            details: None,
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::Parse {
            location: None,
            message: message.into(),
            details: None,
        }
    }

    pub fn decode(message: impl Into<String>) -> Self {
        Self::Decode {
            location: None,
            message: message.into(),
            details: None,
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ContainerFormat {
    Pst,
    Ost,
    Msg,
    Unknown,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum MailboxState {
    Healthy,
    Degraded,
    Corrupt,
    Unknown,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub id: MailboxId,
    pub source_path: PathBuf,
    pub container_format: ContainerFormat,
    pub state: MailboxState,
    pub folder_count: u64,
    pub message_count: u64,
    pub attachment_count: u64,
    pub recovered_count: u64,
    pub parse_error_count: u64,
    pub is_read_only: bool,
    pub opened_at: Option<DateTime<Utc>>,
    pub diagnostics: Vec<ParseEvent>,
}

impl Mailbox {
    pub fn new(source_path: PathBuf, container_format: ContainerFormat) -> Self {
        Self {
            id: MailboxId::new(),
            source_path,
            container_format,
            state: MailboxState::Unknown,
            folder_count: 0,
            message_count: 0,
            attachment_count: 0,
            recovered_count: 0,
            parse_error_count: 0,
            is_read_only: true,
            opened_at: Some(Utc::now()),
            diagnostics: Vec::new(),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: FolderId,
    pub mailbox_id: MailboxId,
    pub parent_id: Option<FolderId>,
    pub name: String,
    pub path: String,
    pub message_count: u64,
    pub unread_count: u64,
    pub total_size: u64,
    pub has_subfolders: bool,
    pub is_hidden: bool,
    pub is_root: bool,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct MessageFlags {
    bits: u16,
}

impl MessageFlags {
    pub const READ: u16 = 0b0000_0000_0000_0001;
    pub const FLAGGED: u16 = 0b0000_0000_0000_0010;
    pub const HAS_ATTACHMENTS: u16 = 0b0000_0000_0000_0100;
    pub const IMPORTANT: u16 = 0b0000_0000_0000_1000;
    pub const DELETED: u16 = 0b0000_0000_0001_0000;

    pub const fn new(bits: u16) -> Self {
        Self { bits }
    }

    pub const fn none() -> Self {
        Self { bits: 0 }
    }

    pub const fn is_set(&self, mask: u16) -> bool {
        self.bits & mask == mask
    }

    pub const fn with(mut self, mask: u16) -> Self {
        self.bits |= mask;
        self
    }

    pub const fn without(mut self, mask: u16) -> Self {
        self.bits &= !mask;
        self
    }

    pub const fn bits(&self) -> u16 {
        self.bits
    }
}

impl Default for MessageFlags {
    fn default() -> Self {
        Self::none()
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum RecipientRole {
    Sender,
    From,
    To,
    Cc,
    Bcc,
    ReplyTo,
    Other(String),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub role: RecipientRole,
    pub display_name: Option<String>,
    pub address: Option<String>,
    pub routing_type: Option<String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BodyFormat {
    PlainText,
    Html,
    Rtf,
    Calendar,
    Unknown(String),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBodyRef {
    pub format: BodyFormat,
    pub byte_size: u64,
    pub content_ref: Option<String>,
    pub is_truncated: bool,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageProperty {
    pub name: String,
    pub value: PropertyValue,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    DateTime(DateTime<Utc>),
    Uuid(Uuid),
    Raw(Vec<u8>),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: AttachmentId,
    pub message_id: MessageId,
    pub filename: Option<String>,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub byte_size: u64,
    pub sha256: Option<String>,
    pub inline: bool,
    pub content_id: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub folder_id: FolderId,
    pub subject: Option<String>,
    pub internet_message_id: Option<String>,
    pub conversation_id: Option<String>,
    pub message_class: Option<String>,
    pub sender: Option<Recipient>,
    pub recipients: Vec<Recipient>,
    pub sent_at: Option<DateTime<Utc>>,
    pub received_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub size: u64,
    pub flags: MessageFlags,
    pub has_attachments: bool,
    pub body: Option<MessageBodyRef>,
    pub attachments: Vec<Attachment>,
    pub properties: Vec<MessageProperty>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ParseStage {
    Discovery,
    Folder,
    Message,
    Recipient,
    Attachment,
    Body,
    Index,
    Export,
    Validate,
    Other(String),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ParsedItemId {
    Mailbox(MailboxId),
    Folder(FolderId),
    Message(MessageId),
    Attachment(AttachmentId),
    ParseEvent(ParseEventId),
    Unknown(String),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct ParseEvent {
    pub id: ParseEventId,
    pub location: Option<ParsedItemId>,
    pub stage: ParseStage,
    pub class: ErrorClass,
    pub severity: Severity,
    pub message: String,
    pub details: Option<String>,
    pub occurs_at: DateTime<Utc>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum SearchMode {
    Auto,
    Full,
    Indexed,
    Hybrid,
}

impl SearchMode {
    pub fn is_textual(self) -> bool {
        matches!(self, Self::Full | Self::Hybrid)
    }
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Auto
    }
}

impl fmt::Display for SearchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Full => f.write_str("full"),
            Self::Indexed => f.write_str("indexed"),
            Self::Hybrid => f.write_str("hybrid"),
        }
    }
}

impl FromStr for SearchMode {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "full" => Ok(Self::Full),
            "indexed" => Ok(Self::Indexed),
            "hybrid" => Ok(Self::Hybrid),
            _ => Err(CoreError::invalid_input(format!("unsupported search mode: {value}"))),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum IndexPolicy {
    Allow,
    Require,
    Refresh,
    Build,
}

impl Default for IndexPolicy {
    fn default() -> Self {
        Self::Allow
    }
}

impl fmt::Display for IndexPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => f.write_str("allow"),
            Self::Require => f.write_str("require"),
            Self::Refresh => f.write_str("refresh"),
            Self::Build => f.write_str("build"),
        }
    }
}

impl FromStr for IndexPolicy {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "allow" => Ok(Self::Allow),
            "require" => Ok(Self::Require),
            "refresh" => Ok(Self::Refresh),
            "build" => Ok(Self::Build),
            _ => Err(CoreError::invalid_input(format!(
                "unsupported index policy: {value}"
            ))),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ExportFormat {
    Eml,
    Mbox,
    Json,
    Jsonl,
    Msg,
}

impl Default for ExportFormat {
    fn default() -> Self {
        Self::Jsonl
    }
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eml => f.write_str("eml"),
            Self::Mbox => f.write_str("mbox"),
            Self::Json => f.write_str("json"),
            Self::Jsonl => f.write_str("jsonl"),
            Self::Msg => f.write_str("msg"),
        }
    }
}

impl FromStr for ExportFormat {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "eml" => Ok(Self::Eml),
            "mbox" => Ok(Self::Mbox),
            "json" => Ok(Self::Json),
            "jsonl" => Ok(Self::Jsonl),
            "msg" => Ok(Self::Msg),
            _ => Err(CoreError::invalid_input(format!(
                "unsupported export format: {value}"
            ))),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum OutputFormat {
    Table,
    Json,
    Jsonl,
    Ndjson,
}

impl OutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Json => "json",
            Self::Jsonl => "jsonl",
            Self::Ndjson => "ndjson",
        }
    }
}

impl FromStr for OutputFormat {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "jsonl" => Ok(Self::Jsonl),
            "ndjson" => Ok(Self::Ndjson),
            _ => Err(CoreError::invalid_input(format!("unsupported output format: {value}"))),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum FilterField {
    Subject,
    Sender,
    Recipient,
    Folder,
    Body,
    HasAttachment,
    Size,
    Id,
    SentAt,
    ReceivedAt,
    ModifiedAt,
    MessageClass,
    Raw(String),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum FilterOperator {
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
    Between,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    In,
    Exists,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum FilterValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    DateTime(DateTime<Utc>),
    Uuid(Uuid),
    Bytes(Vec<u8>),
    Path(PathBuf),
    List(Vec<FilterValue>),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FilterPredicate {
    pub field: FilterField,
    pub op: FilterOperator,
    pub value: FilterValue,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpr {
    True,
    False,
    Predicate(FilterPredicate),
    Not(Box<FilterExpr>),
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
}

impl FilterExpr {
    pub fn and(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::True, rhs) => rhs,
            (lhs, Self::True) => lhs,
            (Self::False, _) => Self::False,
            (_, Self::False) => Self::False,
            (lhs, rhs) => Self::And(vec![lhs, rhs]),
        }
    }

    pub fn or(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::False, rhs) => rhs,
            (lhs, Self::False) => lhs,
            (Self::True, _) => Self::True,
            (_, Self::True) => Self::True,
            (lhs, rhs) => Self::Or(vec![lhs, rhs]),
        }
    }

    pub fn not(expr: Self) -> Self {
        Self::Not(Box::new(expr))
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct SearchFilter {
    pub expression: FilterExpr,
    pub source: Option<String>,
}

impl Default for SearchFilter {
    fn default() -> Self {
        Self {
            expression: FilterExpr::True,
            source: None,
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum MatchSource {
    Full,
    Indexed,
    Hybrid,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct PaginationToken(pub String);

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct PageInfo {
    pub limit: u64,
    pub offset: u64,
    pub has_more: bool,
    pub page_token: Option<PaginationToken>,
    pub next_page_token: Option<PaginationToken>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub message_id: MessageId,
    pub folder_id: FolderId,
    pub score: Option<f64>,
    pub match_source: MatchSource,
    pub matched_fields: Vec<FilterField>,
    pub snippet: Option<String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub mailbox_id: MailboxId,
    pub hits: Vec<SearchHit>,
    pub total: u64,
    pub returned: usize,
    pub query: Option<String>,
    pub source_mode: SearchMode,
    pub include_unindexed: bool,
    pub deterministic: bool,
    pub page: PageInfo,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct FolderListResult {
    pub mailbox_id: MailboxId,
    pub folders: Vec<Folder>,
    pub scanned: u64,
    pub page: PageInfo,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct MessageListResult {
    pub mailbox_id: MailboxId,
    pub folder_id: Option<FolderId>,
    pub messages: Vec<Message>,
    pub scanned: u64,
    pub page: PageInfo,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct ExportResult {
    pub requested: u64,
    pub exported: u64,
    pub skipped: u64,
    pub failed: u64,
    pub destination: PathBuf,
    pub manifest_path: Option<PathBuf>,
    pub deterministic: bool,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub mailbox_id: MailboxId,
    pub passed: bool,
    pub scanned_items: u64,
    pub warnings: u64,
    pub errors: u64,
    pub events: Vec<ParseEvent>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub output: OutputFormat,
    pub payload: CommandPayload,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub enum CommandPayload {
    Mailbox(Mailbox),
    Folders(FolderListResult),
    Messages(MessageListResult),
    Search(SearchResult),
    Export(ExportResult),
    Validation(ValidationResult),
    Index(IndexResult),
    Watch(WatchResult),
    Ui(UiResult),
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct RuntimeExecutionConfig {
    pub jobs: usize,
    pub io_jobs: usize,
    pub cpu_jobs: usize,
    pub single_thread: bool,
    pub strict: bool,
    pub include_unindexed: bool,
    pub index_staleness_threshold: Option<u64>,
}

impl Default for RuntimeExecutionConfig {
    fn default() -> Self {
        Self {
            jobs: 4,
            io_jobs: 4,
            cpu_jobs: 4,
            single_thread: false,
            strict: false,
            include_unindexed: false,
            index_staleness_threshold: None,
        }
    }
}

impl RuntimeExecutionConfig {
    pub fn validate(&self) -> CoreResult<()> {
        if self.jobs == 0 {
            return Err(CoreError::invalid_input("jobs must be greater than zero"));
        }
        if self.io_jobs == 0 {
            return Err(CoreError::invalid_input(
                "io_jobs must be greater than zero",
            ));
        }
        if self.cpu_jobs == 0 {
            return Err(CoreError::invalid_input(
                "cpu_jobs must be greater than zero",
            ));
        }
        Ok(())
    }

    pub fn normalize(&self) -> Self {
        if self.single_thread {
            return Self {
                jobs: 1,
                io_jobs: 1,
                cpu_jobs: 1,
                single_thread: true,
                strict: self.strict,
                include_unindexed: self.include_unindexed,
                index_staleness_threshold: self.index_staleness_threshold,
            };
        }
        Self {
            jobs: self.jobs.max(1),
            io_jobs: self.io_jobs.max(1),
            cpu_jobs: self.cpu_jobs.max(1),
            single_thread: false,
            strict: self.strict,
            include_unindexed: self.include_unindexed,
            index_staleness_threshold: self.index_staleness_threshold,
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct SharedCommandOptions {
    pub filter: Vec<String>,
    pub output: OutputFormat,
    pub limit: Option<u64>,
    pub sort: Option<String>,
    pub deterministic: bool,
    pub strict: bool,
    pub page_token: Option<String>,
}

impl Default for SharedCommandOptions {
    fn default() -> Self {
        Self {
            filter: Vec::new(),
            output: OutputFormat::Table,
            limit: None,
            sort: None,
            deterministic: false,
            strict: false,
            page_token: None,
        }
    }
}

impl SharedCommandOptions {
    pub fn validate(&self) -> CoreResult<()> {
        if let Some(limit) = self.limit {
            if limit == 0 {
                return Err(CoreError::invalid_input("limit must be greater than zero"));
            }
        }
        if let Some(sort) = &self.sort {
            if sort.trim().is_empty() {
                return Err(CoreError::invalid_input("sort expression must not be empty"));
            }
        }
        Ok(())
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct InfoCommand {
    pub source: PathBuf,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct FoldersCommand {
    pub source: PathBuf,
    pub folder: Option<String>,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct MessagesCommand {
    pub source: PathBuf,
    pub folder: Option<String>,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct SearchCommand {
    pub source: PathBuf,
    pub query: String,
    pub fields: Vec<String>,
    pub mode: SearchMode,
    pub index_policy: IndexPolicy,
    pub include_unindexed: bool,
    pub max_results: Option<u64>,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct ExtractCommand {
    pub source: PathBuf,
    pub message_id: Option<String>,
    pub attachment_id: Option<String>,
    pub out: Option<PathBuf>,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct ExportCommand {
    pub source: PathBuf,
    pub format: ExportFormat,
    pub out: Option<PathBuf>,
    pub folder: Option<String>,
    pub message_ids: Vec<String>,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct ValidateCommand {
    pub source: PathBuf,
    pub report: Option<PathBuf>,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct IndexCommand {
    pub source: PathBuf,
    pub db: Option<PathBuf>,
    pub rebuild: bool,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct WatchCommand {
    pub dir: PathBuf,
    pub pattern: Option<String>,
    pub on_changed: String,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct UiCommand {
    pub bind: String,
    pub options: SharedCommandOptions,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum CommandKind {
    Info,
    Folders,
    Messages,
    Search,
    Extract,
    Export,
    Validate,
    Index,
    Watch,
    Ui,
}

impl fmt::Display for CommandKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info => f.write_str("info"),
            Self::Folders => f.write_str("folders"),
            Self::Messages => f.write_str("messages"),
            Self::Search => f.write_str("search"),
            Self::Extract => f.write_str("extract"),
            Self::Export => f.write_str("export"),
            Self::Validate => f.write_str("validate"),
            Self::Index => f.write_str("index"),
            Self::Watch => f.write_str("watch"),
            Self::Ui => f.write_str("ui"),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub enum Command {
    Info(InfoCommand),
    Folders(FoldersCommand),
    Messages(MessagesCommand),
    Search(SearchCommand),
    Extract(ExtractCommand),
    Export(ExportCommand),
    Validate(ValidateCommand),
    Index(IndexCommand),
    Watch(WatchCommand),
    Ui(UiCommand),
}

impl Command {
    pub fn kind(&self) -> CommandKind {
        match self {
            Self::Info(_) => CommandKind::Info,
            Self::Folders(_) => CommandKind::Folders,
            Self::Messages(_) => CommandKind::Messages,
            Self::Search(_) => CommandKind::Search,
            Self::Extract(_) => CommandKind::Extract,
            Self::Export(_) => CommandKind::Export,
            Self::Validate(_) => CommandKind::Validate,
            Self::Index(_) => CommandKind::Index,
            Self::Watch(_) => CommandKind::Watch,
            Self::Ui(_) => CommandKind::Ui,
        }
    }

    pub fn shared_options(&self) -> &SharedCommandOptions {
        match self {
            Self::Info(c) => &c.options,
            Self::Folders(c) => &c.options,
            Self::Messages(c) => &c.options,
            Self::Search(c) => &c.options,
            Self::Extract(c) => &c.options,
            Self::Export(c) => &c.options,
            Self::Validate(c) => &c.options,
            Self::Index(c) => &c.options,
            Self::Watch(c) => &c.options,
            Self::Ui(c) => &c.options,
        }
    }

    pub fn validate(&self) -> CoreResult<()> {
        self.shared_options().validate()?;
        match self {
            Self::Info(command) => validate_source_path(&command.source),
            Self::Folders(command) => {
                validate_source_path(&command.source)?;
                if let Some(folder) = &command.folder {
                    if folder.trim().is_empty() {
                        return Err(CoreError::invalid_input("folder must not be empty"));
                    }
                }
                Ok(())
            }
            Self::Messages(command) => {
                validate_source_path(&command.source)?;
                if let Some(folder) = &command.folder {
                    if folder.trim().is_empty() {
                        return Err(CoreError::invalid_input("folder must not be empty"));
                    }
                }
                Ok(())
            }
            Self::Search(command) => {
                validate_source_path(&command.source)?;
                if command.query.trim().is_empty() {
                    return Err(CoreError::invalid_input("search query must not be empty"));
                }
                if let Some(max) = command.max_results {
                    if max == 0 {
                        return Err(CoreError::invalid_input("max_results must be greater than zero"));
                    }
                }
                Ok(())
            }
            Self::Extract(command) => {
                validate_source_path(&command.source)?;
                if command.message_id.is_none() && command.attachment_id.is_none() {
                    return Err(CoreError::invalid_input(
                        "extract command needs message-id or attachment-id",
                    ));
                }
                if command.out.is_none() {
                    return Err(CoreError::invalid_input("extract command requires --out"));
                }
                Ok(())
            }
            Self::Export(command) => {
                validate_source_path(&command.source)?;
                if command.out.is_none() {
                    return Err(CoreError::invalid_input("export command requires --out"));
                }
                Ok(())
            }
            Self::Validate(command) => validate_source_path(&command.source),
            Self::Index(command) => validate_source_path(&command.source),
            Self::Watch(command) => {
                if command.dir.as_os_str().is_empty() {
                    return Err(CoreError::invalid_input("watch directory is required"));
                }
                if command.on_changed.trim().is_empty() {
                    return Err(CoreError::invalid_input("on_changed command is required"));
                }
                if let Some(pattern) = &command.pattern {
                    if pattern.trim().is_empty() {
                        return Err(CoreError::invalid_input("pattern must not be empty"));
                    }
                }
                Ok(())
            }
            Self::Ui(command) => {
                if command.bind.trim().is_empty() {
                    return Err(CoreError::invalid_input("ui bind address is required"));
                }
                Ok(())
            }
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub runtime: RuntimeExecutionConfig,
    pub output: OutputFormat,
    pub deterministic: bool,
    pub strict: bool,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            runtime: RuntimeExecutionConfig::default(),
            output: OutputFormat::Table,
            deterministic: false,
            strict: false,
        }
    }
}

impl ExecutionContext {
    pub fn merged_strict(command: &Command, this: &Self) -> bool {
        this.strict || command.shared_options().strict
    }

    pub fn output_for(&self, command: &Command) -> OutputFormat {
        if command.shared_options().output != OutputFormat::Table {
            command.shared_options().output
        } else {
            self.output
        }
    }
}

pub trait CommandExecutor {
    fn execute_info(&self, _command: &InfoCommand, _ctx: &ExecutionContext) -> CoreResult<Mailbox> {
        Err(CoreError::unsupported("info command not implemented"))
    }

    fn execute_folders(
        &self,
        _command: &FoldersCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<FolderListResult> {
        Err(CoreError::unsupported("folders command not implemented"))
    }

    fn execute_messages(
        &self,
        _command: &MessagesCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<MessageListResult> {
        Err(CoreError::unsupported("messages command not implemented"))
    }

    fn execute_search(
        &self,
        _command: &SearchCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<SearchResult> {
        Err(CoreError::unsupported("search command not implemented"))
    }

    fn execute_extract(
        &self,
        _command: &ExtractCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<ExportResult> {
        Err(CoreError::unsupported("extract command not implemented"))
    }

    fn execute_export(
        &self,
        _command: &ExportCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<ExportResult> {
        Err(CoreError::unsupported("export command not implemented"))
    }

    fn execute_validate(
        &self,
        _command: &ValidateCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<ValidationResult> {
        Err(CoreError::unsupported("validate command not implemented"))
    }

    fn execute_index(
        &self,
        _command: &IndexCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<IndexResult> {
        Err(CoreError::unsupported("index command not implemented"))
    }

    fn execute_watch(
        &self,
        _command: &WatchCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<WatchResult> {
        Err(CoreError::unsupported("watch command not implemented"))
    }

    fn execute_ui(
        &self,
        _command: &UiCommand,
        _ctx: &ExecutionContext,
    ) -> CoreResult<UiResult> {
        Err(CoreError::unsupported("ui command not implemented"))
    }
}

pub fn execute_command<E: CommandExecutor>(
    executor: &E,
    command: &Command,
    context: &ExecutionContext,
) -> CoreResult<CommandResult> {
    let payload = match command {
        Command::Info(cmd) => CommandPayload::Mailbox(executor.execute_info(cmd, context)?),
        Command::Folders(cmd) => CommandPayload::Folders(executor.execute_folders(cmd, context)?),
        Command::Messages(cmd) => CommandPayload::Messages(executor.execute_messages(cmd, context)?),
        Command::Search(cmd) => CommandPayload::Search(executor.execute_search(cmd, context)?),
        Command::Extract(cmd) => CommandPayload::Export(executor.execute_extract(cmd, context)?),
        Command::Export(cmd) => CommandPayload::Export(executor.execute_export(cmd, context)?),
        Command::Validate(cmd) => CommandPayload::Validation(executor.execute_validate(cmd, context)?),
        Command::Index(cmd) => CommandPayload::Index(executor.execute_index(cmd, context)?),
        Command::Watch(cmd) => CommandPayload::Watch(executor.execute_watch(cmd, context)?),
        Command::Ui(cmd) => CommandPayload::Ui(executor.execute_ui(cmd, context)?),
    };

    Ok(CommandResult {
        output: context.output_for(command),
        payload,
    })
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct IndexResult {
    pub mailbox_id: Option<MailboxId>,
    pub db_path: Option<PathBuf>,
    pub mode: SearchMode,
    pub policy: IndexPolicy,
    pub deterministic: bool,
    pub documents: u64,
    pub segments: u64,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct WatchResult {
    pub watched_dir: PathBuf,
    pub matched_files: u64,
    pub processed_events: u64,
    pub failed: u64,
    pub last_error: Option<String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct UiResult {
    pub session_id: String,
    pub bind: String,
    pub destination: Option<PathBuf>,
    pub started: bool,
    pub deterministic: bool,
}

fn validate_source_path(path: &Path) -> CoreResult<()> {
    if path.as_os_str().is_empty() {
        Err(CoreError::invalid_input("source path must not be empty"))
    } else {
        Ok(())
    }
}
