# Specification: `pst-pst-pst`

## 0) Purpose

`pst-pst-pst` is a high-performance, native Rust application for reading and working with Outlook storage files:

- `.pst`
- `.ost`
- `.msg`

The project provides:

- a machine-friendly CLI (`pst-pst-pst`)
- a native desktop UI shell
- parity for parsing, filtering, search semantics, and export behavior across both surfaces

This specification is the canonical definition for v1 implementation.

## 1) Hard constraints (MUST)

### 1.1 License and policy

- The project is MIT-licensed.
- Public artifacts and contributions are governed by the repository `LICENSE`.

### 1.2 No FFI policy

- Parsing, indexing, search, and export logic in runtime path must be Rust-native.
- No mandatory `*-sys`, COM, Outlook SDK, or wrapper-based dynamic native dependencies in core flows.
- CLI/UI and parser contracts must remain buildable with pure Rust dependencies.

### 1.3 Platform

- Cross-platform support is required for Linux, Windows, and macOS.
- Build and run behavior must be deterministic across supported platforms.

### 1.4 Performance baseline

- Parallel execution is required by default for heavy compute and I/O stages.
- `--single-thread` must provide a deterministic serial fallback.
- Defaults for job sizing must be configurable and bounded.

### 1.5 Naming conventions

- Crate names use `pst-pst-pst-*` naming.
- Source/manifest filenames are lower-case where practical.
- Export/index artifacts should default to lower-case naming and stable extension conventions.

## 2) Architecture

The workspace is split into five layers:

1. `crates/core`
   - command contracts
   - shared domain types
   - result payloads and error taxonomy
2. `crates/parser`
   - backend registry and container discovery
   - pst/ost/msg extraction contracts
3. `crates/index`
   - index build interfaces
   - search query/result metadata
4. `crates/export`
   - export payload contracts
   - checkpoints/progress/chunk scheduling
5. `crates/cli` and `crates/ui`
   - user interfaces
   - parser/index/export orchestration

## 3) Parsing contract

### 3.1 File detection and backend selection

1. Detect file type by:
   - extension
   - OLE/compound signature checks
2. If requested container is specified, prefer that backend.
3. If not specified, use confidence-based probe with fallback candidates.
4. On mismatch and fallback disabled: return typed unsupported container error.

### 3.2 Backend requirements

- `.pst` and `.ost` containers are parsed by the same underlying local parser strategy, with format-aware branching.
- `.msg` uses Rust-native message parser path.
- Backend errors are mapped into:
  - I/O parse errors
  - per-item decode errors
  - recoverable/continuable item errors
- Parsing must produce:
  - `Mailbox`
  - `Folder` tree
  - `Message`
  - `Attachment`
  - `ParseEvent` stream

### 3.3 Recovery behavior

- Non-fatal item corruption must produce a warning event and continue traversal.
- A strict mode is optional and aborts according to configured policy.
- Parse summary includes:
  - recovered item count
  - fatal count
  - elapsed durations
  - event list

## 4) Multi-threading model

### 4.1 Pipeline stages

1. Discovery stage
2. Decode stage
3. Normalize stage (body, recipients, metadata)
4. Index/scan stage
5. Export/export-checkpoint stage

### 4.2 Resource controls

- `--jobs` defines global worker budget.
- `--io-jobs` limits filesystem read/write workers.
- `--cpu-jobs` limits decode/index workers.
- `--single-thread` collapses to one worker in each stage.

### 4.3 Deterministic behavior

- Concurrent output must be ordered deterministically when `--deterministic` is set.
- Stable sort/tie-breaker requirements:
  - primary key: message ID
  - secondary key: source order within parse chunk
  - deterministic cursor across resume boundaries

### 4.4 Backpressure

- All asynchronous producer-consumer paths use bounded buffers.
- Memory pressure is controlled by:
  - bounded queues
  - bounded batch sizes
  - periodic checkpointing

## 5) Search contract (“everything in between”)

The product must support both indexed and non-indexed strategies with explicit provenance.

### 5.1 Modes

`full`, `indexed`, `hybrid`, `auto` are first-class.

- `full`: direct scan only, no index dependency.
- `indexed`: query persisted index only.
- `hybrid`: indexed candidate list + final verification scan.
- `auto`: planner selects strategy based on index validity, staleness, and policy.

### 5.2 Index policy

- `allow`: prefer index, fallback according to planner.
- `require`: return explicit error when index is missing or stale.
- `build`: run/prepare index before query when needed.
- `refresh`: refresh stale index, then execute query.

### 5.3 Search output contract

All modes return a unified schema:

- `SearchResult`
  - `mailbox_id`
  - `hits[]`
  - `total`
  - `returned`
  - `query`
  - `source_mode`
  - `include_unindexed`
  - `deterministic`
  - `page`
- each hit includes:
  - `message_id`
  - `folder_id`
  - `score`
  - `match_source` (`full`, `indexed`, `hybrid`)
  - `matched_fields`
  - optional `snippet`

### 5.4 Acceptance matrix

- `full` must succeed without index artifacts.
- `indexed` must enforce `--index-policy` for missing/stale index.
- `hybrid` must confirm matches through scan pass for candidate verification.
- `auto` must never produce incorrect mode claim in output.

## 6) Export contract

### 6.1 Formats

- `eml`
- `mbox`
- `json`
- `jsonl`

### 6.2 Behavioral requirements

- safe path resolution (no traversal escape)
- deterministic naming with `--deterministic`
- resumable progress via checkpoints
- attachment handling through digest-based dedupe when enabled
- manifest includes completed/skipped/failed summary

## 7) CLI behavior

### 7.1 Required command set

- `info`
- `folders`
- `messages`
- `search`
- `extract`
- `export`
- `validate`
- `index`
- `watch`
- `ui`

### 7.2 Command output behavior

- `table`: terminal presentation.
- `json`/`jsonl`/`ndjson`: machine format for automation.
- exit status must reflect command outcome category (`parse`, `io`, `index`, `export`, etc.).

### 7.3 Filter language parity

- Query parser is shared across CLI/UI.
- Unsupported expressions produce actionable parse errors with location/cursor context.

## 8) UI behavior contract

- Folder tree and message list navigation.
- Message and body preview.
- Search bar with selectable mode (`auto|full|indexed|hybrid`).
- Export action queue with cancel/retry semantics.
- Real-time progress and diagnostics channel.
- Shared command vocabulary with CLI.

## 9) Data and error model

- Errors follow `CoreError` taxonomy:
  - `Io`
  - `Parse`
  - `Decode`
  - `Integrity`
  - `Index`
  - `Export`
  - `Ui`
  - `Unsupported`
- Parse events are structured and persistable.
- Strict mode and recovery mode are explicitly surfaced.

## 10) Non-goals

- No Outlook/cloud account management.
- No write-modify operations on source files.
- No mandatory external runtime dependency outside Rust ecosystem.
- No COM or native desktop automation as required pipeline input.

## 11) Quality and acceptance targets

### 11.1 Reliability

- Partial failures must be reported, not silently dropped.
- Corrupt nodes/messages continue when possible.
- Deterministic and strict modes are explicitly tested.

### 11.2 Performance

- Throughput improvements expected from concurrency and bounded scheduling.
- Interaction latency targets are enforced in UI profile checks.
- Memory footprint bounded through queue limits and streaming pipelines.

### 11.3 Verification

- Unit tests for core domain and query types.
- Integration tests for sample `.pst`, `.ost`, `.msg` fixtures.
- Benchmark gates for parse/index/search regression.
- Search parity tests across modes and policy combinations.

## 12) Milestones

### Milestone 1: foundation

- CLI and parser bootstrap wired
- no-FFI and policy enforcement implemented

### Milestone 2: command parity

- `info`, `folders`, `messages`, `validate`, `search`
- strict and deterministic modes

### Milestone 3: index/search

- indexed and hybrid search available
- planner + policy controls implemented

### Milestone 4: export + watch

- export checkpoints and manifests
- watch automation and durable command handling

### Milestone 5: UI parity

- native shell parity for CLI command set and search schema
- progress, diagnostics, and deterministic execution paths
