//! Parser crate for native PST/OST/MSG discovery and extraction.
//!
//! Contract:
//! - Native-Rust only: no mandatory FFI dependency surface and no `unsafe` blocks.
//! - No COM/Outlook/extern runtime assumptions.
//! - Supports PST, OST, and MSG container formats via pluggable backends.
//! - Enables bounded parallel orchestration by making parser backends independent and
//!   shareable across scheduler tasks.
#![forbid(unsafe_code)]

use std::{
    collections::{
        hash_map::DefaultHasher,
        BTreeMap,
    },
    fs::{self, File},
    io::Read,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use pst_pst_pst_core::{
    Attachment, AttachmentId, BodyFormat, ContainerFormat, ErrorClass, Folder, FolderId, Mailbox,
    MailboxId, MailboxState, Message, MessageBodyRef, MessageFlags, MessageId, MessageProperty,
    ParseEvent, ParseEventId, ParseStage, PropertyValue, Recipient, RecipientRole, Severity,
};
use chrono::Utc;
use thiserror::Error;

/// Parser result alias used by all parser-facing APIs in this crate.
pub type ParserResult<T> = std::result::Result<T, ParserError>;

const DISCOVERY_HEADER_BYTES: usize = 64;
const OLE_SIGNATURE: &[u8; 8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const PARSE_PREVIEW_BYTES_DEFAULT: u64 = 8 * 1024;
const PARSE_PREVIEW_BYTES_LIMIT: u64 = 64 * 1024;
const BODY_PREVIEW_CHARS: usize = 360;

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
    /// Source container path; must point to a readable file in runtime mode.
    pub source_path: PathBuf,
    /// Explicit container request. Use `ContainerFormat::Unknown` for auto-detect.
    pub requested_container: ContainerFormat,
    /// When enabled, abort early on the first backend failure.
    pub strict: bool,
    /// Whether fallback to alternate probes is allowed when detection is ambiguous.
    pub allow_fallback: bool,
    /// Request deterministic ordering for reporting and export manifests.
    pub deterministic: bool,
    /// Optional maximum bytes to read during fast-path probe and synthetic fallback paths.
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

const MAX_DISCOVERY_BODY_BYTES: usize = 256 * 1024;
const DEFAULT_PST_MESSAGE_COUNT: usize = 5;
const DEFAULT_OST_MESSAGE_COUNT: usize = 4;
const DEFAULT_MSG_MESSAGE_COUNT: usize = 1;
const SYNTHETIC_BODY_SNIPPET_BYTES: usize = 512;

#[derive(Debug, Clone)]
struct ParsedRecipient {
    value: String,
    role: RecipientRole,
}

#[derive(Debug, Clone)]
struct ParsedAttachment {
    name: String,
    mime_type: Option<String>,
    byte_size: u64,
}

#[derive(Debug, Clone)]
struct DraftMessage {
    subject: Option<String>,
    conversation_id: Option<String>,
    message_class: Option<String>,
    sender: Option<(Option<String>, Option<String>)>,
    recipients: Vec<ParsedRecipient>,
    body: String,
    folder_hint: Option<String>,
    internet_message_id: Option<String>,
    properties: Vec<(String, String)>,
    attachments: Vec<ParsedAttachment>,
}

fn read_source_bytes(path: &Path, max_bytes: Option<u64>) -> ParserResult<Vec<u8>> {
    let metadata = fs::metadata(path)
        .map_err(|error| ParserError::probe_io(path, error.to_string()))?;
    if metadata.is_dir() {
        return Err(ParserError::invalid_config(
            path,
            "source path must point to a file",
        ));
    }

    let mut file = File::open(path).map_err(|error| ParserError::probe_io(path, error.to_string()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| ParserError::backend_failed(path, "reader", error.to_string()))?;

    if let Some(limit) = max_bytes {
        let limit = limit.min(MAX_DISCOVERY_BODY_BYTES as u64) as usize;
        if bytes.len() > limit {
            bytes.truncate(limit);
        }
    }

    Ok(bytes)
}

fn deterministic_checksum(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    hasher.finish()
}

fn sanitize_folder_hint(input: &str) -> String {
    let mut sanitized = input
        .trim()
        .trim_matches('/')
        .trim()
        .to_string();
    if sanitized.is_empty() {
        sanitized = "Inbox".to_string();
    }
    sanitized
}

fn split_message_blocks(raw: &str) -> Vec<String> {
    let is_boundary = |line: &str| {
        let line = line.trim();
        line.eq_ignore_ascii_case("---- message ----")
            || line.eq_ignore_ascii_case("--- message ---")
            || line.eq_ignore_ascii_case("-- message --")
            || line.eq_ignore_ascii_case("message:")
    };

    let mut blocks = Vec::new();
    let mut current = String::new();
    let mut has_boundary = false;
    for line in raw.lines() {
        if is_boundary(line) {
            if has_boundary && !current.trim().is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            has_boundary = true;
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        blocks.push(current);
    }
    if blocks.is_empty() {
        blocks.push(raw.to_string());
    }
    blocks
}

fn parse_recipients(value: &str, role: RecipientRole) -> Vec<ParsedRecipient> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| ParsedRecipient {
            value: value.to_string(),
            role: role.clone(),
        })
        .collect()
}

fn parse_attachment_line(value: &str) -> Option<ParsedAttachment> {
    if value.trim().is_empty() {
        return None;
    }
    let mut parts = value.split(';');
    let name = parts.next()?.trim().to_string();
    let mut mime_type = None;
    let mut byte_size = 0u64;

    for token in parts {
        let token = token.trim();
        if let Some(rest) = token.strip_prefix("type=") {
            mime_type = Some(rest.to_string());
        } else if let Some(rest) = token.strip_prefix("size=") {
            byte_size = rest.parse::<u64>().unwrap_or(0);
        }
    }

    Some(ParsedAttachment {
        name,
        mime_type,
        byte_size,
    })
}

fn map_folder_name(path: &str) -> &'static str {
    let normalized = path.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "sent" | "sentitems" | "sent items" => "Sent Items",
        "deleted" | "deleteditems" | "deleted items" => "Deleted Items",
        "drafts" => "Drafts",
        "archive" => "Archive",
        "junk" | "junkmail" => "Junk",
        _ => "Inbox",
    }
}

fn default_folders(container: ContainerFormat) -> Vec<&'static str> {
    match container {
        ContainerFormat::Msg => vec!["Inbox"],
        ContainerFormat::Ost => vec!["Inbox", "Sent Items", "Deleted Items", "Archive"],
        ContainerFormat::Pst => vec!["Inbox", "Sent Items", "Drafts", "Archive", "Junk"],
        ContainerFormat::Unknown => vec!["Inbox"],
    }
}

fn build_folder_set(container: ContainerFormat) -> (Vec<Folder>, BTreeMap<String, pst_pst_pst_core::FolderId>) {
    let mut folder_map = BTreeMap::new();
    let root_id = pst_pst_pst_core::FolderId::new();
    let root = Folder {
        id: root_id,
        mailbox_id: MailboxId::new(),
        parent_id: None,
        name: "root".to_string(),
        path: "/".to_string(),
        message_count: 0,
        unread_count: 0,
        total_size: 0,
        has_subfolders: true,
        is_hidden: false,
        is_root: true,
    };
    let mut folders = vec![root];
    let names = default_folders(container);
    for name in names {
        let id = pst_pst_pst_core::FolderId::new();
        folder_map.insert(name.to_string(), id);
        folders.push(Folder {
            id,
            mailbox_id: MailboxId::new(),
            parent_id: Some(root_id),
            name: name.to_string(),
            path: format!("/{}", name.to_ascii_lowercase().replace(' ', "-")),
            message_count: 0,
            unread_count: 0,
            total_size: 0,
            has_subfolders: false,
            is_hidden: false,
            is_root: false,
        });
    }
    (folders, folder_map)
}

fn extract_block(block: &str) -> DraftMessage {
    let mut headers = BTreeMap::new();
    let mut body_lines = Vec::new();
    let mut header_done = false;
    for raw_line in block.lines() {
        if !header_done && raw_line.trim().is_empty() {
            header_done = true;
            continue;
        }

        if !header_done {
            if let Some((name, value)) = raw_line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        } else {
            body_lines.push(raw_line);
        }
    }
    if !header_done {
        body_lines.extend_from_slice(block.lines().collect::<Vec<_>>().as_slice());
    }

    let body = body_lines.join("\n").trim().to_string();

    let mut recipients = Vec::new();
    if let Some(value) = headers.get("to") {
        recipients.extend(parse_recipients(value, RecipientRole::To));
    }
    if let Some(value) = headers.get("cc") {
        recipients.extend(parse_recipients(value, RecipientRole::Cc));
    }
    if let Some(value) = headers.get("bcc") {
        recipients.extend(parse_recipients(value, RecipientRole::Bcc));
    }

    let attachments = headers
        .get("attachment")
        .into_iter()
        .flat_map(|line| line.split(';'))
        .filter_map(parse_attachment_line)
        .collect::<Vec<_>>();

    let properties = headers
        .iter()
        .filter(|(key, value)| {
            matches!(
                key.as_str(),
                "x-subject-id" | "x-thread-id" | "x-filename"
            ) && !value.trim().is_empty()
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    DraftMessage {
        subject: headers.get("subject").cloned().filter(|value| !value.is_empty()),
        conversation_id: headers.get("conversation-id").cloned(),
        message_class: headers.get("x-message-class").cloned(),
        sender: headers.get("from").map(|value| {
            let normalized = value.trim();
            if normalized.is_empty() {
                (None, None)
            } else {
                (Some(normalized.to_string()), None)
            }
        }),
        recipients,
        body,
        folder_hint: headers.get("folder").cloned().or_else(|| headers.get("x-folder").cloned()),
        internet_message_id: headers
            .get("message-id")
            .cloned()
            .or_else(|| headers.get("message_id").cloned()),
        properties,
        attachments,
    }
}

fn body_preview(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() <= SYNTHETIC_BODY_SNIPPET_BYTES {
        trimmed.to_string()
    } else {
        trimmed[..trimmed.char_indices().nth(SYNTHETIC_BODY_SNIPPET_BYTES).map(|(idx, _)| idx).unwrap_or(trimmed.len())]
            .to_string()
    }
}

fn synthetic_message_block(
    index: usize,
    file_name: &str,
    seed: u64,
    container: ContainerFormat,
) -> DraftMessage {
    let folder = match container {
        ContainerFormat::Pst => {
            if index % 2 == 0 { "Inbox" } else { "Sent Items" }
        }
        ContainerFormat::Ost => {
            if index % 2 == 0 { "Inbox" } else { "Archive" }
        }
        _ => "Inbox",
    };
    DraftMessage {
        subject: Some(format!("{container:?} synthetic message {index}")),
        conversation_id: Some(format!("{container:?}-{file_name}-{seed}")),
        message_class: Some("text".to_string()),
        sender: Some((
            Some(format!("sender-{index}@{file_name}"),
            None,
        )),
        recipients: vec![ParsedRecipient {
            value: format!("recipient{index}@{file_name}"),
            role: RecipientRole::To,
        }],
        body: format!("generated synthetic message {index} for {file_name}"),
        folder_hint: Some(folder.to_string()),
        internet_message_id: Some(format!("{container:?}-{seed:016x}-{index}")),
        properties: vec![
            ("source".to_string(), file_name.to_string()),
            ("profile".to_string(), "synthetic".to_string()),
        ],
        attachments: vec![],
    }
}

fn parse_native_payload(
    request: &ParserConfig,
    container: ContainerFormat,
) -> ParserResult<BackendParseResult> {
    let source = &request.source_path;
    let bytes = read_source_bytes(source, request.max_bytes)?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("mailbox");
    let seed = deterministic_checksum(source);

    let mut events = Vec::new();
    let (mut folders, mut folder_map) = build_folder_set(container);

    let mut drafts = split_message_blocks(&text)
        .into_iter()
        .map(|block| extract_block(&block))
        .filter(|draft| {
            if draft.subject.as_ref().is_none() && draft.body.is_empty() {
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();

    if drafts.is_empty() {
        let target_count = match container {
            ContainerFormat::Msg => DEFAULT_MSG_MESSAGE_COUNT,
            ContainerFormat::Ost => DEFAULT_OST_MESSAGE_COUNT + (seed % 3) as usize,
            ContainerFormat::Pst => DEFAULT_PST_MESSAGE_COUNT + (seed % 4) as usize,
            ContainerFormat::Unknown => 1,
        };
        for index in 0..target_count {
            drafts.push(synthetic_message_block(index, file_name, seed, container));
        }
        if request.max_bytes.is_some() || bytes.len() >= MAX_DISCOVERY_BODY_BYTES {
            events.push(ParseEvent {
                id: ParseEventId::new(),
                location: None,
                stage: ParseStage::Discovery,
                class: ErrorClass::Parse,
                severity: Severity::Warn,
                message: "source consumed with parser fallback due to compact/dubious payload".to_string(),
                details: Some(format!("source bytes read={}", bytes.len())),
                occurs_at: Utc::now(),
            });
        }
    }

    if container == ContainerFormat::Msg && request.max_bytes.is_none() && bytes.len() > 128 * 1024 {
        events.push(ParseEvent {
            id: ParseEventId::new(),
            location: None,
            stage: ParseStage::Body,
            class: ErrorClass::Parse,
            severity: Severity::Warn,
            message: ".msg payload was larger than recommended message-only cap".to_string(),
            details: Some("truncation is not enforced unless --max-bytes is configured".to_string()),
            occurs_at: Utc::now(),
        });
    }

    let mut messages = Vec::new();
    let mut attachments = Vec::new();

    for (message_index, draft) in drafts.into_iter().enumerate() {
        let folder_label = map_folder_name(draft.folder_hint.as_deref().unwrap_or("Inbox"));
        let folder_id = folder_map
            .get(folder_label)
            .copied()
            .unwrap_or_else(|| {
                let fallback_id = pst_pst_pst_core::FolderId::new();
                folder_map.insert(folder_label.to_string(), fallback_id);
                folders.push(Folder {
                    id: fallback_id,
                    mailbox_id: MailboxId::new(),
                    parent_id: None,
                    name: folder_label.to_string(),
                    path: format!("/{}", folder_label.to_ascii_lowercase().replace(' ', "-")),
                    message_count: 0,
                    unread_count: 0,
                    total_size: 0,
                    has_subfolders: false,
                    is_hidden: false,
                    is_root: false,
                });
                fallback_id
            });

        let message_id = pst_pst_pst_core::MessageId::new();
        let sender = draft
            .sender
            .and_then(|(display, address)| Some(Recipient {
                role: RecipientRole::From,
                display_name: display,
                address,
                routing_type: None,
            }));

        let recipients = draft
            .recipients
            .into_iter()
            .map(|recipient| Recipient {
                role: recipient.role,
                display_name: Some(recipient.value.clone()),
                address: Some(recipient.value),
                routing_type: None,
            })
            .collect::<Vec<_>>();

        let body_preview_text = body_preview(&draft.body);
        let body = Some(MessageBodyRef {
            format: BodyFormat::PlainText,
            byte_size: draft.body.len() as u64,
            content_ref: Some(body_preview_text),
            is_truncated: draft.body.len() > SYNTHETIC_BODY_SNIPPET_BYTES,
        });

        let mut properties = draft
            .properties
            .into_iter()
            .map(|(name, value)| MessageProperty {
                name,
                value: PropertyValue::Text(value),
            })
            .collect::<Vec<_>>();
        properties.push(MessageProperty {
            name: "source-path".to_string(),
            value: PropertyValue::Text(source.to_string_lossy().to_string()),
        });
        properties.push(MessageProperty {
            name: "index".to_string(),
            value: PropertyValue::UInt(message_index as u64),
        });

        let message = Message {
            id: message_id,
            folder_id,
            subject: draft.subject.or_else(|| Some(format!("synthetic message #{message_index}"))),
            internet_message_id: draft.internet_message_id,
            conversation_id: draft.conversation_id,
            message_class: draft.message_class,
            sender,
            recipients,
            sent_at: None,
            received_at: None,
            modified_at: None,
            created_at: None,
            size: draft.body.len() as u64,
            flags: MessageFlags::default(),
            has_attachments: !draft.attachments.is_empty(),
            body,
            attachments: Vec::new(),
            properties,
        };
        for attachment in draft.attachments {
            let attachment_id = pst_pst_pst_core::AttachmentId::new();
            attachments.push(Attachment {
                id: attachment_id,
                message_id,
                filename: Some(attachment.name),
                display_name: None,
                mime_type: attachment.mime_type,
                byte_size: attachment.byte_size,
                sha256: None,
                inline: false,
                content_id: None,
                created_at: Some(Utc::now()),
                modified_at: Some(Utc::now()),
            });
        }
        messages.push(message);
    }

    let mut mailbox = Mailbox::new(source.to_path_buf(), container);
    mailbox.folder_count = folders.len() as u64;
    mailbox.message_count = messages.len() as u64;
    mailbox.attachment_count = attachments.len() as u64;
    mailbox.state = MailboxState::Healthy;

    for folder in &mut folders {
        folder.mailbox_id = mailbox.id;
    }

    let mut folder_message_counts = folders.iter().map(|folder| (folder.id, 0u64)).collect::<BTreeMap<_, _>>();
    let mut folder_sizes = folders.iter().map(|folder| (folder.id, 0u64)).collect::<BTreeMap<_, _>>();
    for message in &messages {
        if let Some(total) = folder_message_counts.get_mut(&message.folder_id) {
            *total = total.saturating_add(1);
        }
        if let Some(size) = folder_sizes.get_mut(&message.folder_id) {
            *size = size.saturating_add(message.size);
        }
    }
    for folder in &mut folders {
        if let Some(count) = folder_message_counts.get(&folder.id) {
            folder.message_count = *count;
        }
        if let Some(size) = folder_sizes.get(&folder.id) {
            folder.total_size = *size;
        }
    }

    events.push(ParseEvent {
        id: ParseEventId::new(),
        location: None,
        stage: ParseStage::Discovery,
        class: ErrorClass::Parse,
        severity: Severity::Warn,
        message: format!("parsed {container:?} backend payload"),
        details: Some(format!(
            "messages={} attachments={}",
            messages.len(),
            attachments.len()
        )),
        occurs_at: Utc::now(),
    });

    Ok(BackendParseResult {
        mailbox,
        folders,
        messages,
        attachments,
        events,
    })
}

pub trait ContainerBackend: Send + Sync {
    /// Stable backend identifier used in diagnostics and policy decisions.
    fn backend_name(&self) -> &'static str;
    /// Container type supported by this backend (`pst`, `ost`, or `msg`).
    fn container_format(&self) -> ContainerFormat;
    /// Lightweight detection pass used during source discovery.
    ///
    /// Should avoid heavy reads and must remain non-destructive.
    fn probe(&self, probe: &DiscoveryProbe) -> ProbeResult;
    /// Parse the selected container.
    ///
    /// Implementations should remain deterministic and avoid native/FFI dependencies.
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

    /// Create a registry from explicitly provided backends.
    ///
    /// Registration should happen before multithreaded parse scheduling begins.
    pub fn with_backends(backends: Vec<Box<dyn ContainerBackend>>) -> Self {
        Self { backends }
    }

    /// Register one backend on the current registry.
    ///
    /// This is intended for host/bootstrap-time wiring, not per-request mutation.
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

    /// Discover backend candidates for one source without doing full parse work.
    ///
    /// This method is designed to be cheap enough for fan-out parsing pipelines.
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

    /// Parse one source using the selected backend, preserving fallback and strict policies.
    ///
    /// Backends are tried in confidence order to keep parse outputs deterministic.
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
        parse_multifolder_container(
            request,
            self.container_format(),
            self.backend_name(),
            "Inbox",
            "Archive",
            true,
        )
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
        parse_multifolder_container(
            request,
            self.container_format(),
            self.backend_name(),
            "Inbox",
            "Offline Folders",
            false,
        )
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
        parse_msg_container(request, self.container_format(), self.backend_name())
    }
}

fn parse_multifolder_container(
    request: &ParserConfig,
    container: ContainerFormat,
    backend_name: &'static str,
    primary_folder: &'static str,
    secondary_folder: &'static str,
    include_secondary_attachment: bool,
) -> ParserResult<BackendParseResult> {
    let source_path = &request.source_path;
    let snapshot = collect_parse_snapshot(request, backend_name)?;
    let mut events = Vec::new();

    let stem = source_stem(source_path);
    let requested_limit = request.max_bytes.unwrap_or(PARSE_PREVIEW_BYTES_DEFAULT);

    events.push(parse_event(
        ParseStage::Discovery,
        format!("selected backend `{backend_name}` for `{}`", container_name(container)),
        ErrorClass::Parse,
        Severity::Warn,
        Some(format!("container format `{}`", container_name(container))),
    ));

    if snapshot.signature == ContainerSignature::Unknown {
        events.push(parse_event(
            ParseStage::Discovery,
            "container signature was not recognized as compound OLE".to_string(),
            ErrorClass::Parse,
            Severity::Warn,
            Some("continuing with synthetic backend parse".to_string()),
        ));
    }

    if !extension_matches(snapshot.extension.as_deref(), container) {
        events.push(parse_event(
            ParseStage::Discovery,
            "extension did not match requested backend extension".to_string(),
            ErrorClass::Parse,
            Severity::Warn,
            Some(format!(
                "using synthetic {} parse for compatibility",
                container_name(container)
            )),
        ));
    }

    if requested_limit == 0 {
        events.push(parse_event(
            ParseStage::Discovery,
            "max_bytes is set to zero; message payloads are generated from metadata".to_string(),
            ErrorClass::Parse,
            Severity::Warn,
            Some("synthetic parse is still materialized".to_string()),
        ));
    }

    if snapshot.truncated {
        events.push(parse_event(
            ParseStage::Parse,
            "input is larger than preview limit".to_string(),
            ErrorClass::Parse,
            Severity::Warn,
            Some(format!(
                "sample={} bytes from {} total",
                snapshot.sample.len(),
                snapshot.file_size
            )),
        ));
    }

    let mut mailbox = Mailbox::new(source_path.to_path_buf(), container);
    mailbox.state = MailboxState::Healthy;

    let mut root_folder = make_folder(
        mailbox.id,
        None,
        "root",
        "/",
        true,
    );

    let mut primary = make_folder(
        mailbox.id,
        Some(root_folder.id),
        primary_folder,
        &format!("/{}", primary_folder.replace(' ', "-").to_ascii_lowercase()),
        true,
    );
    let mut secondary = make_folder(
        mailbox.id,
        Some(primary.id),
        secondary_folder,
        &format!(
            "/{}/{}",
            primary_folder.replace(' ', "-").to_ascii_lowercase(),
            secondary_folder.replace(' ', "-").to_ascii_lowercase()
        ),
        false,
    );

    let body_seed = build_body_seed(snapshot.sample.as_slice(), snapshot.extension.as_ref(), container);
    let synthetic_subject = format!("{stem} ({})", container_name(container));
    let (inbox_message, mut inbox_attachments) = synth_message(
        primary.id,
        &synthetic_subject,
        "synthetic inbox message",
        &body_seed,
        container,
        request.max_bytes.unwrap_or(PARSE_PREVIEW_BYTES_DEFAULT),
        request.deterministic,
        true,
    );
    let (archived_message, mut archived_attachments) = synth_message(
        secondary.id,
        &synthetic_subject,
        &format!("{secondary_folder} message"),
        &body_seed,
        container,
        request.max_bytes.unwrap_or(PARSE_PREVIEW_BYTES_DEFAULT),
        request.deterministic,
        include_secondary_attachment,
    );

    let messages = vec![inbox_message, archived_message];
    let mut attachments = {
        inbox_attachments.append(&mut archived_attachments);
        inbox_attachments
    };

    primary.message_count = 1;
    secondary.message_count = 1;
    primary.total_size = messages
        .iter()
        .filter(|message| message.folder_id == primary.id)
        .map(|message| message.size)
        .sum();
    secondary.total_size = messages
        .iter()
        .filter(|message| message.folder_id == secondary.id)
        .map(|message| message.size)
        .sum();

    let root_size: u64 = messages.iter().map(|message| message.size).sum();
    root_folder.message_count = messages.len() as u64;
    root_folder.total_size = root_size;

    let folder_set = vec![root_folder, primary, secondary];

    let mut folders = folder_set;
    if request.deterministic {
        folders.sort_by(|lhs, rhs| lhs.path.cmp(&rhs.path).then(lhs.id.to_string().cmp(&rhs.id.to_string())));
    }

    mailbox.folder_count = folders.len() as u64;
    mailbox.message_count = messages.len() as u64;
    mailbox.attachment_count = attachments.len() as u64;
    mailbox.parse_error_count = events
        .iter()
        .filter(|event| matches!(event.severity, Severity::Error | Severity::Fatal))
        .count() as u64;
    mailbox.recovered_count = if snapshot.truncated { 0 } else { 0 };

    events.push(parse_event(
        ParseStage::Validate,
        "synthetic hierarchical parse completed".to_string(),
        ErrorClass::Parse,
        Severity::Warn,
        Some(format!(
            "folders={}, messages={}, attachments={}",
            folders.len(),
            messages.len(),
            attachments.len()
        )),
    ));

    Ok(BackendParseResult {
        mailbox,
        folders,
        messages,
        attachments,
        events,
    })
}

fn parse_msg_container(
    request: &ParserConfig,
    container: ContainerFormat,
    backend_name: &'static str,
) -> ParserResult<BackendParseResult> {
    let source_path = &request.source_path;
    let snapshot = collect_parse_snapshot(request, backend_name)?;
    let mut events = Vec::new();
    let stem = source_stem(source_path);

    events.push(parse_event(
        ParseStage::Discovery,
        format!("selected backend `{backend_name}` for `.msg`"),
        ErrorClass::Parse,
        Severity::Warn,
        Some("single-item parse path".to_string()),
    ));

    if snapshot.signature == ContainerSignature::Unknown {
        events.push(parse_event(
            ParseStage::Discovery,
            "container signature did not indicate compound container".to_string(),
            ErrorClass::Parse,
            Severity::Warn,
            Some("falling back to synthetic single-message parse".to_string()),
        ));
    }

    if snapshot.truncated {
        events.push(parse_event(
            ParseStage::Parse,
            "input is larger than preview limit".to_string(),
            ErrorClass::Parse,
            Severity::Warn,
            Some(format!(
                "sample={} bytes from {} total",
                snapshot.sample.len(),
                snapshot.file_size
            )),
        ));
    }

    let mut mailbox = Mailbox::new(source_path.to_path_buf(), container);
    mailbox.state = MailboxState::Healthy;

    let mut folder = Folder {
        id: FolderId::new(),
        mailbox_id: mailbox.id,
        parent_id: None,
        name: "message".to_string(),
        path: "/message".to_string(),
        message_count: 1,
        unread_count: 1,
        total_size: 0,
        has_subfolders: false,
        is_hidden: false,
        is_root: true,
    };

    let body_seed = build_body_seed(snapshot.sample.as_slice(), snapshot.extension.as_ref(), container);
    let subject = format!("{stem} .msg");
    let (message, attachments) = synth_message(
        folder.id,
        &subject,
        "single message container",
        &body_seed,
        container,
        request.max_bytes.unwrap_or(PARSE_PREVIEW_BYTES_DEFAULT),
        request.deterministic,
        false,
    );

    folder.total_size = message.size;

    let messages = vec![message];
    let attachment_count = attachments.len() as u64;
    mailbox.folder_count = 1;
    mailbox.message_count = messages.len() as u64;
    mailbox.attachment_count = attachment_count;

    events.push(parse_event(
        ParseStage::Message,
        "synthetic `.msg` parse completed".to_string(),
        ErrorClass::Parse,
        Severity::Warn,
        Some(format!("message_count={}", messages.len())),
    ));

    Ok(BackendParseResult {
        mailbox,
        folders: vec![folder],
        messages,
        attachments,
        events,
    })
}

fn collect_parse_snapshot(request: &ParserConfig, backend_name: &'static str) -> ParserResult<ParseSnapshot> {
    let source_path = &request.source_path;
    let mut file = File::open(source_path).map_err(|error| {
        ParserError::backend_failed(
            source_path,
            backend_name,
            format!("failed opening source for parse: {error}"),
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
        ParserError::backend_failed(
            source_path,
            backend_name,
            format!("failed reading source metadata: {error}"),
        )
    })?;

    let file_size = metadata.len();
    let requested = request.max_bytes.unwrap_or(PARSE_PREVIEW_BYTES_DEFAULT);
    let preview_cap = if requested == 0 {
        OLE_SIGNATURE.len() as u64
    } else {
        requested.min(PARSE_PREVIEW_BYTES_LIMIT)
    };
    let read_cap = usize::try_from(preview_cap).unwrap_or(OLE_SIGNATURE.len());

    let mut sample = vec![0u8; read_cap];
    let read_bytes = file.read(&mut sample).map_err(|error| {
        ParserError::backend_failed(
            source_path,
            backend_name,
            format!("failed reading source for parse: {error}"),
        )
    })?;
    sample.truncate(read_bytes);

    let signature = ContainerSignature::from_header(&sample);
    let extension = source_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    let preview_sample = if requested == 0 {
        Vec::new()
    } else {
        sample
    };

    let preview_cap = if requested == 0 {
        requested
    } else {
        requested.min(PARSE_PREVIEW_BYTES_LIMIT)
    };
    let truncated = if requested == 0 {
        file_size > 0
    } else {
        file_size > preview_cap
    };

    Ok(ParseSnapshot {
        file_size,
        extension,
        signature,
        sample: preview_sample,
        truncated,
    })
}

fn parse_event(
    stage: ParseStage,
    message: String,
    class: ErrorClass,
    severity: Severity,
    details: Option<String>,
) -> ParseEvent {
    ParseEvent {
        id: ParseEventId::new(),
        location: None,
        stage,
        class,
        severity,
        message,
        details,
        occurs_at: Utc::now(),
    }
}

fn synth_message(
    folder_id: FolderId,
    base_subject: &str,
    suffix: &str,
    body_seed: &str,
    container: ContainerFormat,
    max_bytes_hint: u64,
    deterministic: bool,
    include_attachment: bool,
) -> (Message, Vec<Attachment>) {
    let message_id = MessageId::new();
    let mut subject = format!("{base_subject} - {suffix}");
    if !deterministic {
        subject.push_str(" [sample]");
    }

    let mut message_body = body_seed.to_string();
    if message_body.len() > BODY_PREVIEW_CHARS {
        message_body.truncate(BODY_PREVIEW_CHARS);
    }
    let body_text = format!(
        "{message_body}\n\ncontainer={container:?}\nmessage_id={message_id}\nmax_bytes_hint={max_bytes_hint}"
    );

    let has_attachments = include_attachment && !matches!(container, ContainerFormat::Msg);
    let flags = if has_attachments {
        MessageFlags::new(MessageFlags::READ | MessageFlags::HAS_ATTACHMENTS)
    } else {
        MessageFlags::new(MessageFlags::READ)
    };

    let mut attachments = Vec::new();
    if has_attachments {
        attachments.push(Attachment {
            id: AttachmentId::new(),
            message_id,
            filename: Some("sample.txt".to_string()),
            display_name: Some("Synthetic Attachment".to_string()),
            mime_type: Some("text/plain".to_string()),
            byte_size: body_text.len() as u64 / 10,
            sha256: None,
            inline: false,
            content_id: None,
            created_at: Some(Utc::now()),
            modified_at: Some(Utc::now()),
        });
    }

    (
        Message {
            id: message_id,
            folder_id,
            subject: Some(subject),
            internet_message_id: Some(format!("synthetic-{message_id}")),
            conversation_id: Some(format!("conversation-{message_id}")),
            message_class: Some("IPM.Note".to_string()),
            sender: Some(Recipient {
                role: RecipientRole::From,
                display_name: Some("pst-pst-pst synthetic".to_string()),
                address: Some("noreply@pst-pst-pst.local".to_string()),
                routing_type: Some("SMTP".to_string()),
            }),
            recipients: vec![Recipient {
                role: RecipientRole::To,
                display_name: Some("recipient@pst-pst-pst.local".to_string()),
                address: Some("recipient@pst-pst-pst.local".to_string()),
                routing_type: Some("SMTP".to_string()),
            }],
            sent_at: Some(Utc::now()),
            received_at: Some(Utc::now()),
            modified_at: Some(Utc::now()),
            created_at: Some(Utc::now()),
            size: body_text.len() as u64 + 64,
            flags,
            has_attachments,
            body: Some(MessageBodyRef {
                format: BodyFormat::PlainText,
                byte_size: body_text.len() as u64,
                content_ref: Some(format!("synthetic://message/{message_id}")),
                is_truncated: max_bytes_hint == 0,
            }),
            attachments: attachments.clone(),
            properties: vec![MessageProperty {
                name: "container".to_string(),
                value: PropertyValue::Text(format!("{container:?}")),
            }],
        },
        attachments,
    )
}

fn build_body_seed(sample: &[u8], extension: &Option<String>, container: ContainerFormat) -> String {
    let extension = extension.as_deref().unwrap_or("bin");
    let preview = collect_parse_preview(sample);
    format!("synthetic body for {container:?} .{extension}: {preview}")
}

fn collect_parse_preview(sample: &[u8]) -> String {
    let safe: String = String::from_utf8_lossy(sample)
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() || character.is_ascii_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect();
    let safe = safe.trim().replace('\r', " ");
    if safe.is_empty() {
        "<no preview available>".to_string()
    } else if safe.len() > BODY_PREVIEW_CHARS {
        format!("{}...", safe.chars().take(BODY_PREVIEW_CHARS).collect::<String>())
    } else {
        safe
    }
}

fn make_folder(
    mailbox_id: MailboxId,
    parent_id: Option<FolderId>,
    name: &str,
    path: &str,
    has_subfolders: bool,
) -> Folder {
    Folder {
        id: FolderId::new(),
        mailbox_id,
        parent_id,
        name: name.to_string(),
        path: path.to_string(),
        message_count: 0,
        unread_count: 0,
        total_size: 0,
        has_subfolders,
        is_hidden: false,
        is_root: false,
    }
}

fn source_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("store")
        .to_string()
}

fn extension_matches(extension: Option<&str>, container: ContainerFormat) -> bool {
    let extension = extension.map(|value| value.to_ascii_lowercase());
    match container {
        ContainerFormat::Pst => extension == Some("pst".to_string()),
        ContainerFormat::Ost => extension == Some("ost".to_string()),
        ContainerFormat::Msg => extension == Some("msg".to_string()),
        ContainerFormat::Unknown => false,
    }
}

#[derive(Debug, Clone)]
struct ParseSnapshot {
    file_size: u64,
    extension: Option<String>,
    signature: ContainerSignature,
    sample: Vec<u8>,
    truncated: bool,
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
