# pst-pst-pst

High-performance, cross-platform tooling for Outlook store and message inspection in Rust.

`pst-pst-pst` provides:

- native CLI workflows over `.pst`, `.ost`, and `.msg`
- a native desktop UI shell with shared query and export semantics
- high-throughput traversal and search with full/no-index/hybrid strategies
- deterministic, reproducible exports and reporting

All runtime code is designed around three project-level guarantees:

- **MIT license** for redistribution and contributions.
- **No FFI in core paths** (native Rust parser/index/export stack only).
- **Default concurrency** for heavy work, with `--single-thread` as an explicit fallback.

## Why this project

The tool is intended for investigators, data engineers, and automation pipelines that need to work with Outlook store formats without Outlook itself, without cloud APIs, and without proprietary native bridges.

## Workspace design

This repository is intentionally split into focused crates:

- `crates/core`: shared domain model, command/result contracts, and execution traits.
- `crates/common`: reusable configuration, logging, and utility policy types.
- `crates/parser`: `.pst`, `.ost`, `.msg` parsing backends.
- `crates/index`: search index contracts and index build state models.
- `crates/export`: export manifests, checkpoints, and engine traits.
- `crates/ui`: native UI control types and command bus abstractions.
- `crates/cli`: command surface + orchestration across parser/index/ui/export.

Crate naming is aligned to `pst-pst-pst-*` with Rust package imports under `pst_pst_pst_*`.

## Platform and safety constraints

- Linux, Windows, and macOS supported.
- Source files and generated artifacts must remain lower-case by convention where practical.
- Read-only source mode is the default for parser workflows.
- No Outlook/Exchange APIs, COM automation, or proprietary runtime DLL/SO dependencies.
- Audit-friendly structured output and deterministic mode for reproducible runs.

## Runtime execution model

`pst-pst-pst` is multithreaded by default:

- `--jobs`: global concurrency budget
- `--io-jobs`: bounded I/O worker pool size
- `--cpu-jobs`: bounded CPU/compute worker pool size
- `--single-thread`: force serial execution path

When `--single-thread` is set, all heavy paths execute in bounded serial order.

## CLI interface

```text
pst-pst-pst <global options> <command> [command options]
```

### Global options

- `--jobs <N>`
- `--io-jobs <N>`
- `--cpu-jobs <N>`
- `--single-thread`
- `--deterministic`
- `--strict`
- `--output <table|json|jsonl|ndjson>`
- `--container <pst|ost|msg>`
- `--config <path>`
- `--log-level <error|warn|info|debug|trace>`
- `--log-json`
- `--yes`
- `--quiet`
- `--no-color`

### Commands

- `info <file>`
  - Open a container and emit high-level metadata + health diagnostics.
- `folders <file>`
  - Traverse and print folder trees with message counts.
- `messages <file>`
  - List messages with filters and paging.
- `search <file> --q <query>`
  - Search messages by query expression.
  - `--search-mode auto|full|indexed|hybrid`  
  - `--index-policy allow|require|refresh|build`
  - `--include-unindexed`
- `extract <file> --message-id <id> --out <dir>`
  - Export one message, with optional attachment selector.
- `export <file> --format eml|mbox|json|jsonl --out <dir>`
  - Bulk export with checkpoint metadata.
- `validate <file> [--report report.json]`
  - Emit parse/validation diagnostics.
- `index <file> [--db <path>] [--rebuild]`
  - Build or refresh local search index state.
- `watch <directory> --pattern <glob> --on-changed "<command>"`
  - Directory automation integration.
- `ui`
  - Launch native UI shell.

## Search modes

`pst-pst-pst` supports all four execution modes with a single result schema and provenance:

- `full`
  - No index dependency.
  - Highest correctness, higher CPU cost.
- `indexed`
  - Uses existing index artifacts only.
  - Fast for repeated queries and stable corpora.
- `hybrid`
  - Candidate IDs from index + full-text verification for correctness.
  - Optional non-indexed region handling when enabled.
- `auto`
  - Chooses indexed path when index health/quality allows it; otherwise full scan fallback.

Every hit includes:

- message + folder IDs
- matched fields
- optional snippet
- `match_source` (`full`, `indexed`, `hybrid`)

## Filter language

Filters are shared between CLI and UI:

`field:operator:value`

Supported operators:

- `=`, `!=`, `<`, `<=`, `>`, `>=`
- `~`, `!~`, `between`
- boolean operators: `and`, `or`, `not`, grouped with parentheses

Examples:

- `received>=2026-01-01`
- `subject~"invoice"`
- `sender="alice@contoso.com"`
- `has-attachment=true`
- `size<=10485760`

## Example usage

```bash
# container summary
pst-pst-pst info ./Mailbox.pst

# folder listing
pst-pst-pst folders ./Mailbox.ost --folder /Inbox --limit 500

# search with deterministic ranking
pst-pst-pst search ./Mailbox.pst --q "from:alice@contoso.com subject~quarterly" --search-mode auto --deterministic

# full export
pst-pst-pst export ./Mailbox.pst --format eml --out ./out --deterministic --jobs 8

# watch a mailbox drop folder
pst-pst-pst watch ./incoming --pattern "*.pst" --on-changed "pst-pst-pst index {path} --db ./var/index"
```

## Output modes

- `table`: human-readable terminal rendering
- `json`: single object for command payloads
- `jsonl` / `ndjson`: streaming machine records

## Configuration

`~/.config/pst-pst-pst/config.toml`

```toml
[runtime]
jobs = 8
io_jobs = 4
cpu_jobs = 4
single_thread = false

[search]
default_mode = "auto"
default_index_policy = "allow"

[output]
prefer_jsonl = true
line_buffer = 64000
```

## Performance and resilience targets

- Bounded worker pools for both I/O and compute.
- Stable ordering when `--deterministic` is enabled.
- Tolerant traversal with per-item failure events (best-effort processing).
- Path-safe export and manifest checkpointing for large batches.

## UI behavior goals

- Folder navigator + message list
- Message and header preview
- Attachment list and export actions
- Search mode toggle (`auto/indexed/full/hybrid`)
- Progress + diagnostics stream

## License

`pst-pst-pst` is MIT licensed. See [`LICENSE`](LICENSE).

## Status and specification

- Technical requirements and acceptance criteria: [`spec.md`](spec.md)
- Roadmap and execution checklist: [`docs/roadmap.md`](docs/roadmap.md), [`docs/implementation-checklist.md`](docs/implementation-checklist.md)
