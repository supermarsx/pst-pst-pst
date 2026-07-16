//! Terminal-oriented UI primitives for command dispatch, state, and rendering.
//! This crate provides lightweight contracts and helper implementations with
//! optional rich/plain output.

use pst_pst_pst_core::{CommandPayload, CoreError, CoreResult};

/// UI module result alias shared across implementations.
pub type UiResult<T> = CoreResult<T>;
/// UI module error alias.
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
    pub mode: UiMode,
    pub output: UiOutput,
    pub deterministic: bool,
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
    pub raw: String,
    pub kind: UiCommandKind,
    pub args: Vec<String>,
}

/// Canonical command kinds supported by terminal UI.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UiCommandKind {
    Help,
    Info,
    Folders,
    Messages,
    Search,
    Extract,
    Export,
    Validate,
    Index,
    Watch,
    Quit,
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
    pub config: UiConfig,
    pub command_counter: u64,
    pub command_history: Vec<String>,
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
    Text(String),
    Core(CommandPayload),
}

/// Command execution result shape.
#[derive(Debug, Clone)]
pub struct UiCommandResult {
    pub command_id: u64,
    pub exit: bool,
    pub status: Option<String>,
    pub payload: Option<UiPayload>,
}

/// Hook point for concrete command implementations.
pub trait UiCommandBus {
    type Error;

    /// Handle a parsed UI command.
    fn execute(
        &mut self,
        state: &UiState,
        command: &UiCommand,
    ) -> Result<UiCommandResult, Self::Error>;
}

/// Hook point for rendering events.
pub trait UiRenderer {
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
                            _ => 0,
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
pub trait UiRuntime {
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
