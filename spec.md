# Specification: `pst-pst-pst`

## 0. Vision and scope

`pst-pst-pst` is a high-performance Rust-native platform for ingesting and operating on Outlook container formats (`.pst`, `.ost`, `.msg`) through one shared execution contract. It provides:

- A scriptable CLI (`pst-pst-pst`).
- A terminal-native interactive shell surface (`pst-pst-pst ui`) using the same command contracts.
- Native parser/index/export behavior with no FFI in core paths.
- Multi-threading for heavy CPU and I/O operations by default.

The intended operators are digital examiners, security teams, SREs, and automation teams that need predictable offline mailbox analysis.

## 1. Non-negotiable requirements

### 1.1 License and distribution

- The project is released under the MIT license (`license`).
- All crates and tooling artifacts inherit this licensing and include license notices where distribution outputs are produced.

### 1.2 No-FFI policy

- There must be no hard dependency on COM, MAPI, proprietary runtime SDKs, or runtime bridge crates in parser/index/export/search execution.
- Native code paths must remain pure Rust at runtime.
- FFI-adjacent crates are only allowed if they do not participate in runtime execution and are gated for non-production tooling.
- Build/test workflows must be able to validate policy conformance.

### 1.3 Cross-platform correctness

- Supported hosts: Windows, macOS, Linux.
- Binary behavior must remain deterministic across hosts for equivalent inputs, options, and data.
- Any host-specific fallback behavior is explicit and documented.

### 1.4 Multi-threading baseline

- All compute-heavy and I/O-heavy stages run through thread pools by default.
- `--single-thread` disables parallel scheduling and forces serial deterministic execution for diagnosability.

### 1.5 Concurrency safety

- All public task runners for index/search/export expose `Send + Sync` constraints.
- Shared states use message passing or lock-safe structures.
- Bounded backpressure is required where work can outpace consumers.

### 1.6 Filename hygiene

- New file and directory outputs use lower-case naming patterns where practical.
- Repository documentation filenames are lower-case by convention (`spec.md`, `docs/*.md`).

## 2. System architecture

### 2.1 Domain layers

1. **CLI layer** (`crates/cli`) parses commands and delegates into shared application services.
2. **Core layer** (`crates/core`) defines stable contracts, identifiers, commands, outputs, and typed errors.
3. **Parser layer** (`crates/parser`) normalizes container contents into `ParsedStore` models.
4. **Index layer** (`crates/index`) builds searchable structures and query plans.
5. **Export layer** (`crates/export`) performs deterministic materialization to common mailbox formats.
6. **UI layer** (`crates/ui`) exposes terminal interactive state synchronized with CLI behavior.

### 2.2 Cross-layer contract

All layers must preserve these invariants:

- A command submitted by UI or CLI maps to a single core command schema.
- Search and export results carry a stable schema across transport surfaces.
- Every non-trivial command returns enough metadata to support retry, reproducibility, and auditing.
- Every command returns a typed completion signal plus per-category error classes.

## 3. Parser requirements (PST/OST/MSG)

### 3.1 Input discovery and backend selection

1. Input is discovered from extension and signature.
2. Backend candidates are generated with confidence scores.
3. The backend with strongest confidence is selected unless unsupported/ambiguous.
4. Unsupported formats terminate with typed error (`unsupported` category).

Supported container mappings:

- `.pst` -> PST backend
- `.ost` -> OST backend
- `.msg` -> MSG backend

### 3.2 Parsing behavior

- Parse output is normalized into shared core types (`Mailbox`, `Folder`, `Message`, `Attachment`, `ParseEvent`).
- Parsing MUST be incremental and recoverable:
  - Non-fatal item/corruption errors become warnings.
  - Strictness mode controls when recoverables are escalated to terminal errors.
- Metadata extraction MUST include IDs, subject, sender/recipients, timestamps, sizes, and folder path when available.

### 3.3 Synthetic parse behavior

When a container is valid but partially unknown, parser MAY emit synthetic diagnostics and synthetic records according to implemented strategy, as long as provenance is explicit in outputs.

### 3.4 Output and recovery constraints

- Parser outputs must include parse telemetry and event streams.
- Parse telemetry must distinguish warning/fatal and preserve fault context fields.
- If strict mode is enabled, recoverable faults may still fail according to command policy.

## 4. Search and query contract

Search in this system is designed as a spectrum from low-latency index paths to exact full scans.

### 4.1 Modes

#### full
- Scan parsed records only.
- Guarantees no index precondition.
- Highest correctness under incomplete index state.

#### indexed
- Query against prebuilt search records only.
- Requires index availability and configured freshness.
- Lowest latency when healthy.

#### hybrid
- Use index candidate generation.
- Verify candidate hits through full-content checks.
- Return mode and source metadata for each hit.

#### auto
- Planner chooses mode based on:
  - requested mode constraints,
  - index policy,
  - freshness,
  - include_unindexed flag,
  - resource profile.

### 4.2 Planner semantics

The planner MUST expose effective mode in result metadata. Auto-planning rules include:

1. If `--search-mode full`, effective mode is always full.
2. If `--search-mode indexed`, require policy pass or fail fast.
3. If `--search-mode hybrid`, return hybrid when index is present; degrade to full only when explicitly allowed.
4. If `--search-mode auto` and index policy is:
   - `require`: must use index and fail if unavailable or stale.
   - `allow`: use index when valid, otherwise full.
   - `build`: build missing/invalid index before execution.
   - `refresh`: rebuild/refresh stale index and continue.

### 4.3 Query model

- Query parsing MUST be deterministic with clear error positions on invalid expressions.
- Query language includes field-level operations and boolean composition.
- Result ranking is deterministic in `--deterministic` mode using stable tie-breakers.

### 4.4 Full-text + indexed parity

Both search paths must emit the same result schema and metadata fields. If a hit path differs in source, this is explicit via `match_source` and `source_mode` fields. Hybrid mode must preserve correctness against indexed mode by verifying candidates.

## 5. Index model

### 5.1 Document model

Index documents represent message-level entities with fields:

- identity fields: mailbox, folder, message IDs
- contact fields: sender, recipients, cc/bcc, subject
- temporal fields: received, sent, created
- content fields: subject, body, attachment names
- metadata fields: size, has_attachment, flags, hash, version markers

### 5.2 Build behavior

- Builds support incremental updates, idempotent upsert semantics, and deterministic ordering controls.
- Builders use parallel workers when message volume justifies partitioned execution.
- Build state must be observable (`queued`, `running`, `done`, `failed`) and include elapsed wall time.

### 5.3 Storage and checkpointing

- Indexes are persisted with generation metadata and optional build fingerprint.
- Stale indexes are detected and handled via policy (`refresh`/`require`/`build`).
- Checkpoint artifacts must identify source and build identity.

### 5.4 Query execution

- Query execution can parallelize by shard/candidate-set partitioning in non-deterministic mode.
- Deterministic mode disables non-stable ordering and enforces canonical tie-breakers.

## 6. Export contract

### 6.1 Supported targets

- `eml`
- `mbox`
- `json`
- `jsonl`

### 6.2 Export semantics

- Export command MUST produce deterministic naming and ordering when `--deterministic` is active.
- Exports for repeated runs on unchanged input SHOULD be idempotent in deterministic mode.
- Export manifest contains run status (`requested`, `completed`, `skipped`, `failed`) and strictness mode.

### 6.3 Safety requirements

- Output path resolution is canonical and bounded to destination root.
- Attachment filenames are sanitized.
- Partial writes must never corrupt completed artifacts.

## 7. CLI contract and output behavior

### 7.1 Global CLI contract

- Commands and outputs are stable and typed.
- Output channels:
  - table (human)
  - json
  - jsonl/ndjson (streaming)
- Exit codes map to command category and failure class.

### 7.2 Core commands

- `info`, `folders`, `messages`, `search`, `extract`, `export`, `validate`, `index`, `watch`, `ui`.
- UI must support the same semantic defaults as CLI.

### 7.3 Validation and diagnostics

- Strict and non-strict modes are explicit per command and exported in command metadata.
- Error diagnostics include domain category and actionable hints where possible.

## 8. UI contract

### 8.1 Shared command schema

- UI emits the same command identifiers and payload shape as CLI.
- Query/search filter representation is compatible with CLI parser semantics.

### 8.2 UX requirements

- Non-blocking input and update loop.
- Progress events for long jobs.
- Reusable session state and command history.
- No external UI runtime dependency beyond native terminal execution layer.

## 9. Concurrency and performance requirements

### 9.1 Throughput

- Default concurrency must scale with CPU core count while avoiding starvation.
- I/O and CPU budgets must be independently tunable.
- Index/search pipelines should maintain stable throughput under large container sets.

### 9.2 Resource control

- Use bounded queues for producer/consumer stages.
- Track backpressure and cap in-memory materialization where practical.
- Large attachment and body reads should use streaming or bounded chunking.

### 9.3 Reproducibility targets

- `--deterministic` mode must produce stable ordering and stable pagination tokens across runs.
- Deterministic mode applies to indexing, search ranking, export pathing, and report summaries.

## 10. Error and telemetry model

All commands use typed error classes:

- IO
- Parse
- Decode
- Integrity
- Index
- Export
- UI
- Unsupported
- InvalidInput

Each class SHOULD map to structured terminal and machine output. Telemetry includes operation ID, command, source, and resource profile.

## 11. Security model

- No credential extraction or secret persistence.
- No implicit writes to source containers.
- No mutation of source by default.
- Export path controls prevent directory traversal and symlink attacks where practical.

## 12. Acceptance criteria

### 12.1 Functional

- `info`, `folders`, `messages`, `search`, `extract`, `export`, `validate`, `index`, `watch`, `ui` are command-present with shared contract behavior.
- `.pst`, `.ost`, `.msg` paths are resolved and mapped correctly by backend selection.
- Search planner output reports effective mode and match source for each hit.
- Full/indexed/hybrid/auto modes produce coherent and auditable result schema.
- Export manifest and report outputs are always parseable.

### 12.2 Non-functional

- No-FFI remains true in runtime path across parser/index/export.
- Multithreading defaults are used on all non-trivial runs.
- Deterministic mode yields stable, replayable ordering.
- Performance remains bounded and degrades predictably under stress.

## 13. Release and operations hardening

- Documentation (`README`, `spec`, roadmap, checklist) must stay in sync with implementation status.
- CI and packaging should validate dependency policy and platform matrix.
- Performance and correctness tests should include malformed containers and large corpora.