//! CLI wiring for `core` contracts and parser/index/export/ui adapters.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command as ShellCommand,
    str::FromStr,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::Utc;
use clap::{Args, Parser, Subcommand, ValueEnum};
use pst_pst_pst_core::{
    AttachmentId,
    Command as CoreCommand,
    CommandExecutor,
    CommandPayload,
    CommandResult,
    ContainerFormat,
    CoreError,
    CoreResult,
    ErrorClass,
    ExecutionContext,
    ExportFormat,
    FilterField,
    Folder,
    FolderId,
    FolderListResult,
    InfoCommand,
    IndexCommand,
    IndexPolicy,
    IndexResult,
    Mailbox,
    MailboxId,
    MailboxState,
    Message,
    MessageId,
    MessageListResult,
    MessagesCommand,
    MatchSource,
    OutputFormat,
    ParseEvent,
    ParseEventId,
    ParseStage,
    PageInfo,
    RuntimeExecutionConfig,
    SearchCommand,
    SearchFilter,
    SearchHit,
    SearchMode,
    SharedCommandOptions,
    Severity,
    ValidationResult,
    ValidateCommand,
    WatchCommand,
    WatchResult,
    ExportResult as CoreExportResult,
    UiCommand as CoreUiCommand,
};
use pst_pst_pst_export::{
    progress_channel, ExportConfig, ExportFormat as ExportRuntimeFormat, ExportManifest, InMemoryCheckpointStore,
    MockExportEngine,
};
use pst_pst_pst_index::{
    IndexBuildRequest, IndexBuildState, IndexEngine, IndexMatch, IndexMatchMetadata, IndexQuery,
    IndexQueryResult, IndexReader, IndexSegment, IndexWriter,
};
use pst_pst_pst_parser::{
    DiscoveryReport, ParserConfig, ParserError, ParserRegistry, ParsedStore,
};
use pst_pst_pst_ui::{
    TerminalUi, UiCommand as UiShellCommand, UiCommandBus, UiCommandKind, UiCommandResult, UiConfig,
    UiEvent, UiMode, UiOutput, UiPayload, UiRuntime,
};
use serde_json::{to_string, to_string_pretty};

#[derive(Debug, Parser)]
#[command(name = "pst-pst-pst")]
#[command(about = "Rust-native parser/index/export/watch/ui CLI")]
struct Cli {
    #[arg(long, default_value_t = 4)]
    jobs: usize,

    #[arg(long, default_value_t = 4)]
    io_jobs: usize,

    #[arg(long, default_value_t = 4)]
    cpu_jobs: usize,

    #[arg(long)]
    single_thread: bool,

    #[arg(long)]
    deterministic: bool,

    #[arg(long)]
    strict: bool,

    #[arg(long, value_enum, default_value_t = OutputArg::Table)]
    output: OutputArg,

    #[arg(long, value_enum)]
    container: Option<ContainerArg>,

    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputArg {
    Table,
    Json,
    Jsonl,
    Ndjson,
}

impl OutputArg {
    fn into_core(self) -> OutputFormat {
        match self {
            Self::Table => OutputFormat::Table,
            Self::Json => OutputFormat::Json,
            Self::Jsonl => OutputFormat::Jsonl,
            Self::Ndjson => OutputFormat::Ndjson,
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum ContainerArg {
    Auto,
    Pst,
    Ost,
    Msg,
}

impl ContainerArg {
    fn into_core(self) -> ContainerFormat {
        match self {
            Self::Auto => ContainerFormat::Unknown,
            Self::Pst => ContainerFormat::Pst,
            Self::Ost => ContainerFormat::Ost,
            Self::Msg => ContainerFormat::Msg,
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum SearchModeArg {
    Auto,
    Full,
    Indexed,
    Hybrid,
}

impl SearchModeArg {
    fn into_core(self) -> SearchMode {
        match self {
            Self::Auto => SearchMode::Auto,
            Self::Full => SearchMode::Full,
            Self::Indexed => SearchMode::Indexed,
            Self::Hybrid => SearchMode::Hybrid,
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum IndexPolicyArg {
    Allow,
    Require,
    Refresh,
    Build,
}

impl IndexPolicyArg {
    fn into_core(self) -> IndexPolicy {
        match self {
            Self::Allow => IndexPolicy::Allow,
            Self::Require => IndexPolicy::Require,
            Self::Refresh => IndexPolicy::Refresh,
            Self::Build => IndexPolicy::Build,
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum ExportFormatArg {
    Eml,
    Mbox,
    Json,
    Jsonl,
    Msg,
}

impl ExportFormatArg {
    fn into_core(self) -> ExportFormat {
        match self {
            Self::Eml => ExportFormat::Eml,
            Self::Mbox => ExportFormat::Mbox,
            Self::Json => ExportFormat::Json,
            Self::Jsonl => ExportFormat::Jsonl,
            Self::Msg => ExportFormat::Msg,
        }
    }
}

#[derive(Debug, Clone, Args)]
struct SharedOptions {
    #[arg(long)]
    filter: Vec<String>,

    #[arg(long)]
    output: Option<OutputArg>,

    #[arg(long)]
    limit: Option<u64>,

    #[arg(long)]
    sort: Option<String>,

    #[arg(long)]
    deterministic: bool,

    #[arg(long)]
    strict: bool,

    #[arg(long)]
    page_token: Option<String>,
}

impl SharedOptions {
    fn to_core(self, _global_output: OutputFormat) -> SharedCommandOptions {
        SharedCommandOptions {
            filter: self.filter,
            output: self.output.map_or(OutputFormat::Table, OutputArg::into_core),
            limit: self.limit,
            sort: self.sort,
            deterministic: self.deterministic,
            strict: self.strict,
            page_token: self.page_token,
        }
    }
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    Info {
        source: PathBuf,
        #[command(flatten)]
        options: SharedOptions,
    },
    Folders {
        source: PathBuf,
        #[arg(long)]
        folder: Option<String>,
        #[command(flatten)]
        options: SharedOptions,
    },
    Messages {
        source: PathBuf,
        #[arg(long)]
        folder: Option<String>,
        #[command(flatten)]
        options: SharedOptions,
    },
    Search {
        source: PathBuf,
        #[arg(short = 'q', long)]
        query: String,
        #[arg(long, value_delimiter = ',')]
        fields: Vec<String>,
        #[arg(long = "search-mode", value_enum, default_value_t = SearchModeArg::Auto)]
        mode: SearchModeArg,
        #[arg(long = "index-policy", value_enum, default_value_t = IndexPolicyArg::Allow)]
        index_policy: IndexPolicyArg,
        #[arg(long)]
        include_unindexed: bool,
        #[arg(long)]
        max_results: Option<u64>,
        #[command(flatten)]
        options: SharedOptions,
    },
    Extract {
        source: PathBuf,
        #[arg(long)]
        message_id: Option<String>,
        #[arg(long)]
        attachment_id: Option<String>,
        #[arg(short = 'o', long)]
        out: PathBuf,
        #[command(flatten)]
        options: SharedOptions,
    },
    Export {
        source: PathBuf,
        #[arg(short = 'f', long, value_enum, default_value_t = ExportFormatArg::Jsonl)]
        format: ExportFormatArg,
        #[arg(short = 'o', long)]
        out: PathBuf,
        #[arg(long)]
        folder: Option<String>,
        #[arg(long)]
        message_ids: Vec<String>,
        #[command(flatten)]
        options: SharedOptions,
    },
    Validate {
        source: PathBuf,
        #[arg(long)]
        report: Option<PathBuf>,
        #[command(flatten)]
        options: SharedOptions,
    },
    Index {
        source: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        rebuild: bool,
        #[command(flatten)]
        options: SharedOptions,
    },
    Watch {
        dir: PathBuf,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long)]
        on_changed: String,
        #[command(flatten)]
        options: SharedOptions,
    },
    Ui {
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: String,
        #[command(flatten)]
        options: SharedOptions,
    },
}

#[derive(Debug, Clone)]
struct GlobalContext {
    output: OutputFormat,
    runtime: RuntimeExecutionConfig,
    deterministic: bool,
    strict: bool,
    requested_container: ContainerFormat,
}

impl GlobalContext {
    fn from_cli(cli: &Cli) -> CoreResult<Self> {
        let runtime = RuntimeExecutionConfig {
            jobs: cli.jobs.max(1),
            io_jobs: cli.io_jobs.max(1),
            cpu_jobs: cli.cpu_jobs.max(1),
            single_thread: cli.single_thread,
            strict: cli.strict,
            include_unindexed: false,
            index_staleness_threshold: None,
        };
        runtime.validate().map_err(|error| CoreError::invalid_input(error.to_string()))?;

        Ok(Self {
            output: cli.output.into_core(),
            runtime: runtime.normalize(),
            deterministic: cli.deterministic,
            strict: cli.strict,
            requested_container: cli
                .container
                .unwrap_or(ContainerArg::Auto)
                .into_core(),
        })
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext {
            runtime: self.runtime,
            output: self.output,
            deterministic: self.deterministic,
            strict: self.strict,
        }
    }
}

#[derive(Clone)]
struct CliExecutor {
    parser: ParserRegistry,
    index: Arc<Mutex<CliIndex>>,
    export_engine: MockExportEngine,
    checkpoint_store: InMemoryCheckpointStore,
    requested_container: ContainerFormat,
}

impl CliExecutor {
    fn new(context: &GlobalContext) -> Self {
        Self {
            parser: ParserRegistry::new(),
            index: Arc::new(Mutex::new(CliIndex::default())),
            export_engine: MockExportEngine::default(),
            checkpoint_store: InMemoryCheckpointStore::default(),
            requested_container: context.requested_container,
        }
    }

    fn parse_store(&self, source: &Path, strict: bool, deterministic: bool) -> CoreResult<ParsedStore> {
        let config = ParserConfig {
            source_path: source.to_path_buf(),
            requested_container: self.requested_container,
            strict,
            allow_fallback: true,
            deterministic,
            max_bytes: None,
        };

        let discovery = self
            .parser
            .discover(&config)
            .map_err(|error| map_parser_error(error))?;
        match self.parser.parse(&config) {
            Ok(store) => Ok(store),
            Err(error) if !strict => Ok(self.scaffolded_store(source, &discovery)),
            Err(error) => Err(map_parser_error(error)),
        }
    }

    fn scaffolded_store(&self, source: &Path, discovery: &DiscoveryReport) -> ParsedStore {
        let selected = discovery
            .selected
            .as_ref()
            .or_else(|| discovery.candidates.first())
            .map(|candidate| candidate.container)
            .unwrap_or(self.requested_container);

        let mut mailbox = Mailbox::new(source.to_path_buf(), selected);
        mailbox.state = MailboxState::Degraded;

        let fallback_notes = if discovery.fallback_reasons.is_empty() {
            vec!["fallback enabled due parser failure".to_string()]
        } else {
            discovery.fallback_reasons.clone()
        };

        let mut events = Vec::new();
        for message in fallback_notes {
            events.push(ParseEvent {
                id: ParseEventId::new(),
                location: None,
                stage: ParseStage::Discovery,
                class: ErrorClass::Parse,
                severity: Severity::Warn,
                message,
                details: None,
                occurs_at: Utc::now(),
            });
        }

        if let Some(candidate) = discovery.selected.as_ref() {
            events.push(ParseEvent {
                id: ParseEventId::new(),
                location: None,
                stage: ParseStage::Discovery,
                class: ErrorClass::Parse,
                severity: Severity::Warn,
                message: format!("selected backend `{}`", candidate.backend_name),
                details: Some(format!("container {:?}", candidate.container)),
                occurs_at: Utc::now(),
            });
        }

        mailbox.diagnostics = events.clone();

        let folder = Folder {
            id: FolderId::new(),
            mailbox_id: mailbox.id,
            parent_id: None,
            name: "root".to_string(),
            path: "/".to_string(),
            message_count: 0,
            unread_count: 0,
            total_size: 0,
            has_subfolders: false,
            is_hidden: false,
            is_root: true,
        };

        ParsedStore {
            mailbox,
            folders: vec![folder],
            messages: Vec::new(),
            attachments: Vec::new(),
            events,
            discovery: discovery.clone(),
        }
    }

    fn command_deterministic(&self, command: &SharedCommandOptions, ctx: &ExecutionContext) -> bool {
        command.deterministic || ctx.deterministic
    }

    fn command_strict(&self, command: &SharedCommandOptions, ctx: &ExecutionContext) -> bool {
        command.strict || ctx.strict
    }

    fn page_info(&self, limit: Option<u64>, page_token: &Option<String>) -> CoreResult<PageInfo> {
        let offset = match page_token {
            Some(token) => token
                .parse::<u64>()
                .map_err(|error| CoreError::invalid_input(format!("invalid page token `{token}`: {error}")))?,
            None => 0,
        };

        let limit = limit.unwrap_or(100).max(1);
        Ok(PageInfo {
            limit,
            offset,
            has_more: false,
            page_token: page_token.clone(),
            next_page_token: None,
        })
    }

    fn folder_path_to_id(&self, folders: &[Folder], path_or_id: &str) -> Option<FolderId> {
        folders.iter().find_map(|folder| {
            if folder.path == path_or_id
                || folder.name == path_or_id
                || folder.id.to_string() == path_or_id
            {
                Some(folder.id)
            } else {
                None
            }
        })
    }

    fn searchable_text(
        &self,
        folder_name: Option<&str>,
        message: &Message,
    ) -> String {
        let mut chunks = Vec::new();
        if let Some(folder_name) = folder_name {
            chunks.push(folder_name.to_ascii_lowercase());
        }
        if let Some(subject) = &message.subject {
            chunks.push(subject.to_ascii_lowercase());
        }
        if let Some(sender) = &message.sender {
            if let Some(address) = &sender.address {
                chunks.push(address.to_ascii_lowercase());
            }
            if let Some(name) = &sender.display_name {
                chunks.push(name.to_ascii_lowercase());
            }
        }
        for recipient in &message.recipients {
            if let Some(address) = &recipient.address {
                chunks.push(address.to_ascii_lowercase());
            }
            if let Some(name) = &recipient.display_name {
                chunks.push(name.to_ascii_lowercase());
            }
        }
        if let Some(body) = &message.body {
            if let Some(content_ref) = &body.content_ref {
                chunks.push(content_ref.to_ascii_lowercase());
            }
        }
        chunks.join(" ")
    }

    fn build_index_if_needed(
        &self,
        parsed: &ParsedStore,
        deterministic: bool,
        force: bool,
    ) -> CoreResult<(u64, u64)> {
        let mut index = self
            .index
            .lock()
            .map_err(|error| CoreError::invalid_input(format!("index lock poisoned: {error}")))?;

        let request = IndexBuildRequest {
            mailbox_id: parsed.mailbox.id,
            deterministic,
            replace_existing: true,
            source_mode: Some(SearchMode::Indexed),
        };
        index.request_build(request).map_err(map_index_error)?;

        let folder_map: HashMap<FolderId, &Folder> = parsed.folders.iter().map(|folder| (folder.id, folder)).collect();
        let docs = parsed
            .messages
            .iter()
            .map(|message| {
                let folder = folder_map.get(&message.folder_id).map(|folder| folder.path.as_str());
                IndexedDocument {
                    message_id: message.id,
                    folder_id: message.folder_id,
                    searchable: self.searchable_text(folder.copied(), message),
                }
            })
            .collect();
        index.upsert(parsed.mailbox.id, docs, deterministic, force)?;

        let state = IndexBuildState::Completed {
            mailbox_id: parsed.mailbox.id,
            deterministic,
            total_documents: parsed.messages.len() as u64,
            total_segments: index.segment_count(parsed.mailbox.id),
        };
        index.finish_build(state).map_err(map_index_error)?;

        Ok((
            index.document_count(parsed.mailbox.id),
            index.segment_count(parsed.mailbox.id),
        ))
    }

    fn index_has_data(&self, mailbox_id: &MailboxId) -> CoreResult<bool> {
        let index = self
            .index
            .lock()
            .map_err(|error| CoreError::invalid_input(format!("index lock poisoned: {error}")))?;
        Ok(index.document_count(*mailbox_id) > 0)
    }

    fn ensure_report_summary_file(
        &self,
        report: &Option<PathBuf>,
        validation: &ValidationResult,
    ) -> CoreResult<()> {
        if let Some(path) = report {
            let payload = to_string_pretty(validation)
                .map_err(|error| CoreError::invalid_input(format!("failed to serialize validation report: {error}")))?;
            let mut file = File::create(path)
                .map_err(|error| CoreError::io(Some(path.clone()), format!("failed to create validation report: {error}")))?;
            file.write_all(payload.as_bytes())
                .map_err(|error| CoreError::io(Some(path.clone()), format!("failed to write validation report: {error}")))?;
        }
        Ok(())
    }

    fn run_export(
        &self,
        config: ExportConfig,
        emit_progress: bool,
        deterministic: bool,
    ) -> CoreResult<CoreExportResult> {
        let (handle, receiver) = progress_channel(4096);
        let manifest = self
            .export_engine
            .execute(&config, None, &handle, &self.checkpoint_store)
            .map_err(|error| CoreError::Export {
                destination: Some(config.destination.clone()),
                message: format!("export engine failed: {error}"),
                details: None,
            })?;

        if emit_progress {
            while let Ok(event) = receiver.recv_timeout(Duration::from_millis(25)) {
                let _ = event; // future: route progress to logging sink
                match event {
                    // no-op
                    _ => {}
                }
            }
        }

        let summary = manifest
            .core_summary
            .unwrap_or_else(|| {
                CoreExportResult {
                    mailbox_id: config.mailbox_id.unwrap_or_else(MailboxId::new),
                    requested: manifest.requested,
                    exported: manifest.exported,
                    skipped: manifest.skipped,
                    failed: manifest.failed,
                    destination: manifest.destination,
                    manifest_path: manifest.checkpoint_path.clone(),
                    deterministic,
                }
            });

        Ok(summary)
    }

    fn search_terms(query: &str) -> Vec<String> {
        query
            .split_whitespace()
            .map(|token| token.to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect()
    }

    fn query_fields(fields: &[String]) -> Vec<String> {
        if fields.is_empty() {
            vec![
                "subject".to_string(),
                "sender".to_string(),
                "folder".to_string(),
                "body".to_string(),
            ]
        } else {
            fields.iter().map(|field| field.to_ascii_lowercase()).collect()
        }
    }

    fn parse_ids<T>(values: &[String], label: &str) -> CoreResult<Vec<T>>
    where
        T: FromStr,
        <T as FromStr>::Err: std::fmt::Display,
    {
        let mut ids = Vec::with_capacity(values.len());
        for value in values {
            ids.push(
                value
                    .parse::<T>()
                    .map_err(|error| CoreError::invalid_input(format!("invalid {label} `{value}`: {error}")))?,
            );
        }
        Ok(ids)
    }
}

impl CommandExecutor for CliExecutor {
    fn execute_info(
        &self,
        command: &InfoCommand,
        context: &ExecutionContext,
    ) -> CoreResult<Mailbox> {
        let parsed = self.parse_store(
            &command.source,
            self.command_strict(&command.options, context),
            self.command_deterministic(&command.options, context),
        )?;

        let mut mailbox = parsed.mailbox;
        mailbox.folder_count = parsed.folders.len() as u64;
        mailbox.message_count = parsed.messages.len() as u64;
        mailbox.attachment_count = parsed.attachments.len() as u64;
        mailbox.parse_error_count = parsed
            .events
            .iter()
            .filter(|event| matches!(event.severity, Severity::Error | Severity::Fatal))
            .count() as u64;
        mailbox.diagnostics = parsed.events;
        Ok(mailbox)
    }

    fn execute_folders(
        &self,
        command: &pst_pst_pst_core::FoldersCommand,
        context: &ExecutionContext,
    ) -> CoreResult<FolderListResult> {
        let parsed = self.parse_store(
            &command.source,
            self.command_strict(&command.options, context),
            self.command_deterministic(&command.options, context),
        )?;

        let mut folders = parsed.folders;
        if let Some(folder_filter) = &command.folder {
            folders.retain(|folder| {
                folder.path == *folder_filter
                    || folder.name == *folder_filter
                    || folder.id.to_string() == *folder_filter
            });
        }

        if self.command_deterministic(&command.options, context) {
            folders.sort_by(|lhs, rhs| lhs.path.cmp(&rhs.path).then(lhs.id.to_string().cmp(&rhs.id.to_string())));
        }

        let mut page = self.page_info(command.options.limit, &command.options.page_token)?;
        let start = page.offset as usize;
        let end = (start + page.limit as usize).min(folders.len());
        page.has_more = end < folders.len();
        if page.has_more {
            page.next_page_token = Some((end as u64).to_string());
        }

        Ok(FolderListResult {
            mailbox_id: parsed.mailbox.id,
            folders: folders.into_iter().skip(start).take(page.limit as usize).collect(),
            scanned: parsed.folders.len() as u64,
            page,
        })
    }

    fn execute_messages(
        &self,
        command: &MessagesCommand,
        context: &ExecutionContext,
    ) -> CoreResult<MessageListResult> {
        let parsed = self.parse_store(
            &command.source,
            self.command_strict(&command.options, context),
            self.command_deterministic(&command.options, context),
        )?;

        let folder_id = command
            .folder
            .as_deref()
            .and_then(|value| self.folder_path_to_id(&parsed.folders, value));

        let mut messages = parsed.messages;
        if let Some(folder_id) = folder_id {
            messages.retain(|message| message.folder_id == folder_id);
        }

        if self.command_deterministic(&command.options, context) {
            messages.sort_by(|lhs, rhs| lhs.id.to_string().cmp(&rhs.id.to_string()));
        }

        let mut page = self.page_info(command.options.limit, &command.options.page_token)?;
        let start = page.offset as usize;
        let end = (start + page.limit as usize).min(messages.len());
        page.has_more = end < messages.len();
        if page.has_more {
            page.next_page_token = Some((end as u64).to_string());
        }

        Ok(MessageListResult {
            mailbox_id: parsed.mailbox.id,
            folder_id,
            messages: messages.into_iter().skip(start).take(page.limit as usize).collect(),
            scanned: parsed.messages.len() as u64,
            page,
        })
    }

    fn execute_search(
        &self,
        command: &SearchCommand,
        context: &ExecutionContext,
    ) -> CoreResult<pst_pst_pst_core::SearchResult> {
        let parsed = self.parse_store(
            &command.source,
            self.command_strict(&command.options, context),
            self.command_deterministic(&command.options, context),
        )?;

        let deterministic = self.command_deterministic(&command.options, context);
        let mut page = self.page_info(command.max_results.or(command.options.limit), &command.options.page_token)?;
        let terms = Self::search_terms(&command.query);
        if terms.is_empty() {
            return Err(CoreError::invalid_input("search query cannot be empty"));
        }

        let folder_map: HashMap<FolderId, &Folder> =
            parsed.folders.iter().map(|folder| (folder.id, folder)).collect();
        let has_index = self.index_has_data(&parsed.mailbox.id)?;
        let force_index = matches!(
            (command.mode, command.index_policy),
            (SearchMode::Auto, IndexPolicy::Build)
                | (SearchMode::Auto, IndexPolicy::Refresh)
                | (SearchMode::Indexed, IndexPolicy::Build)
                | (SearchMode::Indexed, IndexPolicy::Refresh)
                | (SearchMode::Hybrid, IndexPolicy::Build)
                | (SearchMode::Hybrid, IndexPolicy::Refresh)
        );

        let source_mode = match command.mode {
            SearchMode::Full => SearchMode::Full,
            SearchMode::Indexed => {
                if has_index {
                    SearchMode::Indexed
                } else {
                    match command.index_policy {
                        IndexPolicy::Require => {
                            return Err(CoreError::index(
                                Some(parsed.mailbox.id.to_string()),
                                "indexed search requested but no index is available",
                            ));
                        }
                        IndexPolicy::Build | IndexPolicy::Refresh => SearchMode::Indexed,
                        IndexPolicy::Allow => SearchMode::Full,
                    }
                }
            }
            SearchMode::Hybrid => {
                if has_index {
                    SearchMode::Hybrid
                } else {
                    match command.index_policy {
                        IndexPolicy::Require => {
                            return Err(CoreError::index(
                                Some(parsed.mailbox.id.to_string()),
                                "hybrid search requested but no index is available",
                            ));
                        }
                        IndexPolicy::Build | IndexPolicy::Refresh => SearchMode::Hybrid,
                        IndexPolicy::Allow => SearchMode::Full,
                    }
                }
            }
            SearchMode::Auto => {
                if force_index || has_index {
                    SearchMode::Indexed
                } else {
                    SearchMode::Full
                }
            }
        };

        let effective_mode = source_mode;
        let mut used_index = false;
        let fields = Self::query_fields(&command.fields);

        if matches!(effective_mode, SearchMode::Indexed | SearchMode::Hybrid) {
            if source_mode != SearchMode::Full {
                let (_documents, _segments) = self.build_index_if_needed(
                    &parsed,
                    deterministic,
                    force_index || !has_index,
                )?;

                let query = IndexQuery {
                    mailbox_id: parsed.mailbox.id,
                    text: Some(command.query.clone()),
                    filter: SearchFilter::default(),
                    page: page.clone(),
                    mode: effective_mode,
                    include_unindexed: command.include_unindexed,
                    deterministic,
                    segment_ids: Vec::new(),
                };

                let indexed_result = {
                    let index = self
                        .index
                        .lock()
                        .map_err(|error| CoreError::invalid_input(format!("index lock poisoned: {error}")))?;
                    index.query(&query).map_err(|error| error)?
                };

                let total = indexed_result.total;
                let mut hits = Vec::with_capacity(indexed_result.matches.len());
                for item in indexed_result.matches {
                    hits.push(SearchHit {
                        message_id: item.message_id,
                        folder_id: item.folder_id,
                        score: item.metadata.score,
                        match_source: item.metadata.match_source,
                        matched_fields: if item.metadata.matched_fields.is_empty() {
                            vec![FilterField::Raw("indexed".to_string())]
                        } else {
                            item.metadata.matched_fields
                        },
                        snippet: item.metadata.snippet,
                    });
                }

                let returned = hits.len() as u64;
                let start = page.offset as usize;
                let end = (start + page.limit as usize).min(hits.len());
                page.has_more = end < total as usize;
                if page.has_more {
                    page.next_page_token = Some((start as u64 + page.limit).to_string());
                }

                used_index = true;
                return Ok(pst_pst_pst_core::SearchResult {
                    mailbox_id: parsed.mailbox.id,
                    hits: hits.into_iter().skip(start).take(page.limit as usize).collect(),
                    total,
                    returned: returned.min(page.limit),
                    query: Some(command.query.clone()),
                    source_mode: effective_mode,
                    include_unindexed: command.include_unindexed,
                    deterministic,
                    page,
                });
            }
        }

        if used_index {
            return Err(CoreError::index(
                Some(parsed.mailbox.id.to_string()),
                "index mode fallback path was requested but no indexed path was produced",
            ));
        }

        let mut hits: Vec<SearchHit> = Vec::new();
        for message in &parsed.messages {
            let folder_name = folder_map.get(&message.folder_id).map(|folder| folder.path.as_str());
            let haystack = self.searchable_text(folder_name.copied(), message);
            let mut matched = 0usize;
            for term in &terms {
                if haystack.contains(term) {
                    matched += 1;
                }
            }
            if matched == terms.len() {
                let snippet = haystack.chars().take(140).collect::<String>();
                let matched_fields = fields
                    .iter()
                    .map(|value| FilterField::Raw(value.to_string()))
                    .collect();
                hits.push(SearchHit {
                    message_id: message.id,
                    folder_id: message.folder_id,
                    score: None,
                    match_source: MatchSource::Full,
                    matched_fields,
                    snippet: Some(snippet),
                });
            }
        }

        if deterministic {
            hits.sort_by(|lhs, rhs| lhs.message_id.to_string().cmp(&rhs.message_id.to_string()));
        }
        let total = hits.len() as u64;
        let start = page.offset as usize;
        let end = (start + page.limit as usize).min(hits.len());
        page.has_more = end < hits.len();
        if page.has_more {
            page.next_page_token = Some((start as u64 + page.limit).to_string());
        }

        Ok(pst_pst_pst_core::SearchResult {
            mailbox_id: parsed.mailbox.id,
            hits: hits.into_iter().skip(start).take(page.limit as usize).collect(),
            total,
            returned: (end - start) as u64,
            query: Some(command.query.clone()),
            source_mode: SearchMode::Full,
            include_unindexed: command.include_unindexed,
            deterministic,
            page,
        })
    }

    fn execute_extract(
        &self,
        command: &pst_pst_pst_core::ExtractCommand,
        context: &ExecutionContext,
    ) -> CoreResult<CoreExportResult> {
        let parsed = self.parse_store(
            &command.source,
            self.command_strict(&command.options, context),
            self.command_deterministic(&command.options, context),
        )?;

        let deterministic = self.command_deterministic(&command.options, context);
        let strict = self.command_strict(&command.options, context);
        let message_ids = match command.message_id.as_ref() {
            Some(id) => vec![id.parse::<MessageId>().map_err(|error| {
                CoreError::invalid_input(format!("invalid message id `{id}`: {error}"))
            })?],
            None => Vec::new(),
        };
        let attachment_ids = match command.attachment_id.as_ref() {
            Some(id) => vec![id.parse::<AttachmentId>().map_err(|error| {
                CoreError::invalid_input(format!("invalid attachment id `{id}`: {error}"))
            })?],
            None => Vec::new(),
        };

        let config = ExportConfig {
            source_path: command.source.clone(),
            destination: command.out.clone(),
            mailbox_id: Some(parsed.mailbox.id),
            folder_ids: Vec::new(),
            message_ids,
            attachment_ids,
            format: ExportRuntimeFormat::Eml,
            deterministic,
            strict,
            checkpoint_path: Some(command.out.join(".pst-pst-pst-extract.checkpoint")),
            workers: context.runtime.jobs,
            max_messages: Some(1),
        };
        self.run_export(config, false, deterministic)
    }

    fn execute_export(
        &self,
        command: &pst_pst_pst_core::ExportCommand,
        context: &ExecutionContext,
    ) -> CoreResult<CoreExportResult> {
        let parsed = self.parse_store(
            &command.source,
            self.command_strict(&command.options, context),
            self.command_deterministic(&command.options, context),
        )?;
        let deterministic = self.command_deterministic(&command.options, context);
        let strict = self.command_strict(&command.options, context);

        let message_ids = Self::parse_ids::<MessageId>(&command.message_ids, "message-id")?;
        let mut folder_ids = Vec::new();
        if let Some(folder) = &command.folder {
            if let Ok(id) = folder.parse::<FolderId>() {
                folder_ids.push(id);
            } else if !folder.trim().is_empty() {
                return Err(CoreError::invalid_input(format!(
                    "unsupported folder selector in CLI: `{folder}` (expected folder id)"
                )));
            }
        }

        let format = match command.format {
            ExportFormat::Eml => ExportRuntimeFormat::Eml,
            ExportFormat::Mbox => ExportRuntimeFormat::Mbox,
            ExportFormat::Json => ExportRuntimeFormat::Json,
            ExportFormat::Jsonl => ExportRuntimeFormat::Jsonl,
            ExportFormat::Msg => ExportRuntimeFormat::Binary,
        };

        let config = ExportConfig {
            source_path: command.source.clone(),
            destination: command.out.clone(),
            mailbox_id: Some(parsed.mailbox.id),
            folder_ids,
            message_ids,
            attachment_ids: Vec::new(),
            format,
            deterministic,
            strict,
            checkpoint_path: Some(command.out.join(".pst-pst-pst-export.checkpoint")),
            workers: context.runtime.jobs,
            max_messages: Some(parsed.messages.len() as u64),
        };

        self.run_export(config, !matches!(context.output, OutputFormat::Table), deterministic)
    }

    fn execute_validate(
        &self,
        command: &ValidateCommand,
        context: &ExecutionContext,
    ) -> CoreResult<ValidationResult> {
        let parsed = self.parse_store(
            &command.source,
            self.command_strict(&command.options, context),
            self.command_deterministic(&command.options, context),
        )?;

        let passed = !parsed
            .events
            .iter()
            .any(|event| matches!(event.severity, Severity::Error | Severity::Fatal));
        let warnings = parsed
            .events
            .iter()
            .filter(|event| event.severity == Severity::Warn)
            .count() as u64;
        let errors = parsed
            .events
            .iter()
            .filter(|event| matches!(event.severity, Severity::Error | Severity::Fatal))
            .count() as u64;

        let mut result = ValidationResult {
            mailbox_id: parsed.mailbox.id,
            passed,
            scanned_items: parsed.messages.len() as u64 + parsed.attachments.len() as u64,
            warnings,
            errors,
            events: parsed.events,
        };

        self.ensure_report_summary_file(&command.report, &result)?;
        Ok(std::mem::take(&mut result))
    }

    fn execute_index(
        &self,
        command: &IndexCommand,
        context: &ExecutionContext,
    ) -> CoreResult<IndexResult> {
        let parsed = self.parse_store(
            &command.source,
            self.command_strict(&command.options, context),
            self.command_deterministic(&command.options, context),
        )?;
        let deterministic = self.command_deterministic(&command.options, context);

        let (documents, segments) = self.build_index_if_needed(
            &parsed,
            deterministic,
            command.rebuild || !self.index_has_data(&parsed.mailbox.id)?,
        )?;

        Ok(IndexResult {
            mailbox_id: Some(parsed.mailbox.id),
            db_path: command.db.clone(),
            mode: SearchMode::Indexed,
            policy: if command.rebuild {
                IndexPolicy::Build
            } else {
                IndexPolicy::Allow
            },
            deterministic,
            documents,
            segments,
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
        })
    }

    fn execute_watch(
        &self,
        command: &WatchCommand,
        _context: &ExecutionContext,
    ) -> CoreResult<WatchResult> {
        if command.dir.as_os_str().is_empty() {
            return Err(CoreError::invalid_input("watch directory is required"));
        }
        if command.on_changed.trim().is_empty() {
            return Err(CoreError::invalid_input("on-changed command is required"));
        }

        let pattern = command.pattern.clone().unwrap_or_else(|| "*".to_string());
        let mut seen: HashMap<PathBuf, u64> = HashMap::new();
        let mut processed = 0u64;
        let mut failed = 0u64;
        let mut matched_files = 0u64;
        let mut last_error = None;
        let mut cycle = 0u64;
        let max_cycles = std::env::var("PST_PST_PST_WATCH_CYCLES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok());

        loop {
            let current = discover_files(&command.dir, &pattern)?;
            for (path, modified) in current {
                let changed = match seen.get(&path) {
                    Some(previous) => *previous != modified,
                    None => true,
                };
                if changed {
                    matched_files = matched_files.saturating_add(1);
                    processed = processed.saturating_add(1);
                    let rendered = render_on_changed_template(&command.on_changed, &path);
                    if let Err(error) = run_on_changed(&rendered) {
                        failed = failed.saturating_add(1);
                        last_error = Some(error.to_string());
                    } else {
                        last_error = None;
                    }
                }
                seen.insert(path, modified);
            }

            cycle = cycle.saturating_add(1);
            if let Some(max_cycles) = max_cycles {
                if cycle >= max_cycles {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(750));
        }

        Ok(WatchResult {
            watched_dir: command.dir.clone(),
            matched_files,
            processed_events: processed,
            failed,
            last_error,
        })
    }

    fn execute_ui(
        &self,
        command: &CoreUiCommand,
        context: &ExecutionContext,
    ) -> CoreResult<pst_pst_pst_ui::UiResult> {
        let ui_output = match context.output {
            OutputFormat::Json => UiOutput::Jsonl,
            OutputFormat::Jsonl => UiOutput::Jsonl,
            OutputFormat::Ndjson => UiOutput::Ndjson,
            _ => UiOutput::Text,
        };
        let ui_config = UiConfig {
            mode: UiMode::Terminal,
            output: ui_output,
            deterministic: context.deterministic,
            max_history: 256,
        };

        let bus = CliUiBus::new(self.clone(), context.clone());
        let mut ui = TerminalUi::new(bus, ui_config);
        println!("ui session: bind={}", command.bind);

        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line.map_err(|error| {
                CoreError::io(
                    None,
                    format!("failed to read UI line: {error}"),
                )
            })?;
            let (events, rendered) = ui.submit(&line);
            for line in rendered {
                println!("{line}");
            }
            if events.iter().any(|event| matches!(event, UiEvent::Exit { .. })) {
                break;
            }
        }

        Ok(pst_pst_pst_ui::UiResult {
            session_id: format!("ui-{}", command.bind),
            bind: command.bind.clone(),
            destination: None,
            started: true,
            deterministic: context.deterministic,
        })
    }
}

#[derive(Default, Clone)]
struct CliIndex {
    docs: HashMap<MailboxId, Vec<IndexedDocument>>,
    segments: HashMap<MailboxId, Vec<IndexSegment>>,
    states: HashMap<MailboxId, IndexBuildState>,
}

#[derive(Clone)]
struct IndexedDocument {
    message_id: MessageId,
    folder_id: FolderId,
    searchable: String,
}

impl CliIndex {
    fn document_count(&self, mailbox_id: MailboxId) -> u64 {
        self.docs.get(&mailbox_id).map(|docs| docs.len() as u64).unwrap_or(0)
    }

    fn segment_count(&self, mailbox_id: MailboxId) -> u64 {
        self.segments
            .get(&mailbox_id)
            .map(|segments| segments.len() as u64)
            .unwrap_or(0)
    }

    fn upsert(
        &mut self,
        mailbox_id: MailboxId,
        mut docs: Vec<IndexedDocument>,
        deterministic: bool,
        replace: bool,
    ) -> CoreResult<()> {
        if replace {
            self.docs.remove(&mailbox_id);
            self.segments.remove(&mailbox_id);
        }
        if deterministic {
            docs.sort_by(|lhs, rhs| lhs.message_id.to_string().cmp(&rhs.message_id.to_string()));
        }
        let segment = IndexSegment {
            segment_id: format!("{mailbox_id}-seg0"),
            mailbox_id,
            generation: 1,
            document_count: docs.len() as u64,
            checksum: None,
            deterministic,
        };
        self.docs.insert(mailbox_id, docs);
        self.segments.insert(mailbox_id, vec![segment]);
        Ok(())
    }
}

impl IndexReader for CliIndex {
    type Error = CoreError;

    fn query(&self, query: &IndexQuery) -> Result<IndexQueryResult, Self::Error> {
        let docs = self
            .docs
            .get(&query.mailbox_id)
            .ok_or_else(|| CoreError::index(Some(query.mailbox_id.to_string()), "missing indexed documents"))?;
        let segments = self.segments.get(&query.mailbox_id).cloned().unwrap_or_default();

        let terms = query
            .text
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .map(|value| value.to_ascii_lowercase())
            .collect::<Vec<_>>();

        let mut matches = Vec::new();
        for doc in docs {
            let text = doc.searchable.as_str();
            if terms.iter().all(|term| text.contains(term)) {
                matches.push(IndexMatch {
                    message_id: doc.message_id,
                    folder_id: doc.folder_id,
                    metadata: IndexMatchMetadata {
                        match_source: MatchSource::Indexed,
                        score: Some(terms.len() as f64),
                        matched_fields: vec![FilterField::Raw("indexed-text".to_string())],
                        snippet: Some(doc.searchable.chars().take(140).collect()),
                    },
                });
            }
        }

        let total = matches.len() as u64;
        let start = query.page.offset as usize;
        let mut matched = matches;
        if query.deterministic {
            matched.sort_by(|lhs, rhs| lhs.message_id.to_string().cmp(&rhs.message_id.to_string()));
        }
        let end = (start + query.page.limit as usize).min(matched.len());
        let returned_hits = matched
            .into_iter()
            .skip(start)
            .take(query.page.limit as usize)
            .collect::<Vec<_>>();

        let cloned_query = IndexQuery {
            mailbox_id: query.mailbox_id,
            text: query.text.clone(),
            filter: query.filter.clone(),
            page: query.page.clone(),
            mode: query.mode,
            include_unindexed: query.include_unindexed,
            deterministic: query.deterministic,
            segment_ids: query.segment_ids.clone(),
        };

        Ok(IndexQueryResult {
            mailbox_id: query.mailbox_id,
            query: cloned_query,
            matches: returned_hits,
            segments,
            total,
            returned: total as usize,
            deterministic: query.deterministic,
        })
    }

    fn get_segment(&self, segment_id: &str) -> Result<Option<IndexSegment>, Self::Error> {
        for (_, segments) in &self.segments {
            if let Some(segment) = segments.iter().find(|segment| segment.segment_id == segment_id) {
                return Ok(Some(segment.clone()));
            }
        }
        Ok(None)
    }

    fn list_segments(&self, mailbox_id: MailboxId) -> Result<Vec<IndexSegment>, Self::Error> {
        Ok(self.segments.get(&mailbox_id).cloned().unwrap_or_default())
    }

    fn build_state(&self, mailbox_id: MailboxId) -> IndexBuildState {
        self.states.get(&mailbox_id).cloned().unwrap_or(IndexBuildState::Unknown)
    }
}

impl IndexWriter for CliIndex {
    type Error = CoreError;

    fn request_build(
        &mut self,
        request: IndexBuildRequest,
    ) -> Result<IndexBuildState, Self::Error> {
        let state = IndexBuildState::Running {
            batches_processed: 0,
            documents_seen: 0,
            documents_total: None,
            segments_completed: 0,
            segments_total: None,
        };
        self.states.insert(request.mailbox_id, state.clone());
        if request.replace_existing {
            self.docs.remove(&request.mailbox_id);
            self.segments.remove(&request.mailbox_id);
        }
        Ok(state)
    }

    fn remove_segment(&mut self, segment_id: &str) -> Result<IndexBuildState, Self::Error> {
        for (_, segments) in self.segments.iter_mut() {
            segments.retain(|segment| segment.segment_id != segment_id);
        }
        let id = self
            .states
            .keys()
            .next()
            .copied()
            .unwrap_or_else(MailboxId::new);
        Ok(self
            .states
            .entry(id)
            .or_insert(IndexBuildState::NotRequested)
            .clone())
    }

    fn replace_segment(&mut self, segment: IndexSegment) -> Result<(), Self::Error> {
        let list = self
            .segments
            .entry(segment.mailbox_id)
            .or_insert_with(Vec::new);
        match list.iter_mut().find(|existing| existing.segment_id == segment.segment_id) {
            Some(existing) => *existing = segment,
            None => list.push(segment),
        }
        Ok(())
    }

    fn finish_build(&mut self, state: IndexBuildState) -> Result<(), Self::Error> {
        let mailbox_id = match state {
            IndexBuildState::Completed { mailbox_id, .. } => mailbox_id,
            IndexBuildState::Failed { mailbox_id, .. } => mailbox_id,
            IndexBuildState::Queued => {
                return Err(CoreError::index(Some("global".to_string()), "build not completed"));
            }
            IndexBuildState::Running(_) => {
                return Err(CoreError::index(Some("global".to_string()), "build not completed"));
            }
            IndexBuildState::Unknown => MailboxId::new(),
        };
        self.states.insert(mailbox_id, state);
        Ok(())
    }
}

impl IndexEngine for CliIndex {
    type Error = CoreError;
    type Reader = Self;
    type Writer = Self;

    fn reader(&self) -> &Self::Reader {
        self
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self
    }
}

#[derive(Clone)]
struct CliUiBus {
    executor: CliExecutor,
    context: ExecutionContext,
}

impl CliUiBus {
    fn new(executor: CliExecutor, context: ExecutionContext) -> Self {
        Self { executor, context }
    }

    fn parse_command(&self, command: &UiShellCommand) -> CoreResult<CoreCommand> {
        let shared = SharedCommandOptions {
            filter: Vec::new(),
            output: OutputFormat::Table,
            limit: None,
            sort: None,
            deterministic: self.context.deterministic,
            strict: self.context.strict,
            page_token: None,
        };

        match command.kind {
            UiCommandKind::Info => Ok(CoreCommand::Info(InfoCommand {
                source: PathBuf::from(
                    command
                        .args
                        .first()
                        .ok_or_else(|| CoreError::invalid_input("ui info expects source"))?
                        .to_string(),
                ),
                options: shared,
            })),
            UiCommandKind::Folders => {
                let source = command
                    .args
                    .first()
                    .ok_or_else(|| CoreError::invalid_input("ui folders expects source"))?;
                Ok(CoreCommand::Folders(pst_pst_pst_core::FoldersCommand {
                    source: PathBuf::from(source),
                    folder: command.args.get(1).cloned(),
                    options: shared,
                }))
            }
            UiCommandKind::Messages => {
                let source = command
                    .args
                    .first()
                    .ok_or_else(|| CoreError::invalid_input("ui messages expects source"))?;
                Ok(CoreCommand::Messages(pst_pst_pst_core::MessagesCommand {
                    source: PathBuf::from(source),
                    folder: command.args.get(1).cloned(),
                    options: shared,
                }))
            }
            UiCommandKind::Search => {
                if command.args.len() < 2 {
                    return Err(CoreError::invalid_input("ui search expects `source` and `query`"));
                }
                Ok(CoreCommand::Search(SearchCommand {
                    source: PathBuf::from(command.args[0].as_str()),
                    query: command.args[1..].join(" "),
                    fields: Vec::new(),
                    mode: SearchMode::Auto,
                    index_policy: IndexPolicy::Allow,
                    include_unindexed: true,
                    max_results: None,
                    options: shared,
                }))
            }
            UiCommandKind::Extract => {
                if command.args.len() < 2 {
                    return Err(CoreError::invalid_input(
                        "ui extract expects `source` and `message-id|attachment-id`",
                    ));
                }
                Ok(CoreCommand::Extract(pst_pst_pst_core::ExtractCommand {
                    source: PathBuf::from(command.args[0].as_str()),
                    message_id: command.args.get(1).cloned(),
                    attachment_id: None,
                    out: Some(PathBuf::from("out")),
                    options: shared,
                }))
            }
            UiCommandKind::Export => {
                if command.args.len() < 2 {
                    return Err(CoreError::invalid_input("ui export expects `source` and `out`"));
                }
                Ok(CoreCommand::Export(pst_pst_pst_core::ExportCommand {
                    source: PathBuf::from(command.args[0].as_str()),
                    format: ExportFormat::Jsonl,
                    out: Some(PathBuf::from(command.args[1].as_str())),
                    folder: None,
                    message_ids: command.args[2..].to_vec(),
                    options: shared,
                }))
            }
            UiCommandKind::Validate => {
                Ok(CoreCommand::Validate(ValidateCommand {
                    source: PathBuf::from(
                        command
                            .args
                            .first()
                            .ok_or_else(|| CoreError::invalid_input("ui validate expects source"))?,
                    ),
                    report: None,
                    options: shared,
                }))
            }
            UiCommandKind::Index => {
                let source = command
                    .args
                    .first()
                    .ok_or_else(|| CoreError::invalid_input("ui index expects source"))?;
                Ok(CoreCommand::Index(IndexCommand {
                    source: PathBuf::from(source),
                    db: command.args.get(1).map(PathBuf::from),
                    rebuild: false,
                    options: shared,
                }))
            }
            UiCommandKind::Watch => {
                let dir = command
                    .args
                    .first()
                    .ok_or_else(|| CoreError::invalid_input("ui watch expects dir"))?;
                Ok(CoreCommand::Watch(WatchCommand {
                    dir: PathBuf::from(dir),
                    pattern: command.args.get(1).cloned(),
                    on_changed: command
                        .args
                        .get(2)
                        .cloned()
                        .unwrap_or_else(|| "echo {path}".to_string()),
                    options: shared,
                }))
            }
            UiCommandKind::Help => Err(CoreError::unsupported("help command handled by renderer")),
            UiCommandKind::Quit => Err(CoreError::unsupported("quit command handled by UI runtime")),
            UiCommandKind::Unknown(_) => Err(CoreError::unsupported("unsupported ui command")),
        }
    }
}

impl UiCommandBus for CliUiBus {
    type Error = CoreError;

    fn execute(
        &mut self,
        _state: &UiState,
        command: &UiShellCommand,
    ) -> Result<UiCommandResult, Self::Error> {
        if matches!(command.kind, UiCommandKind::Help) {
            return Ok(UiCommandResult {
                command_id: 0,
                exit: false,
                status: Some("available: info folders messages search extract export validate index watch quit".to_string()),
                payload: Some(UiPayload::Text("help".to_string())),
            });
        }

        if matches!(command.kind, UiCommandKind::Quit) {
            return Ok(UiCommandResult {
                command_id: 0,
                exit: true,
                status: Some("quit".to_string()),
                payload: Some(UiPayload::Text("bye".to_string())),
            });
        }

        let command = self.parse_command(command)?;
        let result = pst_pst_pst_core::execute_command(&self.executor, &command, &self.context)?;
        Ok(UiCommandResult {
            command_id: 0,
            exit: false,
            status: Some("ok".to_string()),
            payload: Some(UiPayload::Core(result.payload)),
        })
    }
}

fn to_core_command(cli: &Cli, global: &GlobalContext) -> CoreResult<CoreCommand> {
    let to_shared = |options: &SharedOptions| options.to_core(global.output);
    let shared = |options: &SharedOptions| options.to_core(global.output);
    Ok(match &cli.command {
        CliCommand::Info { source, options } => CoreCommand::Info(InfoCommand {
            source: source.clone(),
            options: shared(options),
        }),
        CliCommand::Folders {
            source,
            folder,
            options,
        } => CoreCommand::Folders(pst_pst_pst_core::FoldersCommand {
            source: source.clone(),
            folder: folder.clone(),
            options: shared(options),
        }),
        CliCommand::Messages {
            source,
            folder,
            options,
        } => CoreCommand::Messages(MessagesCommand {
            source: source.clone(),
            folder: folder.clone(),
            options: shared(options),
        }),
        CliCommand::Search {
            source,
            query,
            fields,
            mode,
            index_policy,
            include_unindexed,
            max_results,
            options,
        } => CoreCommand::Search(SearchCommand {
            source: source.clone(),
            query: query.clone(),
            fields: fields.clone(),
            mode: mode.clone().into_core(),
            index_policy: index_policy.clone().into_core(),
            include_unindexed: *include_unindexed,
            max_results: *max_results,
            options: shared(options),
        }),
        CliCommand::Extract {
            source,
            message_id,
            attachment_id,
            out,
            options,
        } => CoreCommand::Extract(pst_pst_pst_core::ExtractCommand {
            source: source.clone(),
            message_id: message_id.clone(),
            attachment_id: attachment_id.clone(),
            out: Some(out.clone()),
            options: shared(options),
        }),
        CliCommand::Export {
            source,
            format,
            out,
            folder,
            message_ids,
            options,
        } => CoreCommand::Export(pst_pst_pst_core::ExportCommand {
            source: source.clone(),
            format: format.clone().into_core(),
            out: Some(out.clone()),
            folder: folder.clone(),
            message_ids: message_ids.clone(),
            options: shared(options),
        }),
        CliCommand::Validate {
            source,
            report,
            options,
        } => CoreCommand::Validate(ValidateCommand {
            source: source.clone(),
            report: report.clone(),
            options: shared(options),
        }),
        CliCommand::Index {
            source,
            db,
            rebuild,
            options,
        } => CoreCommand::Index(IndexCommand {
            source: source.clone(),
            db: db.clone(),
            rebuild: *rebuild,
            options: shared(options),
        }),
        CliCommand::Watch {
            dir,
            pattern,
            on_changed,
            options,
        } => CoreCommand::Watch(WatchCommand {
            dir: dir.clone(),
            pattern: pattern.clone(),
            on_changed: on_changed.clone(),
            options: shared(options),
        }),
        CliCommand::Ui { bind, options } => CoreCommand::Ui(CoreUiCommand {
            bind: bind.clone(),
            options: shared(options),
        }),
    })
}

fn main() {
    let cli = Cli::parse();
    let global = match GlobalContext::from_cli(&cli) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("invalid global options: {error}");
            std::process::exit(exit_code(&error));
        }
    };

    let command = match to_core_command(&cli, &global) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("invalid command: {error}");
            std::process::exit(exit_code(&error));
        }
    };

    let executor = CliExecutor::new(&global);
    let context = global.execution_context();
    match pst_pst_pst_core::execute_command(&executor, &command, &context) {
        Ok(result) => {
            if let Err(error) = print_command_result(&result) {
                eprintln!("render failed: {error}");
                std::process::exit(exit_code(&error));
            }
        }
        Err(error) => {
            eprintln!("command failed: {error}");
            std::process::exit(exit_code(&error));
        }
    }
}

fn print_command_result(result: &CommandResult) -> CoreResult<()> {
    match result.output {
        OutputFormat::Json => {
            println!(
                "{}",
                to_string_pretty(&result.payload)
                    .map_err(|error| CoreError::invalid_input(format!("failed to render JSON: {error}")))?
            );
            Ok(())
        }
        OutputFormat::Jsonl | OutputFormat::Ndjson => {
            println!(
                "{}",
                to_string(&result.payload)
                    .map_err(|error| CoreError::invalid_input(format!("failed to render JSONL: {error}")))?
            );
            Ok(())
        }
        OutputFormat::Table => render_table_payload(&result.payload),
    }
}

fn render_table_payload(payload: &CommandPayload) -> CoreResult<()> {
    match payload {
        CommandPayload::Mailbox(mailbox) => {
            println!("mailbox = {}", mailbox.id);
            println!("source = {}", mailbox.source_path.display());
            println!("container = {:?}", mailbox.container_format);
            println!("state = {:?}", mailbox.state);
            println!("folders = {}", mailbox.folder_count);
            println!("messages = {}", mailbox.message_count);
            println!("attachments = {}", mailbox.attachment_count);
            Ok(())
        }
        CommandPayload::Folders(result) => {
            println!("scanned = {}", result.scanned);
            for folder in &result.folders {
                println!(
                    " - {} [{}] messages={}",
                    folder.path, folder.id, folder.message_count
                );
            }
            println!("has_more = {}", result.page.has_more);
            Ok(())
        }
        CommandPayload::Messages(result) => {
            println!("scanned = {}", result.scanned);
            for message in &result.messages {
                let subject = message.subject.as_deref().unwrap_or("<no subject>");
                println!(" - {} {}", message.id, subject);
            }
            println!("has_more = {}", result.page.has_more);
            Ok(())
        }
        CommandPayload::Search(result) => {
            println!(
                "hits = {} returned = {} source_mode = {:?}",
                result.total, result.returned, result.source_mode
            );
            for hit in &result.hits {
                println!(
                    " - {} score = {:?} source = {:?} folder = {}",
                    hit.message_id, hit.score, hit.match_source, hit.folder_id
                );
            }
            Ok(())
        }
        CommandPayload::Export(result) => {
            println!("requested = {}", result.requested);
            println!("exported = {}", result.exported);
            println!("skipped = {}", result.skipped);
            println!("failed = {}", result.failed);
            println!("destination = {}", result.destination.display());
            Ok(())
        }
        CommandPayload::Validation(result) => {
            println!("passed = {}", result.passed);
            println!("scanned_items = {}", result.scanned_items);
            println!("warnings = {}", result.warnings);
            println!("errors = {}", result.errors);
            Ok(())
        }
        CommandPayload::Index(result) => {
            println!("mailbox = {:?}", result.mailbox_id);
            println!("documents = {}", result.documents);
            println!("segments = {}", result.segments);
            println!("policy = {:?}", result.policy);
            println!("mode = {:?}", result.mode);
            Ok(())
        }
        CommandPayload::Watch(result) => {
            println!("watch_dir = {}", result.watched_dir.display());
            println!("matched_files = {}", result.matched_files);
            println!("processed_events = {}", result.processed_events);
            println!("failed = {}", result.failed);
            if let Some(last_error) = &result.last_error {
                println!("last_error = {last_error}");
            }
            Ok(())
        }
        CommandPayload::Ui(result) => {
            println!(
                "session = {} started={} bind={}",
                result.session_id, result.started, result.bind
            );
            Ok(())
        }
    }
}

fn discover_files(base: &Path, pattern: &str) -> CoreResult<HashMap<PathBuf, u64>> {
    if !base.exists() {
        return Err(CoreError::io(
            Some(base.to_path_buf()),
            "watch directory does not exist",
        ));
    }

    let mut candidates = HashSet::new();
    let mut map = HashMap::new();
    let entries = fs::read_dir(base).map_err(|error| {
        CoreError::io(
            Some(base.to_path_buf()),
            format!("failed to enumerate watch directory: {error}"),
        )
    })?;

    for entry in entries {
        let entry = entry
            .map_err(|error| CoreError::io(Some(base.to_path_buf()), format!("failed to read dir entry: {error}")))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file = path.file_name().and_then(|value| value.to_str()).unwrap_or("");
        if !matches_pattern(file, pattern) {
            continue;
        }
        if !candidates.insert(path.clone()) {
            continue;
        }
        let metadata = entry.metadata().map_err(|error| {
            CoreError::io(
                Some(path.clone()),
                format!("failed to read metadata: {error}"),
            )
        })?;
        let modified = metadata.modified().ok().and_then(|value| {
            value
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs())
        });
        map.insert(path, modified.unwrap_or(0));
    }
    Ok(map)
}

fn matches_pattern(file_name: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    if pattern.starts_with("*.") && pattern.matches('*').count() == 1 {
        let extension = pattern.trim_start_matches("*.");
        file_name
            .rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(extension));
        return file_name
            .to_ascii_lowercase()
            .ends_with(&pattern.to_ascii_lowercase().trim_start_matches('*'));
    }
    if pattern.starts_with('*') {
        let suffix = pattern.trim_start_matches('*');
        file_name
            .to_ascii_lowercase()
            .ends_with(&suffix.to_ascii_lowercase())
    } else if pattern.ends_with('*') {
        let prefix = pattern.trim_end_matches('*');
        file_name
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
    } else {
        file_name
            .to_ascii_lowercase()
            .eq_ignore_ascii_case(&pattern.to_ascii_lowercase())
    }
}

fn render_on_changed_template(command: &str, path: &Path) -> String {
    command
        .replace("{path}", &path.display().to_string())
        .replace("{dir}", path.parent().unwrap_or(Path::new("")).display().to_string().as_str())
}

fn run_on_changed(command: &str) -> CoreResult<()> {
    let status = if cfg!(windows) {
        ShellCommand::new("cmd")
            .args(["/C", command])
            .status()
    } else {
        ShellCommand::new("sh")
            .args(["-c", command])
            .status()
    }
    .map_err(|error| CoreError::io(None, format!("failed to run hook: {error}")))?;

    if status.success() {
        Ok(())
    } else {
        Err(CoreError::invalid_input(format!("hook failed with {status}")))
    }
}

fn map_parser_error(error: ParserError) -> CoreError {
    match error {
        ParserError::ProbeIo { path, message } => CoreError::io(Some(path), message),
        ParserError::UnsupportedContainer { path, .. } => CoreError::unsupported(format!(
            "unsupported container for source `{path}`"
        )),
        ParserError::BackendUnavailable { path, backend_name, message } => {
            CoreError::unsupported(format!("backend `{backend_name}` unavailable for `{path}`: {message}"))
        }
        ParserError::BackendFailed { path, backend_name, message } => {
            CoreError::parse(format!("backend `{backend_name}` failed for `{path}`: {message}"))
        }
        ParserError::InvalidConfig { path, message } => {
            CoreError::invalid_input(format!("invalid parser config for `{path}`: {message}"))
        }
        ParserError::BackendExhausted { path, attempts } => {
            CoreError::parse(format!("all parser backends failed for `{path}`: {attempts:?}"))
        }
    }
}

fn map_index_error(error: CoreError) -> CoreError {
    error
}

fn exit_code(error: &CoreError) -> i32 {
    match error {
        CoreError::Io { .. } => 10,
        CoreError::Parse { .. } => 11,
        CoreError::Decode { .. } => 12,
        CoreError::Integrity { .. } => 13,
        CoreError::Index { .. } => 14,
        CoreError::Export { .. } => 15,
        CoreError::Ui { .. } => 16,
        CoreError::Unsupported { .. } => 17,
        CoreError::InvalidInput { .. } => 18,
    }
}

