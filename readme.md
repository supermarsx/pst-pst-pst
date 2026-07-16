# pst-pst-pst

`pst-pst-pst` is a high-performance, cross-platform, **native Rust** toolkit for reading, searching, validating, exporting, and automating workflows over Outlook container formats (`.pst`, `.ost`, `.msg`).

The project ships a terminal-first CLI and a native terminal UI surface built on the same contracts so every operation can move between interactive and scripted workflows without semantic drift.

## Positioning

- **No FFI**: runtime parser/index/export paths are implemented in Rust with no `COM`, `libpst`-style runtime bindings, or foreign-process protocol.
- **Cross-platform**: Windows, macOS, Linux first-class.
- **Multithreaded by default**: concurrency is used for I/O parsing and CPU search/index paths unless `--single-thread` is set.
- **Deterministic mode**: explicit, reproducible outputs where needed.
- **License**: MIT.

## Project layout

- `crates/core`: stable domain types (`Mailbox`, `Folder`, `Message`, `Attachment`, results, errors).
- `crates/common`: shared configuration and utility primitives.
- `crates/parser`: container detection and native backend orchestration for PST/OST/MSG.
- `crates/index`: in-memory and persisted index engine with query planning.
- `crates/export`: export workers and checkpoint manifests.
- `crates/ui`: terminal session protocol and runtime state.
- `crates/cli`: orchestrator that binds parser/index/export/ui into one CLI.

Crate names are aligned with the repository name (`pst-pst-pst-*`) and code targets lower-case module filenames and docs filenames where practical.

## Core guarantees

1. **No FFI runtime path** in parser/index/search/export.
2. **Read-only source handling** by default (no writes to source containers unless explicitly required by operations).
3. **Cross-platform determinism** with explicit concurrency controls.
4. **Multi-target compatibility** for `.pst`, `.ost`, and `.msg` containers.
5. **Structured diagnostics** for automated auditing.

## Performance model

Threading defaults are split into budgets rather than one global switch:

- `--jobs`: global worker budget (defaults to available parallelism).
- `--io-jobs`: dedicated I/O workers.
- `--cpu-jobs`: dedicated CPU workers for index/search phases.
- `--single-thread`: disables worker fan-out for deterministic or minimal memory runs.

All heavy paths should be bounded and cancel-friendly. Every long running operation should emit progress and carry an operation context.

## Search: full-text + indexed + everything in between

`pst-pst-pst` supports search in four modes and surfaces the chosen effective mode in output metadata.

| Mode | Description |
| --- | --- |
| `full` | Direct scan over parsed message records without index prerequisites. |
| `indexed` | Query using prebuilt index state only. |
| `hybrid` | Candidate narrowing via index + full-text verification pass. |
| `auto` | Planner chooses mode from index health and command policy. |

Index policy flags:

- `allow` (default): use index when present, otherwise fallback.
- `require`: error when index health/availability requirements are not met.
- `build`: build index before search when missing.
- `refresh`: refresh stale index before execution.

## Command surface

The CLI name is `pst-pst-pst`.

```text
pst-pst-pst [global options] <command> [command options]
```

### Global options

- `--jobs <N>`
- `--io-jobs <N>`
- `--cpu-jobs <N>`
- `--single-thread`
- `--deterministic`
- `--strict`
- `--container <pst|ost|msg|auto>`
- `--output <table|json|jsonl|ndjson>`

### Commands

- `info <source>`
  - Print container metadata, backend details, parse diagnostics.
- `folders <source>`
  - Enumerate mailbox folders with filter and pagination.
- `messages <source>`
  - List message metadata with optional field projection and paging.
- `search <source> --q <query>`
  - Run full/indexed/hybrid/auto search.
  - `--search-mode`, `--index-policy`, `--max-results`, `--include-unindexed`, `--fields`, `--page`.
- `extract <source> [--message-id | --attachment-id] --out <path>`
  - Export a single message or attachment.
- `export <source> --format <eml|mbox|json|jsonl> --out <path>`
  - Export selected/all messages and optional attachments.
- `validate <source> [--report <path>]`
  - Emit parse integrity checks and degraded-failure details.
- `index <source> [--db <path>] [--rebuild]`
  - Build or refresh index state.
- `watch <directory> --pattern <glob> --on-changed "<command>"`
  - Filesystem automation mode with bounded trigger semantics.
- `ui`
  - Native terminal interface with command parity to CLI.

## Structured output model

CLI output can be rendered in:

- Human-readable table output (`table`)
- JSON object (`json`)
- Line-streamed records (`jsonl`, `ndjson`)

Search-related outputs include provenance fields such as `source_mode`, `match_source`, `include_unindexed`, and `deterministic`, so downstream automation can reason about correctness and latency tradeoffs.

## Running from source

### Prereqs

- Rust toolchain (MSRV 1.70+)
- Git

### Build and run

```bash
cargo build --workspace --release
cargo run --package pst-pst-pst-cli -- --help
cargo run --package pst-pst-pst-cli -- --jobs 8 info ./Mailbox.pst
```

### Install

```bash
cargo install --path . --bin pst-pst-pst
```

## Example workflows

```bash
# Introspect store metadata
pst-pst-pst info ./cases/enterprise.pst

# Directory audit for all folders and messages
pst-pst-pst folders ./cases/desktop.ost --limit 250
pst-pst-pst messages ./cases/desktop.ost --limit 400 --fields id subject sender received_at

# Full/Indexed/Hybrid search matrix
pst-pst-pst search ./cases/desktop.pst \
  --q "subject:quarterly sender:ops@company.com" \
  --search-mode indexed \
  --index-policy allow \
  --max-results 50

pst-pst-pst search ./cases/desktop.pst \
  --q "body~forensic body~timeline" \
  --search-mode hybrid \
  --include-unindexed \
  --deterministic

# Build and refresh indexes
pst-pst-pst index ./cases/desktop.pst --db ./var/index --rebuild
pst-pst-pst search ./cases/desktop.pst --q "from:alice@corp.com" --search-mode auto --index-policy build

# Deterministic export run
pst-pst-pst export ./cases/desktop.pst \
  --format eml \
  --out ./exports/desktop-2026-07 \
  --deterministic

# Automation watch loop
pst-pst-pst watch ./incoming --pattern "*.pst" --on-changed "pst-pst-pst index {path} --rebuild"
```

## Why no FFI?

This project avoids foreign parser runtimes to keep build and deployment predictable, reduce transitive risk, and keep behavior transparent across all supported OS families.

## Security and safety defaults

- No container writes unless an export command is explicit.
- Export output writes are sanitized and scoped to destination roots.
- Structured errors are typed by domain class (IO, parse, index, export, etc.).

## Project documents

- [Formal specification](spec.md)
- [Roadmap](docs/roadmap.md)
- [Implementation checklist](docs/implementation-checklist.md)

## License

This repository is MIT licensed. See [`license`](license).