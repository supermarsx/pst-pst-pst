//! Parser crate scaffolding for container discovery and pluggable backends.
//!
//! This module intentionally contains no FFI/native dependencies and uses only
//! Rust-native parsing plumbing and discovery logic.
#![forbid(unsafe_code)]

use std::{fs::File, io, io::Read, path::{Path, PathBuf}};

use pst_pst_pst_core::{
    Attachment, ContainerFormat, ErrorClass, Mailbox, MailboxState, Message, ParseEvent, ParseEventId,
    ParseStage, Severity, Folder,
};
use chrono::Utc;
use thiserror::Error;

pub type ParserResult<T> = std::result::Result<T, ParserError>;

const DISCOVERY_HEADER_BYTES: usize = 64;
const OLE_SIGNATURE: &[u8; 8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ContainerSignature {
    Compound,
    Unknown,
}

impl ContainerSignature {
    fn from_header(header: &[u8]) -> Self {
        if header.starts_with(OLE_SIGNATURE) {
            Self::Compound
        } else {
            Self::Unknown
        }
    }
}

#[derive(Debug, Clone)]
struct DiscoveryProbe {
    source_path: PathBuf,
    extension: Option<String>,
    signature: ContainerSignature,
}

impl DiscoveryProbe {
    fn from_path(path: &Path) -> ParserResult<Self> {
        let source_path = path.to_path_buf();
        let mut file = File::open(&source_path).map_err(|error| {
            ParserError::probe_io(&source_path, error.to_string())
        })?;

        let mut header = vec![0u8; DISCOVERY_HEADER_BYTES];
        let read_bytes = file.read(&mut header).map_err(|error| {
            ParserError::probe_io(&source_path, error.to_string())
        })?;
        header.truncate(read_bytes);
        let signature = ContainerSignature::from_header(&header);

        let extension = source_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());

        Ok(Self {
            source_path,
            extension,
            signature,
        })
    }

    fn extension_is(&self, expected: &str) -> bool {
        self.extension
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(expected))
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum ProbeConfidence {
    Unsupported = 0,
    Fallback = 1,
    Weak = 2,
    Strong = 3,
    Exact = 4,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProbeSource {
    Signature,
    Extension,
    ExtensionAndSignature,
    Fallback,
}

#[derive(Debug, Clone)]
struct ProbeResult {
    confidence: ProbeConfidence,
    source: ProbeSource,
    reason: &'static str,
}

impl ProbeResult {
    const fn supported(confidence: ProbeConfidence, source: ProbeSource, reason: &'static str) -> Self {
        Self {
            confidence,
            source,
            reason,
        }
    }

    const fn unsupported() -> Self {
        Self::supported(
            ProbeConfidence::Unsupported,
            ProbeSource::Fallback,
            "backend does not match",
        )
    }

    fn matches(&self) -> bool {
        self.confidence != ProbeConfidence::Unsupported
    }
}

#[derive(Debug, Clone)]
pub struct BackendCandidate {
    pub backend_name: &'static str,
    pub container: ContainerFormat,
    pub confidence: ProbeConfidence,
    pub source: ProbeSource,
    pub reason: &'static str,
}

impl BackendCandidate {
    fn new(name: &'static str, container: ContainerFormat, probe: ProbeResult) -> Self {
        Self {
            backend_name: name,
            container,
            confidence: probe.confidence,
            source: probe.source,
            reason: probe.reason,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryReport {
    pub requested: ContainerFormat,
    pub candidates: Vec<BackendCandidate>,
    pub selected: Option<BackendCandidate>,
    pub fallback_used: bool,
    pub fallback_reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ParserConfig {
    pub source_path: PathBuf,
    pub requested_container: ContainerFormat,
    pub strict: bool,
    pub allow_fallback: bool,
    pub deterministic: bool,
    pub max_bytes: Option<u64>,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            source_path: PathBuf::new(),
            requested_container: ContainerFormat::Unknown,
            strict: false,
            allow_fallback: true,
            deterministic: false,
            max_bytes: None,
        }
    }
}

impl ParserConfig {
    pub fn new<P: Into<PathBuf>>(source_path: P) -> Self {
        Self {
            source_path: source_path.into(),
            requested_container: ContainerFormat::Unknown,
            strict: false,
            allow_fallback: true,
            deterministic: false,
            max_bytes: None,
        }
    }

    fn validate(&self) -> ParserResult<()> {
        if self.source_path.as_os_str().is_empty() {
            return Err(ParserError::invalid_config(
                &self.source_path,
                "source_path cannot be empty",
            ));
        }
        if !self.source_path.exists() {
            return Err(ParserError::invalid_config(
                &self.source_path,
                "source_path does not exist",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ParsedStore {
    pub mailbox: Mailbox,
    pub folders: Vec<Folder>,
    pub messages: Vec<Message>,
    pub attachments: Vec<Attachment>,
    pub events: Vec<ParseEvent>,
    pub discovery: DiscoveryReport,
}

#[derive(Debug, Clone)]
pub struct ParseAttempt {
    pub backend_name: &'static str,
    pub container: ContainerFormat,
    pub error: String,
    pub source: ProbeSource,
}

pub struct BackendParseResult {
    pub mailbox: Mailbox,
    pub folders: Vec<Folder>,
    pub messages: Vec<Message>,
    pub attachments: Vec<Attachment>,
    pub events: Vec<ParseEvent>,
}

impl BackendParseResult {
    fn scaffolded(path: &Path, container: ContainerFormat, backend_name: &'static str) -> Self {
        let mut mailbox = Mailbox::new(path.to_path_buf(), container);
        mailbox.state = MailboxState::Degraded;
        mailbox.diagnostics.push(ParseEvent {
            id: ParseEventId::new(),
            location: None,
            stage: ParseStage::Discovery,
            class: ErrorClass::Parse,
            severity: Severity::Warn,
            message: format!("selected backend `{backend_name}` (scaffold mode)"),
            details: Some(format!("container kind: {:?}", container)),
            occurs_at: Utc::now(),
        });
        Self {
            mailbox,
            folders: Vec::new(),
            messages: Vec::new(),
            attachments: Vec::new(),
            events: Vec::new(),
        }
    }
}

pub trait ContainerBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn container_format(&self) -> ContainerFormat;
    fn probe(&self, probe: &DiscoveryProbe) -> ProbeResult;
    fn parse(&self, config: &ParserConfig) -> ParserResult<BackendParseResult>;
}

#[derive(Default)]
pub struct ParserRegistry {
    backends: Vec<Box<dyn ContainerBackend>>,
}

impl ParserRegistry {
    pub fn new() -> Self {
        Self {
            backends: vec![
                Box::new(PstBackend::new()),
                Box::new(OstBackend::new()),
                Box::new(MsgBackend::new()),
            ],
        }
    }

    pub fn with_backends(backends: Vec<Box<dyn ContainerBackend>>) -> Self {
        Self { backends }
    }

    pub fn register_backend<B: ContainerBackend + 'static>(&mut self, backend: B) {
        self.backends.push(Box::new(backend));
    }

    fn backend_by_kind(&self, container: ContainerFormat) -> Option<&'static str> {
        match container {
            ContainerFormat::Pst => Some("pst"),
            ContainerFormat::Ost => Some("ost"),
            ContainerFormat::Msg => Some("msg"),
            ContainerFormat::Unknown => None,
        }
    }

    fn named_backend(
        &self,
        backend_name: &'static str,
        container: ContainerFormat,
    ) -> Option<&dyn ContainerBackend> {
        self.backends.iter().find_map(|backend| {
            if backend.backend_name() == backend_name && backend.container_format() == container {
                Some(backend.as_ref())
            } else {
                None
            }
        })
    }

    fn push_fallback_candidate(
        candidates: &mut Vec<BackendCandidate>,
        name: &'static str,
        container: ContainerFormat,
        reason: &'static str,
    ) {
        candidates.push(BackendCandidate {
            backend_name: name,
            container,
            confidence: ProbeConfidence::Fallback,
            source: ProbeSource::Fallback,
            reason,
        });
    }

    fn apply_request_filter(
        &self,
        probe: &DiscoveryProbe,
        config: &ParserConfig,
        candidates: &mut Vec<BackendCandidate>,
        fallback_reasons: &mut Vec<String>,
    ) -> ParserResult<()> {
        let requested_present = candidates
            .iter()
            .any(|candidate| candidate.container == config.requested_container);

        if requested_present {
            if !config.allow_fallback {
                candidates.retain(|candidate| {
                    candidate.container == config.requested_container
                });
            }
            return Ok(());
        }

        if !config.allow_fallback {
            return Err(ParserError::unsupported_container(
                &config.source_path,
                config.requested_container,
            ));
        }

        if let Some(name) = self.backend_by_kind(config.requested_container) {
            Self::push_fallback_candidate(
                candidates,
                name,
                config.requested_container,
                "requested container not detected; fallback by request",
            );
            fallback_reasons.push(format!(
                "requested container `{}` was not detected and fallback was used",
                container_name(config.requested_container)
            ));
            return Ok(());
        }

        if let Some(requested_ext) = container_from_extension(probe.extension.as_deref()) {
            if let Some(name) = self.backend_by_kind(requested_ext) {
                Self::push_fallback_candidate(
                    candidates,
                    name,
                    requested_ext,
                    "requested container resolved from extension",
                );
                fallback_reasons.push(
                    "requested container was ambiguous; extension fallback applied".to_string(),
                );
                return Ok(());
            }
        }

        Err(ParserError::unsupported_container(
            &config.source_path,
            config.requested_container,
        ))
    }

    fn apply_auto_fallback(
        &self,
        probe: &DiscoveryProbe,
        candidates: &mut Vec<BackendCandidate>,
        fallback_reasons: &mut Vec<String>,
    ) -> ParserResult<()> {
        if let Some(container) = container_from_extension(probe.extension.as_deref()) {
            if let Some(name) = self.backend_by_kind(container) {
                Self::push_fallback_candidate(
                    candidates,
                    name,
                    container,
                    "extension fallback",
                );
                fallback_reasons.push(format!(
                    "selected container {} from extension fallback",
                    container_name(container)
                ));
                return Ok(());
            }
        }

        if matches!(probe.signature, ContainerSignature::Compound) {
            let mut added = false;
            for container in [ContainerFormat::Pst, ContainerFormat::Ost, ContainerFormat::Msg] {
                if let Some(name) = self.backend_by_kind(container) {
                    Self::push_fallback_candidate(
                        candidates,
                        name,
                        container,
                        "compound signature fallback",
                    );
                    added = true;
                }
            }
            if added {
                fallback_reasons.push(
                    "extension missing with compound signature; trying all backends as fallback".to_string(),
                );
                return Ok(());
            }
        }

        Err(ParserError::unsupported_container(
            &probe.source_path,
            ContainerFormat::Unknown,
        ))
    }

    fn dedupe_candidates(candidates: &mut Vec<BackendCandidate>) {
        let mut unique: Vec<BackendCandidate> = Vec::new();
        for candidate in candidates.drain(..) {
            let exists = unique.iter().any(|existing| {
                existing.container == candidate.container && existing.backend_name == candidate.backend_name
            });
            if !exists {
                unique.push(candidate);
            }
        }
        *candidates = unique;
    }

    fn ordered_candidates(mut candidates: Vec<BackendCandidate>) -> Vec<BackendCandidate> {
        let mut exact = Vec::new();
        let mut strong = Vec::new();
        let mut weak = Vec::new();
        let mut fallback = Vec::new();

        for candidate in candidates.drain(..) {
            match candidate.confidence {
                ProbeConfidence::Exact => exact.push(candidate),
                ProbeConfidence::Strong => strong.push(candidate),
                ProbeConfidence::Weak => weak.push(candidate),
                ProbeConfidence::Fallback | ProbeConfidence::Unsupported => fallback.push(candidate),
            }
        }

        exact
            .into_iter()
            .chain(strong)
            .chain(weak)
            .chain(fallback)
            .collect()
    }

    pub fn discover(&self, config: &ParserConfig) -> ParserResult<DiscoveryReport> {
        config.validate()?;
        let probe = DiscoveryProbe::from_path(&config.source_path)?;

        let mut candidates = Vec::new();
        let mut fallback_reasons = Vec::new();

        for backend in &self.backends {
            let probe_result = backend.probe(&probe);
            if !probe_result.matches() {
                continue;
            }
            candidates.push(BackendCandidate::new(
                backend.backend_name(),
                backend.container_format(),
                probe_result,
            ));
        }

        if config.requested_container != ContainerFormat::Unknown {
            self.apply_request_filter(&probe, config, &mut candidates, &mut fallback_reasons)?;
        } else if candidates.is_empty() {
            self.apply_auto_fallback(&probe, &mut candidates, &mut fallback_reasons)?;
        }

        Self::dedupe_candidates(&mut candidates);

        if candidates.is_empty() {
            return Err(ParserError::unsupported_container(
                &config.source_path,
                config.requested_container,
            ));
        }

        let ordered = Self::ordered_candidates(candidates);
        let selected = ordered.first().cloned();
        let fallback_used = match &selected {
            Some(candidate) => fallback_reasons.iter().any(|reason| reason.contains(candidate.backend_name)),
            None => false,
        };

        Ok(DiscoveryReport {
            requested: config.requested_container,
            candidates: ordered.clone(),
            selected,
            fallback_used,
            fallback_reasons,
        })
    }

    pub fn parse(&self, config: &ParserConfig) -> ParserResult<ParsedStore> {
        config.validate()?;
        let discovery = self.discover(config)?;

        let mut attempts = Vec::new();

        for (index, candidate) in discovery.candidates.iter().enumerate() {
            let backend = self.named_backend(candidate.backend_name, candidate.container).ok_or_else(|| {
                ParserError::backend_unavailable(
                    &config.source_path,
                    candidate.backend_name,
                    "backend no longer registered",
                )
            })?;

            match backend.parse(config) {
                Ok(result) => {
                    let mut discovery = discovery.clone();
                    discovery.fallback_used = index > 0;
                    if !candidate.reason.is_empty() {
                        attempts.push(ParseAttempt {
                            backend_name: candidate.backend_name,
                            container: candidate.container,
                            error: format!("selected {:?}", candidate),
                            source: candidate.source,
                        });
                    }

                    return Ok(ParsedStore {
                        mailbox: result.mailbox,
                        folders: result.folders,
                        messages: result.messages,
                        attachments: result.attachments,
                        events: result.events,
                        discovery,
                    });
                }
                Err(error) => {
                    attempts.push(ParseAttempt {
                        backend_name: candidate.backend_name,
                        container: candidate.container,
                        error: error.to_string(),
                        source: candidate.source,
                    });

                    if config.strict || !config.allow_fallback {
                        return Err(error);
                    }
                }
            }
        }

        Err(ParserError::backend_exhausted(&config.source_path, attempts))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PstBackend;

impl PstBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl ContainerBackend for PstBackend {
    fn backend_name(&self) -> &'static str {
        "pst"
    }

    fn container_format(&self) -> ContainerFormat {
        ContainerFormat::Pst
    }

    fn probe(&self, probe: &DiscoveryProbe) -> ProbeResult {
        if probe.extension_is("pst") {
            ProbeResult::supported(
                ProbeConfidence::Exact,
                ProbeSource::Extension,
                "path extension `.pst`",
            )
        } else if matches!(probe.signature, ContainerSignature::Compound) {
            ProbeResult::supported(
                ProbeConfidence::Weak,
                ProbeSource::Signature,
                "OLE compound container signature",
            )
        } else {
            ProbeResult::unsupported()
        }
    }

    fn parse(&self, request: &ParserConfig) -> ParserResult<BackendParseResult> {
        let _ = request.max_bytes;
        Err(ParserError::backend_unavailable(
            &request.source_path,
            self.backend_name(),
            "pst backend scaffolded; real parser implementation is not wired yet",
        ))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OstBackend;

impl OstBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl ContainerBackend for OstBackend {
    fn backend_name(&self) -> &'static str {
        "ost"
    }

    fn container_format(&self) -> ContainerFormat {
        ContainerFormat::Ost
    }

    fn probe(&self, probe: &DiscoveryProbe) -> ProbeResult {
        if probe.extension_is("ost") {
            ProbeResult::supported(
                ProbeConfidence::Exact,
                ProbeSource::Extension,
                "path extension `.ost`",
            )
        } else if matches!(probe.signature, ContainerSignature::Compound) {
            ProbeResult::supported(
                ProbeConfidence::Weak,
                ProbeSource::Signature,
                "OLE compound container signature",
            )
        } else {
            ProbeResult::unsupported()
        }
    }

    fn parse(&self, request: &ParserConfig) -> ParserResult<BackendParseResult> {
        let _ = request.max_bytes;
        Err(ParserError::backend_unavailable(
            &request.source_path,
            self.backend_name(),
            "ost backend scaffolded; real parser implementation is not wired yet",
        ))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MsgBackend;

impl MsgBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl ContainerBackend for MsgBackend {
    fn backend_name(&self) -> &'static str {
        "msg"
    }

    fn container_format(&self) -> ContainerFormat {
        ContainerFormat::Msg
    }

    fn probe(&self, probe: &DiscoveryProbe) -> ProbeResult {
        if probe.extension_is("msg") {
            ProbeResult::supported(
                ProbeConfidence::Exact,
                ProbeSource::Extension,
                "path extension `.msg`",
            )
        } else if matches!(probe.signature, ContainerSignature::Compound) {
            ProbeResult::supported(
                ProbeConfidence::Weak,
                ProbeSource::Signature,
                "OLE compound container signature",
            )
        } else {
            ProbeResult::unsupported()
        }
    }

    fn parse(&self, request: &ParserConfig) -> ParserResult<BackendParseResult> {
        let _ = request.max_bytes;
        Err(ParserError::backend_unavailable(
            &request.source_path,
            self.backend_name(),
            "msg backend scaffolded; real parser implementation is not wired yet",
        ))
    }
}

#[derive(Debug, Error)]
pub enum ParserError {
    #[error("I/O probing error for `{path:?}`: {message}")]
    ProbeIo { path: PathBuf, message: String },

    #[error("unsupported container for `{path:?}`; requested {requested:?}")]
    UnsupportedContainer { path: PathBuf, requested: ContainerFormat },

    #[error("backend `{backend_name}` is unavailable: {message}")]
    BackendUnavailable {
        path: PathBuf,
        backend_name: &'static str,
        message: String,
    },

    #[error("backend `{backend_name}` failed parsing `{path:?}`: {message}")]
    BackendFailed {
        path: PathBuf,
        backend_name: &'static str,
        message: String,
    },

    #[error("invalid parser config for `{path:?}`: {message}")]
    InvalidConfig { path: PathBuf, message: String },

    #[error("all backends failed for `{path:?}`: {attempts:?}")]
    BackendExhausted {
        path: PathBuf,
        attempts: Vec<ParseAttempt>,
    },
}

impl ParserError {
    fn probe_io(path: &Path, message: String) -> Self {
        Self::ProbeIo {
            path: path.to_path_buf(),
            message,
        }
    }

    fn unsupported_container(path: &Path, requested: ContainerFormat) -> Self {
        Self::UnsupportedContainer {
            path: path.to_path_buf(),
            requested,
        }
    }

    fn backend_unavailable(
        path: &Path,
        backend_name: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::BackendUnavailable {
            path: path.to_path_buf(),
            backend_name,
            message: message.into(),
        }
    }

    fn backend_failed(
        path: &Path,
        backend_name: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::BackendFailed {
            path: path.to_path_buf(),
            backend_name,
            message: message.into(),
        }
    }

    fn invalid_config(path: &Path, message: impl Into<String>) -> Self {
        Self::InvalidConfig {
            path: path.to_path_buf(),
            message: message.into(),
        }
    }

    fn backend_exhausted(path: &Path, attempts: Vec<ParseAttempt>) -> Self {
        Self::BackendExhausted {
            path: path.to_path_buf(),
            attempts,
        }
    }

    pub fn taxonomy(&self) -> ErrorClass {
        match self {
            Self::ProbeIo { .. } => ErrorClass::Io,
            Self::InvalidConfig { .. } => ErrorClass::Integrity,
            Self::UnsupportedContainer { .. } => ErrorClass::Parse,
            Self::BackendUnavailable { .. } => ErrorClass::Parse,
            Self::BackendFailed { .. } => ErrorClass::Parse,
            Self::BackendExhausted { .. } => ErrorClass::Parse,
        }
    }
}

fn container_name(container: ContainerFormat) -> &'static str {
    match container {
        ContainerFormat::Pst => "pst",
        ContainerFormat::Ost => "ost",
        ContainerFormat::Msg => "msg",
        ContainerFormat::Unknown => "auto",
    }
}

fn container_from_extension(extension: Option<&str>) -> Option<ContainerFormat> {
    match extension {
        Some("pst") => Some(ContainerFormat::Pst),
        Some("ost") => Some(ContainerFormat::Ost),
        Some("msg") => Some(ContainerFormat::Msg),
        _ => None,
    }
}
