use serde::{Deserialize, Serialize};

use std::{fmt, path::PathBuf, str::FromStr};

#[derive(Debug, Clone)]
pub enum ConfigError {
    InvalidArg(String),
    InvalidValue(String),
    Validation(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArg(msg) | Self::InvalidValue(msg) | Self::Validation(msg) => {
                write!(f, "{msg}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

pub trait Validate {
    fn validate(&self) -> Result<(), ConfigError>;
}

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

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Table
    }
}

impl FromStr for OutputFormat {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "jsonl" => Ok(Self::Jsonl),
            "ndjson" => Ok(Self::Ndjson),
            _ => Err(ConfigError::InvalidValue(format!(
                "unsupported output format '{value}'"
            ))),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum SearchMode {
    Auto,
    Full,
    Indexed,
    Hybrid,
}

impl SearchMode {
    pub const fn is_textual(self) -> bool {
        matches!(self, Self::Full | Self::Hybrid)
    }
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Auto
    }
}

impl FromStr for SearchMode {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "full" => Ok(Self::Full),
            "indexed" => Ok(Self::Indexed),
            "hybrid" => Ok(Self::Hybrid),
            _ => Err(ConfigError::InvalidValue(format!(
                "unsupported search mode '{value}'"
            ))),
        }
    }
}

impl fmt::Display for SearchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = match self {
            Self::Auto => "auto",
            Self::Full => "full",
            Self::Indexed => "indexed",
            Self::Hybrid => "hybrid",
        };
        f.write_str(v)
    }
}

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

impl FromStr for IndexPolicy {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "allow" => Ok(Self::Allow),
            "require" => Ok(Self::Require),
            "refresh" => Ok(Self::Refresh),
            "build" => Ok(Self::Build),
            _ => Err(ConfigError::InvalidValue(format!(
                "unsupported index policy '{value}'"
            ))),
        }
    }
}

impl fmt::Display for IndexPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = match self {
            Self::Allow => "allow",
            Self::Require => "require",
            Self::Refresh => "refresh",
            Self::Build => "build",
        };
        f.write_str(v)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ExportFormat {
    Eml,
    Mbox,
    Json,
    Jsonl,
}

impl Default for ExportFormat {
    fn default() -> Self {
        Self::Eml
    }
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Eml => "eml",
            Self::Mbox => "mbox",
            Self::Json => "json",
            Self::Jsonl => "jsonl",
        };
        f.write_str(value)
    }
}

impl FromStr for ExportFormat {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "eml" => Ok(Self::Eml),
            "mbox" => Ok(Self::Mbox),
            "json" => Ok(Self::Json),
            "jsonl" => Ok(Self::Jsonl),
            _ => Err(ConfigError::InvalidValue(format!(
                "unsupported export format '{value}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub config: Option<PathBuf>,
    pub jobs: usize,
    pub io_jobs: usize,
    pub cpu_jobs: usize,
    pub single_thread: bool,
    pub yes: bool,
    pub quiet: bool,
    pub no_color: bool,
    pub search_mode: SearchMode,
    pub index_policy: IndexPolicy,
    pub include_unindexed: bool,
    pub index_staleness_threshold_secs: Option<u64>,
    pub strict: bool,
    pub log_level: Option<String>,
    pub log_json: bool,
    pub log_file: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            config: None,
            jobs: 4,
            io_jobs: 2,
            cpu_jobs: 2,
            single_thread: false,
            yes: false,
            quiet: false,
            no_color: false,
            search_mode: SearchMode::default(),
            index_policy: IndexPolicy::default(),
            include_unindexed: false,
            index_staleness_threshold_secs: None,
            strict: false,
            log_level: None,
            log_json: false,
            log_file: None,
        }
    }
}

impl Validate for RuntimeConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.jobs == 0 {
            return Err(ConfigError::Validation("jobs must be greater than zero".into()));
        }
        if self.io_jobs == 0 {
            return Err(ConfigError::Validation(
                "io_jobs must be greater than zero".into(),
            ));
        }
        if self.cpu_jobs == 0 {
            return Err(ConfigError::Validation(
                "cpu_jobs must be greater than zero".into(),
            ));
        }
        if let Some(level) = &self.log_level {
            let allowed = ["error", "warn", "info", "debug", "trace"];
            if !allowed.contains(&level.to_ascii_lowercase().as_str()) {
                return Err(ConfigError::Validation(format!(
                    "log_level must be one of {:?}",
                    allowed
                )));
            }
        }
        Ok(())
    }
}

impl RuntimeConfig {
    pub fn normalize(&self) -> Self {
        if self.single_thread {
            return Self {
                jobs: 1,
                io_jobs: 1,
                cpu_jobs: 1,
                single_thread: true,
                ..self.clone()
            };
        }

        Self {
            jobs: self.jobs.max(1),
            io_jobs: self.io_jobs.max(1),
            cpu_jobs: self.cpu_jobs.max(1),
            single_thread: false,
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub format: OutputFormat,
    pub output_path: Option<PathBuf>,
    pub verbose: bool,
    pub color: bool,
    pub include_header: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            format: OutputFormat::default(),
            output_path: None,
            verbose: false,
            color: false,
            include_header: true,
        }
    }
}

impl Validate for OutputConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if let Some(path) = &self.output_path {
            if path.as_os_str().is_empty() {
                return Err(ConfigError::Validation(
                    "output_path must not be empty".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub query: String,
    pub fields: Vec<String>,
    pub mode: SearchMode,
    pub index_policy: IndexPolicy,
    pub include_unindexed: bool,
    pub max_results: u64,
    pub timeout_secs: u64,
    pub recursive: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            query: String::new(),
            fields: Vec::new(),
            mode: SearchMode::default(),
            index_policy: IndexPolicy::default(),
            include_unindexed: false,
            max_results: 100,
            timeout_secs: 60,
            recursive: false,
        }
    }
}

impl Validate for SearchConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.query.trim().is_empty() {
            return Err(ConfigError::Validation("search query must not be empty".into()));
        }
        if self.max_results == 0 {
            return Err(ConfigError::Validation(
                "max_results must be greater than zero".into(),
            ));
        }
        if self.timeout_secs == 0 {
            return Err(ConfigError::Validation(
                "timeout_secs must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedCommandConfig {
    pub filter: Vec<String>,
    pub output_format: OutputFormat,
    pub limit: Option<u64>,
    pub sort: Option<String>,
    pub deterministic: bool,
    pub strict: bool,
    pub page_token: Option<String>,
}

impl Default for SharedCommandConfig {
    fn default() -> Self {
        Self {
            filter: Vec::new(),
            output_format: OutputFormat::default(),
            limit: None,
            sort: None,
            deterministic: false,
            strict: false,
            page_token: None,
        }
    }
}

impl Validate for SharedCommandConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if let Some(limit) = self.limit {
            if limit == 0 {
                return Err(ConfigError::Validation("limit must be greater than zero".into()));
            }
        }
        if let Some(sort) = &self.sort {
            if sort.trim().is_empty() {
                return Err(ConfigError::Validation("sort must not be empty".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InfoCommand {
    pub file: PathBuf,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FoldersCommand {
    pub file: PathBuf,
    pub folder: Option<String>,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MessagesCommand {
    pub file: PathBuf,
    pub folder: Option<String>,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchCommand {
    pub file: PathBuf,
    pub config: SearchConfig,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtractCommand {
    pub file: PathBuf,
    pub message_id: Option<String>,
    pub attachment_id: Option<String>,
    pub out: Option<PathBuf>,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportCommand {
    pub file: PathBuf,
    pub format: ExportFormat,
    pub out: Option<PathBuf>,
    pub folder: Option<String>,
    pub message_ids: Vec<String>,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ValidateCommand {
    pub file: PathBuf,
    pub report: Option<PathBuf>,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexCommand {
    pub file: PathBuf,
    pub db: Option<PathBuf>,
    pub rebuild: bool,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchCommand {
    pub dir: PathBuf,
    pub pattern: String,
    pub on_changed: String,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiCommand {
    pub bind: String,
    pub shared: SharedCommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandModel {
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

impl Validate for CommandModel {
    fn validate(&self) -> Result<(), ConfigError> {
        match self {
            Self::Info(cmd) => {
                cmd.shared.validate()?;
                validate_path(&cmd.file, "file")?;
                Ok(())
            }
            Self::Folders(cmd) => {
                cmd.shared.validate()?;
                validate_path(&cmd.file, "file")?;
                if let Some(folder) = &cmd.folder {
                    if folder.trim().is_empty() {
                        return Err(ConfigError::Validation("folder must not be empty".into()));
                    }
                }
                Ok(())
            }
            Self::Messages(cmd) => {
                cmd.shared.validate()?;
                validate_path(&cmd.file, "file")?;
                if let Some(folder) = &cmd.folder {
                    if folder.trim().is_empty() {
                        return Err(ConfigError::Validation("folder must not be empty".into()));
                    }
                }
                Ok(())
            }
            Self::Search(cmd) => {
                cmd.shared.validate()?;
                cmd.config.validate()?;
                validate_path(&cmd.file, "file")?;
                Ok(())
            }
            Self::Extract(cmd) => {
                cmd.shared.validate()?;
                validate_path(&cmd.file, "file")?;
                if cmd.message_id.is_none() && cmd.attachment_id.is_none() {
                    return Err(ConfigError::Validation(
                        "extract requires message-id or attachment-id".into(),
                    ));
                }
                if cmd.out.is_none() {
                    return Err(ConfigError::Validation(
                        "extract requires output path".into(),
                    ));
                }
                Ok(())
            }
            Self::Export(cmd) => {
                cmd.shared.validate()?;
                validate_path(&cmd.file, "file")?;
                validate_path_optional(cmd.out.as_ref(), "out")?;
                Ok(())
            }
            Self::Validate(cmd) => {
                cmd.shared.validate()?;
                validate_path(&cmd.file, "file")?;
                validate_path_optional(cmd.report.as_ref(), "report")?;
                Ok(())
            }
            Self::Index(cmd) => {
                cmd.shared.validate()?;
                validate_path(&cmd.file, "file")?;
                validate_path_optional(cmd.db.as_ref(), "db")?;
                Ok(())
            }
            Self::Watch(cmd) => {
                cmd.shared.validate()?;
                validate_path(&cmd.dir, "dir")?;
                if cmd.pattern.trim().is_empty() {
                    return Err(ConfigError::Validation(
                        "watch pattern must not be empty".into(),
                    ));
                }
                if cmd.on_changed.trim().is_empty() {
                    return Err(ConfigError::Validation(
                        "on_changed command must not be empty".into(),
                    ));
                }
                Ok(())
            }
            Self::Ui(cmd) => {
                cmd.shared.validate()?;
                if cmd.bind.trim().is_empty() {
                    return Err(ConfigError::Validation("bind must not be empty".into()));
                }
                Ok(())
            }
        }
    }
}

impl CommandModel {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Info(_) => "info",
            Self::Folders(_) => "folders",
            Self::Messages(_) => "messages",
            Self::Search(_) => "search",
            Self::Extract(_) => "extract",
            Self::Export(_) => "export",
            Self::Validate(_) => "validate",
            Self::Index(_) => "index",
            Self::Watch(_) => "watch",
            Self::Ui(_) => "ui",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutionPlan {
    pub runtime: RuntimeConfig,
    pub output: OutputConfig,
    pub command: CommandModel,
}

impl Default for ExecutionPlan {
    fn default() -> Self {
        Self {
            runtime: RuntimeConfig::default(),
            output: OutputConfig::default(),
            command: CommandModel::Info(InfoCommand {
                file: PathBuf::new(),
                shared: SharedCommandConfig::default(),
            }),
        }
    }
}

impl Validate for ExecutionPlan {
    fn validate(&self) -> Result<(), ConfigError> {
        self.runtime.validate()?;
        self.output.validate()?;
        self.command.validate()?;
        Ok(())
    }
}

fn validate_path(path: &PathBuf, name: &str) -> Result<(), ConfigError> {
    if path.as_os_str().is_empty() {
        Err(ConfigError::Validation(format!("{name} must not be empty")))
    } else {
        Ok(())
    }
}

fn validate_path_optional(path: Option<&PathBuf>, name: &str) -> Result<(), ConfigError> {
    if let Some(value) = path {
        validate_path(value, name)?;
    }
    Ok(())
}
