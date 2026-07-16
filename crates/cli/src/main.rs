#![forbid(unsafe_code)]

use chrono::Utc;
use clap::{Args, Parser, Subcommand, ValueEnum};
use pst_pst_pst_core::{
    AttachmentId, Command as CoreCommand, CommandPayload, CoreError, CoreResult, ContainerFormat, ErrorClass,
    ExecutionContext, ExportFormat as CoreExportFormat, FilterField, Folder, FolderId, FolderListResult, InfoCommand,
    IndexCommand, IndexPolicy, IndexResult, Mailbox, MailboxId, MailboxState, Message, MessageId,
    MessageListResult, MatchSource, OutputFormat, ParseEvent, ParseEventId, ParseStage, PageInfo,
    PaginationToken, RuntimeExecutionConfig, SearchCommand, SearchFilter, SearchHit, SearchMode, Severity,
    SharedCommandOptions, ValidateCommand, ValidationResult, WatchCommand, WatchResult,
    ExportResult as CoreExportResult, ExportCommand, CommandResult, UiCommand as CoreUiCommand, UiResult,
};
use pst_pst_pst_export::{
    progress_channel, ExportConfig, ExportFormat as RuntimeExportFormat, InMemoryCheckpointStore,
    MockExportEngine,
};
use pst_pst_pst_index::{
    FilterField as IndexFilterField, IndexBuildRequest, IndexBuildState, IndexMatch, IndexMatchMetadata,
    IndexQuery, IndexQueryResult, IndexSegment, SearchMode as IndexSearchMode,
};
use pst_pst_pst_parser::{DiscoveryReport, ParserConfig, ParserError, ParserRegistry, ParsedStore};
use pst_pst_pst_ui::{
    TerminalUi, UiCommand as UiShellCommand, UiCommandBus, UiCommandKind, UiCommandResult, UiConfig,
    UiEvent, UiMode, UiOutput, UiPayload, UiRuntime,
};
use serde_json::{to_string, to_string_pretty};
use std::{cmp::Ordering, collections::{HashMap, HashSet}, io::{self, BufRead, Write}, path::{Path, PathBuf}, process::Command as ShellCommand, str::FromStr, sync::{Arc, Mutex}, thread, time::{SystemTime, UNIX_EPOCH}};

#[derive(Debug, Parser)]
#[command(name = "pst-pst-pst")]
#[command(about = "Rust-native mailbox CLI with parser/index/export/search/watch/ui glue")]
struct Cli {
    #[arg(long)]
    jobs: Option<usize>,
    #[arg(long)]
    io_jobs: Option<usize>,
    #[arg(long)]
    cpu_jobs: Option<usize>,
    #[arg(long)]
    single_thread: bool,
    #[arg(long)]
    deterministic: bool,
    #[arg(long)]
    strict: bool,
    #[arg(long, value_enum)]
    container: Option<ContainerArg>,
    #[arg(long, value_enum, default_value_t = OutputArg::Table)]
    output: OutputArg,
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
    fn into_core(self) -> CoreExportFormat {
        match self {
            Self::Eml => CoreExportFormat::Eml,
            Self::Mbox => CoreExportFormat::Mbox,
            Self::Json => CoreExportFormat::Json,
            Self::Jsonl => CoreExportFormat::Jsonl,
            Self::Msg => CoreExportFormat::Msg,
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
    fn into_core(self, global: OutputFormat) -> SharedCommandOptions {
        SharedCommandOptions {
            filter: self.filter,
            output: self.output.map_or(global, OutputArg::into_core),
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
        #[arg(long)]
        bind: String,
        #[command(flatten)]
        options: SharedOptions,
    },
}

#[derive(Debug)]
struct GlobalContext {
    output: OutputFormat,
    runtime: RuntimeExecutionConfig,
    deterministic: bool,
    strict: bool,
    requested_container: ContainerFormat,
}

impl GlobalContext {
    fn from_cli(cli: &Cli) -> CoreResult<Self> {
        let parallelism = thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4);
        let runtime = RuntimeExecutionConfig {
            jobs: cli.jobs.unwrap_or(parallelism),
            io_jobs: cli.io_jobs.unwrap_or(parallelism),
            cpu_jobs: cli.cpu_jobs.unwrap_or(parallelism),
            single_thread: cli.single_thread,
            strict: cli.strict,
            include_unindexed: true,
            index_staleness_threshold: None,
        };
        runtime
            .validate()
            .map_err(|error| CoreError::invalid_input(error.to_string()))?;
        Ok(Self {
            output: cli.output.into_core(),
            runtime: runtime.normalize(),
            deterministic: cli.deterministic,
            strict: cli.strict,
            requested_container: cli.container.clone().unwrap_or(ContainerArg::Auto).into_core(),
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
    parser: Arc<ParserRegistry>,
    index: Arc<Mutex<CliIndex>>,
    export_engine: MockExportEngine,
    checkpoint_store: InMemoryCheckpointStore,
    requested_container: ContainerFormat,
}

impl CliExecutor {
    fn new(context: &GlobalContext) -> Self {
        Self {
            parser: Arc::new(ParserRegistry::new()),
            index: Arc::new(Mutex::new(CliIndex::default())),
            export_engine: MockExportEngine::default(),
            checkpoint_store: InMemoryCheckpointStore::default(),
            requested_container: context.requested_container,
        }
    }

    fn parse_store(
        &self,
        source: &Path,
        options: &SharedCommandOptions,
        context: &ExecutionContext,
    ) -> CoreResult<ParsedStore> {
        let strict = options.strict || context.strict;
        let config = ParserConfig {
            source_path: source.to_path_buf(),
            requested_container: self.requested_container,
            strict,
            allow_fallback: !strict,
            deterministic: options.deterministic || context.deterministic,
            max_bytes: None,
        };
        let discovery = self
            .parser
            .discover(&config)
            .map_err(map_parser_error)?;
        match self.parser.parse(&config) {
            Ok(parsed) => Ok(parsed),
            Err(error) => {
                if strict {
                    Err(map_parser_error(error))
                } else {
                    Ok(self.scaffolded_store(source, &discovery))
                }
            }
        }
    }

    fn scaffolded_store(&self, source: &Path, report: &DiscoveryReport) -> ParsedStore {
        let container = report
            .selected
            .as_ref()
            .map(|candidate| candidate.container)
            .unwrap_or(self.requested_container);
        let mut mailbox = Mailbox::new(source.to_path_buf(), container);
        mailbox.state = MailboxState::Degraded;
        let fallback_event = ParseEvent {
            id: ParseEventId::new(),
            location: None,
            stage: ParseStage::Discovery,
            class: ErrorClass::Parse,
            severity: Severity::Warn,
            message: "using parser fallback result".to_string(),
            details: if report.fallback_reasons.is_empty() {
                None
            } else {
                Some(report.fallback_reasons.join("; "))
            },
            occurs_at: Utc::now(),
        };
        let root = Folder {
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
            folders: vec![root],
            messages: Vec::new(),
            attachments: Vec::new(),
            events: vec![fallback_event],
            discovery: report.clone(),
        }
    }

    fn folder_lookup(&self, folders: &[Folder]) -> HashMap<FolderId, String> {
        folders
            .iter()
            .map(|folder| (folder.id, folder.path.clone()))
            .collect()
    }

    fn resolve_folder<'a>(&self, folders: &'a [Folder], selector: &'a str) -> Option<&'a Folder> {
        if let Ok(folder_id) = selector.parse::<FolderId>() {
            return folders.iter().find(|folder| folder.id == folder_id);
        }
        folders
            .iter()
            .find(|folder| folder.name == selector || folder.path == selector)
    }

    fn parse_id<T>(raw: &str, label: &str) -> CoreResult<T>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        raw.parse::<T>()
            .map_err(|error| CoreError::invalid_input(format!("invalid {label} `{raw}`: {error}")))
    }

    fn page_from_token(&self, options: &SharedCommandOptions, limit_override: Option<u64>) -> CoreResult<PageInfo> {
        let limit = limit_override.or(options.limit).unwrap_or(100).max(1);
        let offset = options
            .page_token
            .as_ref()
            .map(|token| {
                token.parse::<u64>().map_err(|error| {
                    CoreError::invalid_input(format!("invalid page token `{token}`: {error}"))
                })
            })
            .transpose()?
            .unwrap_or(0);
        Ok(PageInfo {
            limit,
            offset,
            has_more: false,
            page_token: options.page_token.clone().map(PaginationToken),
            next_page_token: None,
        })
    }

    fn ensure_index(&self, parsed: &ParsedStore, force: bool, deterministic: bool) -> CoreResult<(u64, u64)> {
        let request = IndexBuildRequest {
            mailbox_id: parsed.mailbox.id,
            deterministic,
            replace_existing: force,
            source_mode: Some(IndexSearchMode::Indexed),
        };
        let docs = self
            .build_index_documents(parsed);
        let mut index = self
            .index
            .lock()
            .map_err(|error| CoreError::io(None, format!("index lock poisoned: {error}")))?;
        index.request_build(request, deterministic, force)?;
        index.upsert(parsed.mailbox.id, docs, force, deterministic)?;
        index.finish(parsed.mailbox.id, deterministic)?;
        Ok(index.document_count(parsed.mailbox.id), index.segment_count(parsed.mailbox.id))
    }

    fn build_index_documents(&self, parsed: &ParsedStore) -> Vec<CliIndexedDocument> {
        let folder_lookup = self.folder_lookup(&parsed.folders);
        parsed
            .messages
            .iter()
            .map(|message| CliIndexedDocument {
                message_id: message.id,
                folder_id: message.folder_id,
                searchable: searchable_text(message, &folder_lookup),
            })
            .collect()
    }

    fn execute_search_indexed(
        &self,
        mailbox_id: MailboxId,
        query_text: &str,
        max_results: Option<u64>,
        deterministic: bool,
        options: &SharedCommandOptions,
        mode: SearchMode,
        include_unindexed: bool,
    ) -> CoreResult<(Vec<SearchHit>, SearchMode, u64, bool)> {
        let page = self.page_from_token(options, max_results)?;
        let mut index_page = page.clone();
        index_page.limit = u64::MAX;
        let text = normalize_query(query_text);
        let index_query = IndexQuery {
            mailbox_id,
            text: if text.is_empty() { None } else { Some(text.clone()) },
            filter: SearchFilter::default(),
            page: index_page,
            mode: IndexSearchMode::Indexed,
            include_unindexed,
            deterministic,
            segment_ids: Vec::new(),
        };
        let result = self
            .index
            .lock()
            .map_err(|error| CoreError::io(None, format!("index lock poisoned: {error}")))?;
        let result = result
            .query(&index_query)
            .map_err(|error| CoreError::invalid_input(format!("index query failed: {error}")))?;
        let mut hits: Vec<SearchHit> = result
            .matches
            .into_iter()
            .map(|hit| SearchHit {
                message_id: hit.message_id,
                folder_id: hit.folder_id,
                score: hit.metadata.score,
                match_source: if mode == SearchMode::Hybrid {
                    MatchSource::Hybrid
                } else {
                    MatchSource::Indexed
                },
                matched_fields: hit
                    .metadata
                    .matched_fields
                    .into_iter()
                    .map(index_field_to_core)
                    .collect(),
                snippet: hit.metadata.snippet,
            })
            .collect();
        if deterministic {
            hits.sort_by(|a, b| {
                let cmp = a.message_id.to_string().cmp(&b.message_id.to_string());
                if cmp == Ordering::Equal {
                    a.folder_id.to_string().cmp(&b.folder_id.to_string())
                } else {
                    cmp
                }
            });
        }
        let total = result.total as usize;
        let offset = page.offset as usize;
        let limit = page.limit as usize;
        let has_more = offset.saturating_add(limit) < total;
        let hits = hits
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        Ok((
            hits,
            if mode == SearchMode::Hybrid {
                SearchMode::Hybrid
            } else {
                SearchMode::Indexed
            },
            result.total,
            has_more,
        ))
    }

    fn execute_search_full(
        &self,
        parsed: &ParsedStore,
        query: &str,
        fields: &[String],
        max_results: Option<u64>,
        deterministic: bool,
        options: &SharedCommandOptions,
    ) -> CoreResult<(Vec<SearchHit>, SearchMode, u64, bool)> {
        let normalized_fields = selected_search_fields(fields);
        let terms = normalize_query_terms(query);
        let mut hits = Vec::new();
        let folder_lookup = self.folder_lookup(&parsed.folders);
        for message in &parsed.messages {
            if terms.is_empty() {
                continue;
            }
            let haystack = fields_searchable_text(message, &folder_lookup);
            let matched = terms.iter().all(|term| haystack.contains(term));
            if !matched {
                continue;
            }
            hits.push(SearchHit {
                message_id: message.id,
                folder_id: message.folder_id,
                score: None,
                match_source: MatchSource::Full,
                matched_fields: normalized_fields
                    .iter()
                    .map(|field| field.to_core())
                    .collect(),
                snippet: Some(
                    snippet_for_terms(&haystack, &terms).unwrap_or_else(|| message.subject.clone().unwrap_or_default()),
                ),
            });
        }
        if deterministic {
            hits.sort_by(|a, b| {
                let cmp = a.message_id.to_string().cmp(&b.message_id.to_string());
                if cmp == Ordering::Equal {
                    a.folder_id.to_string().cmp(&b.folder_id.to_string())
                } else {
                    cmp
                }
            });
        }
        let total = hits.len();
        let page = self.page_from_token(options, max_results)?;
        let has_more = (page.offset as usize).saturating_add(page.limit as usize) < total;
        let offset = page.offset as usize;
        let hits = hits
            .into_iter()
            .skip(offset)
            .take(page.limit as usize)
            .collect::<Vec<_>>();
        Ok((hits, SearchMode::Full, total as u64, has_more))
    }

    fn run_export(
        &self,
        source: &Path,
        options: &SharedCommandOptions,
        context: &ExecutionContext,
        command_source: &Path,
        format: CoreExportFormat,
        out: &Path,
        folder_id: Option<FolderId>,
        message_ids: Vec<MessageId>,
        attachment_ids: Vec<AttachmentId>,
        parsed: &ParsedStore,
        max_messages: Option<u64>,
    ) -> CoreResult<CoreExportResult> {
        let deterministic = context.deterministic || options.deterministic;
        let folder_ids = folder_id.into_iter().collect::<Vec<_>>();
        let mut config = ExportConfig {
            source_path: source.to_path_buf(),
            destination: out.to_path_buf(),
            mailbox_id: Some(parsed.mailbox.id),
            folder_ids,
            message_ids,
            attachment_ids,
            format: to_runtime_format(format),
            deterministic,
            strict: context.strict || options.strict,
            checkpoint_path: Some(out.join(".checkpoint")),
            workers: context.runtime.jobs.max(1),
            max_messages: max_messages.or(Some(parsed.messages.len() as u64)),
        };

        // Use command source path to keep deterministic semantics stable.
        config.source_path = command_source.to_path_buf();
        let (handle, _receiver) = progress_channel(8);
        let manifest = self
            .export_engine
            .execute(&config, None, &handle, &self.checkpoint_store)
            .map_err(|error| CoreError::invalid_input(error.to_string()))?;
        Ok(manifest
            .core_summary
            .unwrap_or_else(|| CoreExportResult {
                mailbox_id: parsed.mailbox.id,
                requested: config.requested_count(),
                exported: 0,
                skipped: 0,
                failed: 0,
                destination: out.to_path_buf(),
                manifest_path: None,
                deterministic,
            }))
    }
}

impl pst_pst_pst_core::CommandExecutor for CliExecutor {
    fn execute_info(&self, command: &InfoCommand, context: &ExecutionContext) -> CoreResult<Mailbox> {
        let parsed = self.parse_store(&command.source, &command.options, context)?;
        Ok(parsed.mailbox)
    }

    fn execute_folders(&self, command: &FoldersCommand, context: &ExecutionContext) -> CoreResult<FolderListResult> {
        let parsed = self.parse_store(&command.source, &command.options, context)?;
        let mut folders = parsed.folders.clone();
        if let Some(folder) = &command.folder {
            if let Some(selected) = self.resolve_folder(&folders, folder) {
                folders = vec![selected.clone()];
            } else {
                folders.clear();
            }
        }
        let page = self.page_from_token(&command.options, None)?;
        let has_more = (page.offset as usize).saturating_add(page.limit as usize) < folders.len();
        let scanned = folders.len() as u64;
        let folders = folders
            .into_iter()
            .skip(page.offset as usize)
            .take(page.limit as usize)
            .collect::<Vec<_>>();
        Ok(FolderListResult {
            mailbox_id: parsed.mailbox.id,
            folders,
            scanned,
            page: PageInfo {
                has_more,
                ..page
            },
        })
    }

    fn execute_messages(
        &self,
        command: &pst_pst_pst_core::MessagesCommand,
        context: &ExecutionContext,
    ) -> CoreResult<MessageListResult> {
        let parsed = self.parse_store(&command.source, &command.options, context)?;
        let mut messages = parsed.messages.clone();
        if let Some(folder) = &command.folder {
            if let Some(selected) = self.resolve_folder(&parsed.folders, folder) {
                let folder_id = selected.id;
                messages.retain(|message| message.folder_id == folder_id);
            } else {
                messages.clear();
            }
        }
        let page = self.page_from_token(&command.options, None)?;
        let has_more = (page.offset as usize).saturating_add(page.limit as usize) < messages.len();
        let scanned = messages.len() as u64;
        let messages = messages
            .into_iter()
            .skip(page.offset as usize)
            .take(page.limit as usize)
            .collect::<Vec<_>>();
        Ok(MessageListResult {
            mailbox_id: parsed.mailbox.id,
            folder_id: command.folder.as_ref().and_then(|folder| {
                self.resolve_folder(&parsed.folders, folder).map(|folder| folder.id)
            }),
            messages,
            scanned,
            page: PageInfo {
                has_more,
                ..page
            },
        })
    }

    fn execute_search(
        &self,
        command: &SearchCommand,
        context: &ExecutionContext,
    ) -> CoreResult<pst_pst_pst_core::SearchResult> {
        let parsed = self.parse_store(&command.source, &command.options, context)?;
        let deterministic = command.options.deterministic || context.deterministic;
        let mode = command.mode;
        let page = self.page_from_token(&command.options, command.max_results)?;
        let index_state = self
            .index
            .lock()
            .map_err(|error| CoreError::io(None, format!("index lock poisoned: {error}")))?;
        let has_index = index_state.has_index(parsed.mailbox.id);

        let use_index = match mode {
            SearchMode::Full => false,
            SearchMode::Auto => {
                has_index || matches!(command.index_policy, IndexPolicy::Build | IndexPolicy::Refresh)
            }
            SearchMode::Indexed | SearchMode::Hybrid => {
                has_index
                    || matches!(command.index_policy, IndexPolicy::Build | IndexPolicy::Refresh)
                    || command.options.strict
                    || context.strict
            }
        };

        if command.index_policy == IndexPolicy::Require && !has_index && matches!(mode, SearchMode::Indexed | SearchMode::Hybrid) {
            return Err(CoreError::invalid_input(
                "index is required but missing; run `index --build` first",
            ));
        }

        if use_index && (!has_index || matches!(command.index_policy, IndexPolicy::Build | IndexPolicy::Refresh)) {
            self.ensure_index(
                &parsed,
                matches!(command.index_policy, IndexPolicy::Build | IndexPolicy::Refresh),
                deterministic,
            )?;
        }

        let (mut hits, mut source_mode, mut total, has_more_from_index) = if use_index {
            self.execute_search_indexed(
                parsed.mailbox.id,
                &command.query,
                command.max_results,
                deterministic,
                &command.options,
                mode,
                command.include_unindexed,
            )?
        } else {
            self.execute_search_full(
                &parsed,
                &command.query,
                &command.fields,
                command.max_results,
                deterministic,
                &command.options,
            )?
        };

        if mode == SearchMode::Hybrid && use_index {
            if command.include_unindexed {
                let all_options = SharedCommandOptions {
                    page_token: None,
                    ..command.options.clone()
                };
                let (index_hits, _, index_total, _) = self.execute_search_indexed(
                    parsed.mailbox.id,
                    &command.query,
                    Some(u64::MAX),
                    deterministic,
                    &all_options,
                    mode,
                    true,
                )?;
                let (full_hits, _, full_total, _) = self.execute_search_full(
                    &parsed,
                    &command.query,
                    &command.fields,
                    Some(u64::MAX),
                    deterministic,
                    &all_options,
                )?;

                let mut merged = index_hits;
                let mut existing: HashSet<MessageId> = merged.iter().map(|hit| hit.message_id).collect();
                for hit in full_hits {
                    if !existing.contains(&hit.message_id) {
                        merged.push(hit);
                    }
                }
                if deterministic {
                    merged.sort_by(|a, b| {
                        let cmp = a.message_id.to_string().cmp(&b.message_id.to_string());
                        if cmp == Ordering::Equal {
                            a.folder_id.to_string().cmp(&b.folder_id.to_string())
                        } else {
                            cmp
                        }
                    });
                }

                total = merged.len() as u64;
                hits = merged
                    .into_iter()
                    .skip(page.offset as usize)
                    .take(page.limit as usize)
                    .collect::<Vec<_>>();

                return Ok(pst_pst_pst_core::SearchResult {
                    mailbox_id: parsed.mailbox.id,
                    hits,
                    total,
                    returned: hits.len(),
                    query: Some(command.query.clone()),
                    source_mode: SearchMode::Hybrid,
                    include_unindexed: command.include_unindexed,
                    deterministic,
                    page: PageInfo {
                        has_more: page.offset.saturating_add(page.limit) < total,
                        ..page
                    },
                });
            }

            source_mode = SearchMode::Hybrid;
        }

        Ok(pst_pst_pst_core::SearchResult {
            mailbox_id: parsed.mailbox.id,
            hits,
            total,
            returned: hits.len(),
            query: Some(command.query.clone()),
            source_mode,
            include_unindexed: command.include_unindexed,
            deterministic,
            page: PageInfo {
                has_more: has_more_from_index,
                ..page
            },
        })
    }

    fn execute_extract(
        &self,
        command: &pst_pst_pst_core::ExtractCommand,
        context: &ExecutionContext,
    ) -> CoreResult<CoreExportResult> {
        let parsed = self.parse_store(&command.source, &command.options, context)?;
        let out = command.out.clone().ok_or_else(|| CoreError::invalid_input("extract requires --out"))?;
        let mut message_ids = Vec::new();
        let mut attachment_ids = Vec::new();
        if let Some(message_id) = &command.message_id {
            message_ids.push(Self::parse_id::<MessageId>(message_id, "message-id")?);
        }
        if let Some(attachment_id) = &command.attachment_id {
            attachment_ids.push(Self::parse_id::<AttachmentId>(attachment_id, "attachment-id")?);
        }
        self.run_export(
            &command.source,
            &command.options,
            context,
            &command.source,
            CoreExportFormat::Jsonl,
            &out,
            None,
            message_ids,
            attachment_ids,
            &parsed,
            Some(1),
        )
    }

    fn execute_export(
        &self,
        command: &ExportCommand,
        context: &ExecutionContext,
    ) -> CoreResult<CoreExportResult> {
        let parsed = self.parse_store(&command.source, &command.options, context)?;
        let out = command.out.clone().ok_or_else(|| CoreError::invalid_input("export requires --out"))?;
        let folder_id = command.folder.as_ref().and_then(|selector| {
            self.resolve_folder(&parsed.folders, selector).map(|folder| folder.id)
        });
        let message_ids = command
            .message_ids
            .iter()
            .map(|raw| Self::parse_id::<MessageId>(raw, "message-id"))
            .collect::<CoreResult<Vec<_>>>()?;
        self.run_export(
            &command.source,
            &command.options,
            context,
            &command.source,
            command.format,
            &out,
            folder_id,
            message_ids,
            Vec::new(),
            &parsed,
            if command.message_ids.is_empty() {
                Some(parsed.messages.len() as u64)
            } else {
                None
            },
        )
    }

    fn execute_validate(
        &self,
        command: &ValidateCommand,
        context: &ExecutionContext,
    ) -> CoreResult<ValidationResult> {
        let parsed = self.parse_store(&command.source, &command.options, context)?;
        let mut warnings = 0u64;
        let mut errors = 0u64;
        let mut scan_events = parsed.events;
        scan_events.extend(parsed.mailbox.diagnostics.clone());
        for event in &scan_events {
            if event.severity == Severity::Warn {
                warnings = warnings.saturating_add(1);
            } else if event.severity == Severity::Error || event.severity == Severity::Fatal {
                errors = errors.saturating_add(1);
            }
        }
        let passed = errors == 0;
        let scanned_items = (parsed.folders.len() + parsed.messages.len() + parsed.attachments.len()) as u64;
        if let Some(report_path) = command.report {
            let serialized = to_string_pretty(&scan_events)
                .map_err(|error| CoreError::invalid_input(format!("validate report failed: {error}")))?;
            std::fs::write(report_path, serialized)
                .map_err(|error| CoreError::io(None, format!("write validate report failed: {error}")))?;
        }
        Ok(ValidationResult {
            mailbox_id: parsed.mailbox.id,
            passed,
            scanned_items,
            warnings,
            errors,
            events: scan_events,
        })
    }

    fn execute_index(
        &self,
        command: &IndexCommand,
        context: &ExecutionContext,
    ) -> CoreResult<IndexResult> {
        let parsed = self.parse_store(&command.source, &command.options, context)?;
        let deterministic = command.options.deterministic || context.deterministic;
        let has_index = self
            .index
            .lock()
            .map_err(|error| CoreError::io(None, format!("index lock poisoned: {error}")))?
            .has_index(parsed.mailbox.id);
        if !has_index || command.rebuild {
            self.ensure_index(&parsed, command.rebuild || !has_index, deterministic)?;
        }
        let (documents, segments) = self
            .index
            .lock()
            .map_err(|error| CoreError::io(None, format!("index lock poisoned: {error}")))?
            .document_and_segment_counts(parsed.mailbox.id);
        let now = Some(Utc::now());
        Ok(IndexResult {
            mailbox_id: Some(parsed.mailbox.id),
            db_path: command.db.clone(),
            mode: SearchMode::Indexed,
            policy: IndexPolicy::Allow,
            deterministic,
            documents,
            segments,
            started_at: now,
            completed_at: now,
        })
    }

    fn execute_watch(
        &self,
        command: &WatchCommand,
        _context: &ExecutionContext,
    ) -> CoreResult<WatchResult> {
        let pattern = command.pattern.clone().unwrap_or_else(|| "*".to_string());
        let discovered = discover_files(&command.dir, &pattern)?;
        let mut processed = 0u64;
        let mut failed = 0u64;
        let mut last_error = None;
        for path in discovered.keys() {
            let rendered = render_on_changed_template(&command.on_changed, path);
            if let Err(error) = run_on_changed(&rendered) {
                failed = failed.saturating_add(1);
                last_error = Some(error.to_string());
            } else {
                processed = processed.saturating_add(1);
            }
        }
        Ok(WatchResult {
            watched_dir: command.dir.clone(),
            matched_files: discovered.len() as u64,
            processed_events: processed,
            failed,
            last_error,
        })
    }

    fn execute_ui(
        &self,
        command: &CoreUiCommand,
        _context: &ExecutionContext,
    ) -> CoreResult<UiResult> {
        let mut ui = TerminalUi::new(
            CliUiBus::new(self.clone(), command.bind.clone()),
            UiConfig {
                mode: UiMode::Terminal,
                output: UiOutput::Text,
                deterministic: true,
                max_history: 128,
            },
        );
        let start = now_millis();
        println!("pst-pst-pst ui session bind={}", command.bind);
        println!("commands: info folders messages search extract export validate index watch quit");
        let stdin = io::stdin();
        let mut input = String::new();
        let mut session_handle = io::stdout();
        loop {
            session_handle
                .write_all(b"pst> ")
                .map_err(|error| CoreError::io(None, format!("write prompt failed: {error}")))?;
            session_handle.flush()?;
            input.clear();
            let read = stdin
                .lock()
                .read_line(&mut input)
                .map_err(|error| CoreError::io(None, format!("read ui line failed: {error}")))?;
            if read == 0 {
                break;
            }
            if input.trim().is_empty() {
                continue;
            }
            let (_events, lines) = ui.submit(input.trim_end());
            for line in lines {
                println!("{line}");
            }
            if _events.iter().any(|event| matches!(event, UiEvent::Exit { .. })) {
                break;
            }
        }
        Ok(UiResult {
            session_id: format!("ui-{start}"),
            bind: command.bind.clone(),
            destination: None,
            started: true,
            deterministic: true,
        })
    }
}

#[derive(Default)]
struct CliIndex {
    mailboxes: HashMap<MailboxId, CliMailboxIndex>,
}

#[derive(Clone)]
struct CliMailboxIndex {
    generation: u64,
    deterministic: bool,
    documents: Vec<CliIndexedDocument>,
    segments: Vec<IndexSegment>,
    state: IndexBuildState,
}

impl Default for CliMailboxIndex {
    fn default() -> Self {
        Self {
            generation: 0,
            deterministic: false,
            documents: Vec::new(),
            segments: Vec::new(),
            state: IndexBuildState::Unknown,
        }
    }
}

impl CliMailboxIndex {
    fn complete_count(&self) -> bool {
        matches!(self.state, IndexBuildState::Completed { .. })
    }
}

#[derive(Clone)]
struct CliIndexedDocument {
    message_id: MessageId,
    folder_id: FolderId,
    searchable: String,
}

impl CliIndex {
    fn has_index(&self, mailbox_id: MailboxId) -> bool {
        self.mailboxes
            .get(&mailbox_id)
            .is_some_and(CliMailboxIndex::complete_count)
    }

    fn request_build(
        &mut self,
        request: IndexBuildRequest,
        _deterministic: bool,
        force: bool,
    ) -> CoreResult<()> {
        let entry = self
            .mailboxes
            .entry(request.mailbox_id)
            .or_insert_with(CliMailboxIndex::default);
        if force {
            entry.documents.clear();
            entry.segments.clear();
        }
        entry.state = IndexBuildState::Queued;
        Ok(())
    }

    fn upsert(
        &mut self,
        mailbox_id: MailboxId,
        documents: Vec<CliIndexedDocument>,
        force: bool,
        deterministic: bool,
    ) -> CoreResult<()> {
        let entry = self
            .mailboxes
            .get_mut(&mailbox_id)
            .ok_or_else(|| CoreError::invalid_input("missing index build request"))?;
        if force {
            entry.documents.clear();
            entry.segments.clear();
        }
        entry.generation = entry.generation.saturating_add(1);
        entry.deterministic = deterministic;
        entry.documents = documents;
        entry.state = IndexBuildState::Running(pst_pst_pst_index::IndexBuildProgress {
            batches_processed: 1,
            documents_seen: entry.documents.len() as u64,
            documents_total: Some(entry.documents.len() as u64),
            segments_completed: 0,
            segments_total: Some(1),
        });
        let segment = IndexSegment {
            segment_id: format!("seg-{}-{}", mailbox_id, entry.generation),
            mailbox_id,
            generation: entry.generation,
            document_count: entry.documents.len() as u64,
            checksum: None,
            deterministic,
        };
        entry.segments.push(segment);
        entry.state = IndexBuildState::Running(pst_pst_pst_index::IndexBuildProgress {
            batches_processed: 1,
            documents_seen: entry.documents.len() as u64,
            documents_total: Some(entry.documents.len() as u64),
            segments_completed: 1,
            segments_total: Some(1),
        });
        Ok(())
    }

    fn finish(&mut self, mailbox_id: MailboxId, deterministic: bool) -> CoreResult<()> {
        let entry = self
            .mailboxes
            .get_mut(&mailbox_id)
            .ok_or_else(|| CoreError::invalid_input("missing index state"))?;
        let total_documents = entry.documents.len() as u64;
        let total_segments = entry.segments.len() as u64;
        entry.state = IndexBuildState::Completed {
            mailbox_id,
            deterministic,
            total_documents,
            total_segments,
        };
        Ok(())
    }

    fn query(&self, request: &IndexQuery) -> CoreResult<IndexQueryResult> {
        let entry = self
            .mailboxes
            .get(&request.mailbox_id)
            .ok_or_else(|| CoreError::invalid_input("index missing"))?;
        if !entry.complete_count() {
            return Err(CoreError::invalid_input("index not ready"));
        }

        let termset = normalize_query_terms(request.text.as_deref().unwrap_or(""));
        let mut indexed_matches = Vec::new();
        for doc in &entry.documents {
            if termset.iter().all(|term| doc.searchable.contains(term)) {
                indexed_matches.push((doc.message_id, doc.folder_id));
            }
        }

        if request.deterministic {
            indexed_matches.sort_by(|(left, _), (right, _)| left.to_string().cmp(&right.to_string()));
        }

        let offset = request.page.offset as usize;
        let limit = request.page.limit as usize;
        let total = indexed_matches.len() as u64;
        let matches = indexed_matches
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(message_id, folder_id)| {
                IndexMatch {
                    message_id,
                    folder_id,
                    metadata: IndexMatchMetadata {
                        match_source: MatchSource::Indexed,
                        score: None,
                        matched_fields: vec![IndexFilterField::Body],
                        snippet: None,
                    },
                }
            })
            .collect::<Vec<_>>();
        let segments = entry
            .segments
            .iter()
            .map(|segment| IndexedSegmentResult {
                segment_id: segment.segment_id.clone(),
                matches: vec![],
            })
            .collect::<Vec<_>>();
        Ok(IndexQueryResult {
            mailbox_id: request.mailbox_id,
            query: request.clone(),
            matches,
            segments,
            total,
            returned: matches.len(),
            deterministic: request.deterministic,
        })
    }

    fn document_and_segment_counts(&self, mailbox_id: MailboxId) -> (u64, u64) {
        self.mailboxes
            .get(&mailbox_id)
            .map(|entry| (entry.documents.len() as u64, entry.segments.len() as u64))
            .unwrap_or((0, 0))
    }
}

fn normalize_query(query: &str) -> String {
    query.to_ascii_lowercase()
}

fn normalize_query_terms(query: &str) -> Vec<String> {
    normalize_query(query)
        .split_whitespace()
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
}

fn selected_search_fields(raw: &[String]) -> Vec<SearchFieldSpec> {
    let normalized = if raw.is_empty() {
        vec!["subject".to_string(), "sender".to_string(), "body".to_string(), "folder".to_string()]
    } else {
        raw.iter().map(|value| value.to_ascii_lowercase()).collect()
    };
    normalized.into_iter().map(SearchFieldSpec::from).collect()
}

#[derive(Clone)]
struct SearchFieldSpec {
    name: String,
}

impl From<String> for SearchFieldSpec {
    fn from(name: String) -> Self {
        Self { name }
    }
}

impl SearchFieldSpec {
    fn to_core(&self) -> FilterField {
        match self.name.as_str() {
            "subject" => FilterField::Subject,
            "sender" => FilterField::Sender,
            "recipient" => FilterField::Recipient,
            "folder" => FilterField::Folder,
            "has_attachment" | "has-attachment" => FilterField::HasAttachment,
            "size" => FilterField::Size,
            "id" => FilterField::Id,
            "sentat" | "sent_at" => FilterField::SentAt,
            "receivedat" | "received_at" => FilterField::ReceivedAt,
            "modifiedat" | "modified_at" => FilterField::ModifiedAt,
            "messageclass" | "message_class" => FilterField::MessageClass,
            _ => FilterField::Raw(self.name.clone()),
        }
    }

    fn matches(&self, message: &Message, folder_lookup: &HashMap<FolderId, String>) -> String {
        match self.name.as_str() {
            "subject" => message.subject.clone().unwrap_or_default(),
            "sender" => message
                .sender
                .as_ref()
                .and_then(|sender| sender.address.clone())
                .unwrap_or_default(),
            "recipient" => message
                .recipients
                .iter()
                .filter_map(|recipient| recipient.address.clone())
                .collect::<Vec<_>>()
                .join(" "),
            "folder" => folder_lookup
                .get(&message.folder_id)
                .cloned()
                .unwrap_or_default(),
            "body" => message.body.as_ref().and_then(|body| body.content_ref.clone()).unwrap_or_default(),
            "has_attachment" => message.has_attachments.to_string(),
            "size" => message.size.to_string(),
            _ => String::new(),
        }
    }
}

fn searchable_text(message: &Message, folder_lookup: &HashMap<FolderId, String>) -> String {
    let mut text = Vec::new();
    if let Some(subject) = &message.subject {
        text.push(subject.to_lowercase());
    }
    if let Some(sender) = &message.sender {
        if let Some(address) = &sender.address {
            text.push(address.to_ascii_lowercase());
        }
        if let Some(name) = &sender.display_name {
            text.push(name.to_ascii_lowercase());
        }
    }
    for recipient in &message.recipients {
        if let Some(address) = &recipient.address {
            text.push(address.to_ascii_lowercase());
        }
        if let Some(name) = &recipient.display_name {
            text.push(name.to_ascii_lowercase());
        }
    }
    if let Some(body) = &message.body {
        if let Some(content) = &body.content_ref {
            text.push(content.to_ascii_lowercase());
        }
    }
    if let Some(path) = folder_lookup.get(&message.folder_id) {
        text.push(path.to_ascii_lowercase());
    }
    text.join(" ")
}

fn fields_searchable_text(message: &Message, folder_lookup: &HashMap<FolderId, String>) -> String {
    let mut text = Vec::new();
    if let Some(subject) = &message.subject {
        text.push(subject.to_ascii_lowercase());
    }
    if let Some(sender) = &message.sender {
        if let Some(address) = &sender.address {
            text.push(address.to_ascii_lowercase());
        }
        if let Some(name) = &sender.display_name {
            text.push(name.to_ascii_lowercase());
        }
    }
    if let Some(body) = &message.body {
        if let Some(content) = &body.content_ref {
            text.push(content.to_ascii_lowercase());
        }
    }
    if let Some(path) = folder_lookup.get(&message.folder_id) {
        text.push(path.to_ascii_lowercase());
    }
    text.join(" ")
}

fn snippet_for_terms(haystack: &str, terms: &[String]) -> Option<String> {
    for term in terms {
        if let Some(index) = haystack.find(term) {
            let start = index.saturating_sub(40);
            let end = (index + term.len() + 40).min(haystack.len());
            return Some(haystack[start..end].to_string());
        }
    }
    None
}

fn index_field_to_core(field: IndexFilterField) -> FilterField {
    match field {
        IndexFilterField::Subject => FilterField::Subject,
        IndexFilterField::Sender => FilterField::Sender,
        IndexFilterField::Recipient => FilterField::Recipient,
        IndexFilterField::Folder => FilterField::Folder,
        IndexFilterField::Body => FilterField::Body,
        IndexFilterField::HasAttachment => FilterField::HasAttachment,
        IndexFilterField::Size => FilterField::Size,
        IndexFilterField::Id => FilterField::Id,
        IndexFilterField::SentAt => FilterField::SentAt,
        IndexFilterField::ReceivedAt => FilterField::ReceivedAt,
        IndexFilterField::ModifiedAt => FilterField::ModifiedAt,
        IndexFilterField::MessageClass => FilterField::MessageClass,
        IndexFilterField::Raw(value) => FilterField::Raw(value),
    }
}

fn to_core_command(cli: &Cli, context: &GlobalContext) -> CoreResult<CoreCommand> {
    let shared = |options: &SharedOptions| -> SharedCommandOptions { options.into_core(context.output) };
    Ok(match &cli.command {
        CliCommand::Info { source, options } => {
            CoreCommand::Info(InfoCommand {
                source: source.clone(),
                options: shared(options),
            })
        }
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
        } => CoreCommand::Messages(pst_pst_pst_core::MessagesCommand {
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
        CliCommand::Export {
            source,
            format,
            out,
            folder,
            message_ids,
            options,
        } => CoreCommand::Export(ExportCommand {
            source: source.clone(),
            format: format.clone().into_core(),
            out: Some(out.clone()),
            folder: folder.clone(),
            message_ids: message_ids.clone(),
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
            options: SharedCommandOptions {
                strict: false,
                ..shared(options)
            },
        }),
        CliCommand::Ui { bind, options } => CoreCommand::Ui(CoreUiCommand {
            bind: bind.clone(),
            options: shared(options),
        }),
    })
}

fn to_runtime_format(format: CoreExportFormat) -> RuntimeExportFormat {
    match format {
        CoreExportFormat::Eml => RuntimeExportFormat::Eml,
        CoreExportFormat::Mbox => RuntimeExportFormat::Mbox,
        CoreExportFormat::Json => RuntimeExportFormat::Json,
        CoreExportFormat::Jsonl => RuntimeExportFormat::Jsonl,
        CoreExportFormat::Msg => RuntimeExportFormat::Binary,
    }
}

fn to_core_error(message: impl Into<String>) -> CoreError {
    CoreError::invalid_input(message.into())
}

fn map_parser_error(error: ParserError) -> CoreError {
    match error {
        ParserError::ProbeIo { path, message } => CoreError::Io {
            path: Some(path),
            message: format!("parser I/O while probing source: {message}"),
            details: Some("failed to read source metadata or probe signature".to_string()),
        },
        ParserError::InvalidConfig { path, message } => {
            CoreError::invalid_input(format!("invalid parser config `{path:?}`: {message}"))
        }
        ParserError::UnsupportedContainer { path, requested } => CoreError::unsupported(format!(
            "unsupported container `{path:?}` for `{requested:?}`",
        )),
        ParserError::BackendUnavailable {
            path,
            backend_name,
            message,
        } => CoreError::Parse {
            location: None,
            message: format!("backend `{backend_name}` unavailable for `{path:?}`"),
            details: Some(message),
        },
        ParserError::BackendFailed {
            path,
            backend_name,
            message,
        } => CoreError::Parse {
            location: None,
            message: format!("backend `{backend_name}` failed for `{path:?}`"),
            details: Some(message),
        },
        ParserError::BackendExhausted { path, attempts } => {
            let details = attempts
                .into_iter()
                .map(|attempt| format!(
                    "{} ({:?}): {}",
                    attempt.backend_name, attempt.source, attempt.error
                ))
                .collect::<Vec<_>>()
                .join("; ");
            CoreError::Parse {
                location: None,
                message: format!("all parser backends exhausted for `{path:?}`"),
                details: Some(details),
            }
        }
    }
}

fn print_command_result(result: &CommandResult) -> CoreResult<()> {
    match result.output {
        OutputFormat::Json => {
            let text = to_string_pretty(&result.payload)
                .map_err(|error| CoreError::invalid_input(format!("failed to render json output: {error}")))?;
            println!("{text}");
        }
        OutputFormat::Jsonl | OutputFormat::Ndjson => {
            let text = to_string(&result.payload)
                .map_err(|error| CoreError::invalid_input(format!("failed to render jsonl output: {error}")))?;
            println!("{text}");
        }
        OutputFormat::Table => render_table_payload(&result.payload)?,
    }
    Ok(())
}

fn render_table_payload(payload: &CommandPayload) -> CoreResult<()> {
    match payload {
        CommandPayload::Mailbox(mailbox) => {
            println!("mailbox-id: {}", mailbox.id);
            println!("source: {}", mailbox.source_path.display());
            println!("container: {:?}", mailbox.container_format);
            println!("state: {:?}", mailbox.state);
            println!("folders: {}", mailbox.folder_count);
            println!("messages: {}", mailbox.message_count);
            println!("attachments: {}", mailbox.attachment_count);
        }
        CommandPayload::Folders(result) => {
            println!("mailbox: {}", result.mailbox_id);
            println!("folders: {}", result.scanned);
            for folder in &result.folders {
                println!("{} {}", folder.path, folder.id);
            }
            println!("has_more: {}", result.page.has_more);
        }
        CommandPayload::Messages(result) => {
            println!("mailbox: {}", result.mailbox_id);
            println!("messages: {}", result.scanned);
            if let Some(folder_id) = result.folder_id {
                println!("folder: {}", folder_id);
            }
            for message in &result.messages {
                println!("{} {}", message.id, message.subject.clone().unwrap_or_default());
            }
            println!("has_more: {}", result.page.has_more);
        }
        CommandPayload::Search(result) => {
            println!(
                "mailbox={} mode={:?} total={} returned={}",
                result.mailbox_id,
                result.source_mode,
                result.total,
                result.hits.len()
            );
            for hit in &result.hits {
                println!("{} {:?} {:?}", hit.message_id, hit.folder_id, hit.match_source);
            }
            println!("returned: {}", result.returned);
            println!("has_more: {}", result.page.has_more);
        }
        CommandPayload::Export(result) => {
            println!("requested={}", result.requested);
            println!("exported={}", result.exported);
            println!("skipped={}", result.skipped);
            println!("failed={}", result.failed);
            println!("destination={}", result.destination.display());
        }
        CommandPayload::Validation(result) => {
            println!("mailbox={} passed={}", result.mailbox_id, result.passed);
            println!("scanned_items={}", result.scanned_items);
            println!("warnings={}", result.warnings);
            println!("errors={}", result.errors);
        }
        CommandPayload::Index(result) => {
            println!(
                "mailbox={:?} documents={} segments={} deterministic={}",
                result.mailbox_id, result.documents, result.segments, result.deterministic
            );
            println!("policy={:?} mode={:?}", result.policy, result.mode);
        }
        CommandPayload::Watch(result) => {
            println!("dir={}", result.watched_dir.display());
            println!("matched_files={}", result.matched_files);
            println!("processed_events={}", result.processed_events);
            println!("failed={}", result.failed);
            if let Some(error) = &result.last_error {
                println!("last_error={error}");
            }
        }
        CommandPayload::Ui(result) => {
            println!("session={} bind={}", result.session_id, result.bind);
            println!("started={}", result.started);
        }
    }
    Ok(())
}

struct CliUiBus {
    executor: CliExecutor,
    bind_source: String,
}

impl CliUiBus {
    fn new(executor: CliExecutor, bind_source: String) -> Self {
        Self {
            executor,
            bind_source,
        }
    }
}

impl UiCommandBus for CliUiBus {
    type Error = CoreError;

    fn execute(
        &mut self,
        _state: &pst_pst_pst_ui::UiState,
        command: &UiShellCommand,
    ) -> Result<UiCommandResult, Self::Error> {
        if let UiCommandKind::Quit = command.kind {
            return Ok(UiCommandResult {
                command_id: 0,
                exit: true,
                status: Some("bye".to_string()),
                payload: Some(UiPayload::Text("bye".to_string())),
            });
        }
        if matches!(command.kind, UiCommandKind::Help) {
            return Ok(UiCommandResult {
                command_id: 0,
                exit: false,
                status: Some("help".to_string()),
                payload: Some(UiPayload::Text(
                    "commands: info folders messages search extract export validate index watch quit".to_string(),
                )),
            });
        }

        let core = map_ui_to_core(command, &self.bind_source)?;
        let context = ExecutionContext {
            runtime: RuntimeExecutionConfig {
                jobs: 1,
                io_jobs: 1,
                cpu_jobs: 1,
                single_thread: true,
                strict: false,
                include_unindexed: true,
                index_staleness_threshold: None,
            },
            output: OutputFormat::Table,
            deterministic: true,
            strict: false,
        };
        let result = pst_pst_pst_core::execute_command(&self.executor, &core, &context)?;
        Ok(UiCommandResult {
            command_id: 0,
            exit: false,
            status: Some("ok".to_string()),
            payload: Some(UiPayload::Core(result.payload)),
        })
    }
}

fn map_ui_to_core(command: &UiShellCommand, bind_source: &str) -> CoreResult<CoreCommand> {
    let shared = SharedCommandOptions {
        filter: Vec::new(),
        output: OutputFormat::Table,
        limit: None,
        sort: None,
        deterministic: true,
        strict: false,
        page_token: None,
    };

    let args = &command.args;
    let with_base_source = |selector: Option<&String>| -> PathBuf {
        selector
            .map(PathBuf::from)
            .filter(|path| path.as_os_str().is_empty() == false)
            .unwrap_or_else(|| PathBuf::from(bind_source))
    };

    match command.kind {
        UiCommandKind::Info => Ok(CoreCommand::Info(InfoCommand {
            source: args.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(bind_source)),
            options: shared.clone(),
        })),
        UiCommandKind::Folders => {
            let source = with_base_source(args.first());
            let folder = if args.len() > 1 { Some(args[1].clone()) } else { None };
            Ok(CoreCommand::Folders(pst_pst_pst_core::FoldersCommand {
                source,
                folder,
                options: shared,
            }))
        }
        UiCommandKind::Messages => {
            let source = with_base_source(args.first());
            let folder = if args.len() > 1 { Some(args[1].clone()) } else { None };
            Ok(CoreCommand::Messages(pst_pst_pst_core::MessagesCommand {
                source,
                folder,
                options: shared,
            }))
        }
        UiCommandKind::Search => {
            if args.is_empty() {
                return Err(to_core_error("search expects a query"));
            }
            let source = with_base_source(args.first());
            let query_offset = if Path::new(&source).exists() { 1 } else { 0 };
            let query = args[query_offset..].join(" ");
            if query.trim().is_empty() {
                return Err(to_core_error("search expects a query"));
            }
            Ok(CoreCommand::Search(SearchCommand {
                source,
                query,
                fields: Vec::new(),
                mode: SearchMode::Auto,
                index_policy: IndexPolicy::Allow,
                include_unindexed: true,
                max_results: None,
                options: shared,
            }))
        }
        UiCommandKind::Extract => {
            if args.is_empty() {
                return Err(to_core_error("extract expects a message id or attachment id"));
            }
            let (source, first, second) = if args.len() == 1 {
                (PathBuf::from(bind_source), Some(&args[0]), None)
            } else {
                (
                    PathBuf::from(&args[0]),
                    args.get(1),
                    args.get(2),
                )
            };
            Ok(CoreCommand::Extract(pst_pst_pst_core::ExtractCommand {
                source,
                message_id: first.map(|value| value.clone()),
                attachment_id: second.cloned(),
                out: Some(PathBuf::from(".")),
                options: shared,
            }))
        }
        UiCommandKind::Export => {
            if args.len() < 2 {
                return Err(to_core_error("export expects: <source> <out> [message-id...]"));
            }
            Ok(CoreCommand::Export(ExportCommand {
                source: PathBuf::from(&args[0]),
                format: CoreExportFormat::Jsonl,
                out: Some(PathBuf::from(&args[1])),
                folder: None,
                message_ids: args[2..].to_vec(),
                options: shared,
            }))
        }
        UiCommandKind::Validate => {
            let source = with_base_source(args.first());
            Ok(CoreCommand::Validate(ValidateCommand {
                source,
                report: None,
                options: shared,
            }))
        }
        UiCommandKind::Index => {
            let source = with_base_source(args.first());
            Ok(CoreCommand::Index(IndexCommand {
                source,
                db: None,
                rebuild: false,
                options: shared,
            }))
        }
        UiCommandKind::Watch => {
            let dir = with_base_source(args.first());
            Ok(CoreCommand::Watch(WatchCommand {
                dir,
                pattern: None,
                on_changed: args.get(1).cloned().unwrap_or_else(|| "echo {path}".to_string()),
                options: shared,
            }))
        }
        UiCommandKind::Unknown(_) => Err(to_core_error("unknown UI command")),
        _ => Err(to_core_error("unsupported UI command")),
    }
}

fn discover_files(base: &Path, pattern: &str) -> CoreResult<HashMap<PathBuf, u64>> {
    if !base.is_dir() {
        return Err(CoreError::io(
            Some(base.to_path_buf()),
            "watch path must be a directory",
        ));
    }
    let mut discovered = HashMap::new();
    for entry in std::fs::read_dir(base)
        .map_err(|error| CoreError::io(Some(base.to_path_buf()), format!("read dir failed: {error}")))? {
        let entry = entry.map_err(|error| CoreError::io(Some(base.to_path_buf()), format!("dir entry failed: {error}")))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches_pattern(&file_name, pattern) {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        discovered.insert(path, mtime);
    }
    Ok(discovered)
}

fn matches_pattern(file_name: &str, pattern: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    if pattern == "*" || pattern.is_empty() {
        return true;
    }
    if let Some(ext) = pattern.strip_prefix("*.") {
        let ext = format!(".{ext}");
        return file_name.ends_with(&ext);
    }
    if pattern.ends_with('*') {
        return file_name.starts_with(&pattern.trim_end_matches('*'));
    }
    if pattern.starts_with('*') {
        return file_name.ends_with(&pattern.trim_start_matches('*'));
    }
    file_name == pattern
}

fn render_on_changed_template(command: &str, path: &Path) -> String {
    command
        .replace("{path}", path.display().to_string().as_str())
        .replace("{dir}", path.parent().unwrap_or(Path::new(".")).display().to_string().as_str())
}

fn run_on_changed(raw_command: &str) -> CoreResult<()> {
    let status = if cfg!(windows) {
        ShellCommand::new("cmd")
            .args(["/C", raw_command])
            .status()
            .map_err(|error| CoreError::io(None, format!("watch command failed: {error}")))?
    } else {
        ShellCommand::new("sh")
            .args(["-c", raw_command])
            .status()
            .map_err(|error| CoreError::io(None, format!("watch command failed: {error}")))?
    };
    if status.success() {
        Ok(())
    } else {
        Err(CoreError::invalid_input(format!(
            "watch command failed with status {status}"
        )))
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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

fn main() {
    let cli = Cli::parse();
    let context = match GlobalContext::from_cli(&cli) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("invalid args: {error}");
            std::process::exit(exit_code(&error));
        }
    };

    let command = match to_core_command(&cli, &context) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("invalid command: {error}");
            std::process::exit(exit_code(&error));
        }
    };

    if let Err(error) = command.validate() {
        eprintln!("invalid command input: {error}");
        std::process::exit(exit_code(&error));
    }

    let executor = CliExecutor::new(&context);
    let execution = context.execution_context();
    match pst_pst_pst_core::execute_command(&executor, &command, &execution) {
        Ok(result) => {
            if let Err(error) = print_command_result(&result) {
                eprintln!("rendering failed: {error}");
                std::process::exit(exit_code(&error));
            }
        }
        Err(error) => {
            eprintln!("command failed: {error}");
            std::process::exit(exit_code(&error));
        }
    }
}
