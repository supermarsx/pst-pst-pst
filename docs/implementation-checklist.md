# Implementation Checklist

This checklist is prioritized by domain and structured for execution planning. It is aligned to `README.md`, `spec.md`, and `docs/roadmap.md`.

### Global hard requirements

- **License target:** MIT.
- **No-FFI:** no native bindings, no COM, no `*-sys` parser/runtime dependencies, and no FFI in core/CLI/UI paths.
- **Default threading:** multithreaded execution is required for parse/index/export/search-heavy paths unless `--single-thread` is active.
- **Search contract:** CLI and UI must expose `full`, `indexed`, `hybrid`, and `auto` modes with equivalent result schemas.
- **Parity target:** CLI and UI must share parser, filter/query language, and export semantics.

## Legend
- **Priority**
  - `P0` ? must complete before first usable build
  - `P1` ? required for v1 baseline
  - `P2` ? required for robustness/perf quality
  - `P3` ? improvement / polish
- **Type**
  - `Must` = required behavior
  - `Nice` = desirable enhancement for v1.x

## 1) Parser

### Must-have

1. `P0` **Must** ? Implement container bootstrap and format detection in Rust-only parsers
- Dependencies: source abstraction and `core` error types in `crates/core`, workspace feature flags for `pst/ost/msg`.
- Acceptance checks:
  - Detect `.pst`, `.ost`, `.msg` by signature + extension fallback.
  - Reject unsupported formats with typed error (`UnsupportedFormat`).
  - Open/read path uses no write operations by default.

2. `P1` **Must** ? Implement tolerant traversal pipeline with bounded concurrency
- Dependencies: parser crate worker pool integration, cancellation tokens, bounded channels, `--single-thread` override.
- Acceptance checks:
  - Folder/message traversal completes without stack overflow on deep trees.
  - Queue lengths are bounded under load and backpressure is observable.
  - Corrupt item triggers recoverable parse warning and does not abort full run.

3. `P1` **Must** ? Build typed message model extraction (IDs, folder path, metadata fields)
- Dependencies: schema in `core` and filter/query field mapping.
- Acceptance checks:
  - Fields required by `filters` and `search` are available: subject, sender/recipients, dates, flags, has-attachment, size.
  - Missing/invalid fields degrade to explicit nullable defaults without panic.

4. `P2` **Must** ? Implement body/attachment extraction behind explicit request path
- Dependencies: decoder abstraction, MIME/type detection, attachment index.
- Acceptance checks:
  - `messages` command and UI preview can return headers without force-decoding full bodies.
  - Body and attachment bytes are fetched only when requested by UI preview or export command.

5. `P2` **Must** ? Add structured parse diagnostics and per-item failure records
- Dependencies: error taxonomy (`io/parse/decode/integrity`) and event manifest writer.
- Acceptance checks:
  - `validate` emits machine-readable per-item event entries.
  - Non-fatal errors do not crash execution unless `--strict` in strict mode.

### Nice-to-have

6. `P2` **Nice** ? Add parser-level resilience heuristics for partial/legacy variants
- Dependencies: additional fixture corpus and decoding feature flags.
- Acceptance checks:
  - Legacy PST/OST variants parse at parity with modern variants for folder/tree and message headers.

7. `P3` **Nice** ? Add optional property/attachment prefetch hints and decode cache
- Dependencies: bounded LRU cache + cache lifecycle metrics.
- Acceptance checks:
  - Throughput gains in repeated exports from same archive with no change in memory ceiling violation.

## 2) Index/Search

### Must-have

1. `P1` **Must** ? Define shared searchable document schema and indexing pipeline
- Dependencies: parser output contract and `core` filter AST normalizer.
- Acceptance checks:
  - Indexed fields match filter/search fields exactly and are deterministic.
  - Indexer supports resume checkpoints and partial progress writes.

2. `P1` **Must** ? Implement search mode planner (`auto`, `full`, `indexed`, `hybrid`)
- Dependencies: index state metadata (mtime/hash/staleness), scheduler, query parser.
- Acceptance checks:
  - `auto` selects indexed mode when index valid, otherwise full scan.
  - `indexed` refuses stale/missing index and reports actionable reason.
  - `hybrid` returns `match_source=hybrid` and includes scan verification when configured.

3. `P1` **Must** ? Implement deterministic ranking + stable pagination
- Dependencies: canonical sort keys, cursor token format, thread-safe reduction stage.
- Acceptance checks:
  - Same query with same index and options yields stable ordering and repeatable `next` tokens.
  - Deterministic mode forces stable tie-breakers across threads.

4. `P2` **Must** ? Build incremental index build/refresh API
- Dependencies: persistence manifest, transactionally updated db directory structure, staleness policy.
- Acceptance checks:
  - Rebuilding with unchanged source leaves unchanged index fingerprint.
  - Refresh updates only changed sections and reports elapsed wall-clock duration.

### Nice-to-have

5. `P2` **Nice** ? Add snippet and match-highlight generation for indexed mode
- Dependencies: tokenizer and position metadata in index postings.
- Acceptance checks:
  - UI/CLI can render bounded context snippets around first match term.

6. `P3` **Nice** ? Add query plan cache and latency telemetry per query
- Dependencies: cache key on normalized AST + index version + policy options.
- Acceptance checks:
  - Cached plan reuse is visible in logs; p95 latency improvement for repeated equivalent queries.

## 3) CLI

### Must-have

1. `P0` **Must** ? Implement command surface and shared config model
- Dependencies: `crates/cli` command router, `crates/common` config loader.
- Acceptance checks:
  - Commands available: `info`, `folders`, `messages`, `search`, `extract`, `export`, `validate`, `index`, `watch`, `ui`.
  - Config precedence: flags > env > config file > defaults.

2. `P1` **Must** ? Implement output mode support and error envelopes
- Dependencies: formatter module, schema serialization, streaming writer abstraction.
- Acceptance checks:
  - Outputs in table/json/jsonl/ndjson consistently for each command.
  - Machine output is schema-valid and does not block on large streams.

3. `P1` **Must** ? Enforce threading defaults and runtime overrides from CLI
- Dependencies: global options parser, job scheduler, `--single-thread` integration.
- Acceptance checks:
  - Default job count uses CPU core count and enables concurrent stages.
  - `--single-thread` disables thread pools and yields deterministic serial behavior.

4. `P1` **Must** ? Implement filter language parsing shared with UI
- Dependencies: parser for filter grammar in `core` and UI contract.
- Acceptance checks:
  - Parenthesized boolean expressions and operators are equivalent across CLI and UI.
  - Invalid expressions return typed parse errors with cursor position.

5. `P2` **Must** ? Implement streaming progress and cancellation behavior
- Dependencies: progress channel, cancellation token API, signal handling.
- Acceptance checks:
  - Long operations show periodic progress and recover gracefully from SIGINT.
  - Interrupted exports and index builds leave resumable checkpoints.

### Nice-to-have

6. `P2` **Nice** ? Add shell completion and machine-friendly status codes
- Dependencies: arg parser completion API and CLI docs generator.
- Acceptance checks:
  - Bash/Zsh completion includes all commands and major flags.
  - Non-zero and structured error codes map to command category.

7. `P3` **Nice** ? Add `watch` debounced scheduling improvements
- Dependencies: filesystem event abstraction, debounce scheduler.
- Acceptance checks:
  - New/updated stores trigger single coalesced job within configured quiet period.

## 4) UI

### Must-have

1. `P1` **Must** ? Implement UI shell with shared command/query contract
- Dependencies: `core` domain models, query engine integration, cross-platform window bootstrap.
- Acceptance checks:
  - All primary CLI operations have discoverable UI equivalents.
  - No FFI-based UI stack; native Rust UI technology only.

2. `P1` **Must** ? Build folder tree, virtualized message list, and preview panes
- Dependencies: parser streaming stream adapter, list virtualization, attachment preview service.
- Acceptance checks:
  - Render >100k-row virtualized list without full materialization.
  - Message selection updates preview within 250ms under background activity (P95 target).

3. `P2` **Must** ? Implement filter/search toolbar and job status panel
- Dependencies: shared search planner, progress bus.
- Acceptance checks:
  - UI displays mode used (`full/indexed/hybrid/auto`) per result.
  - Search execution does not freeze UI thread.

4. `P2` **Must** ? Implement deterministic export queue with retry/cancel
- Dependencies: export command API, background worker pool, checkpoint manifest API.
- Acceptance checks:
  - Cancelled exports stop within bounded delay and leave a restartable manifest.
  - Deterministic export names remain stable per run order.

### Nice-to-have

5. `P2` **Nice** ? Add timeline view and keyboard shortcut map
- Dependencies: timestamp sorting, keymap registry.
- Acceptance checks:
  - Keyboard shortcuts are discoverable and customizable through settings.

6. `P3` **Nice** ? Add lightweight diagnostics dashboard for queue and cache metrics
- Dependencies: structured telemetry, telemetry schema.
- Acceptance checks:
  - Operators can inspect queue depth, parse error count, and cache hit ratio in UI.

## 5) Export

### Must-have

1. `P1` **Must** ? Implement deterministic multi-format export engine (`eml`, `mbox`, `json`, `jsonl`)
- Dependencies: parser body model, formatter adapters, output path resolver.
- Acceptance checks:
  - Exports include headers, body, recipients, folders, and attachment metadata.
  - `--deterministic` yields stable file naming and ordering regardless of thread count.

2. `P1` **Must** ? Enforce safe path handling and filename sanitization
- Dependencies: path canonicalization utility, sandbox root check.
- Acceptance checks:
  - Export attempts cannot write outside destination root.
  - Traversal sequences (`../`) and reserved device names are sanitized deterministically.

3. `P2` **Must** ? Add resumable export/checkpoint manifest
- Dependencies: manifest schema + atomic commit routine + export scheduler.
- Acceptance checks:
  - Interrupted exports can resume without duplicate completed records.
  - Manifest includes source id, item id, destination path, checksum, and completion state.

4. `P2` **Must** ? Add optional attachment dedupe by hash/id
- Dependencies: SHA-256 utility, dedupe index.
- Acceptance checks:
  - Duplicate attachment payloads map to single written blob when dedupe enabled.

### Nice-to-have

5. `P3` **Nice** ? Add manifest bundle mode for export audit zip and signature hash manifest
- Dependencies: archive writer and checksum index.
- Acceptance checks:
  - Single command produces audit-ready bundle with reproducible manifest.

## 6) Operations

### Must-have

1. `P0` **Must** ? Enforce no-FFI policy in CI and dependency gates
- Dependencies: dependency audit workflow, allowlist parser in CI.
- Acceptance checks:
  - CI fails if forbidden dependency/classification is introduced.
  - Build matrix confirms no `*-sys` parser/runtime packages are part of runtime closure.

2. `P0` **Must** ? Create cross-platform build pipeline and reproducible artifacts
- Dependencies: packaging matrix, lockfile freeze checks, release docs.
- Acceptance checks:
  - Windows/macOS/Linux release artifacts build from same source revision.
  - Artifacts are byte-reproducible under unchanged input and toolchain constraints.

3. `P1` **Must** ? Add benchmark and regression gates for key paths
- Dependencies: corpus dataset, benchmark harness, CI job limits.
- Acceptance checks:
  - Parse/index/export performance tests run on each milestone branch.
  - Regressions beyond configured thresholds fail CI.

4. `P1` **Must** ? Add structured logging, audit IDs, and support tickets
- Dependencies: logger middleware, request/job context propagation.
- Acceptance checks:
  - Every run emits operation ID, command, filter/search key summary, and error class.
  - Log redaction can hide sensitive addresses/content by config.

5. `P2` **Must** ? Define operational runbooks and defaults
- Dependencies: documentation site/docs updates.
- Acceptance checks:
  - `--help` lists defaults; docs explain tuning for `jobs`, `io-jobs`, `cpu-jobs`.
  - Recovery playbook exists for corrupted index and partial exports.

### Nice-to-have

6. `P2` **Nice** ? Add release health checks and user-facing upgrade notes
- Dependencies: changelog generator, migration notes generator.
- Acceptance checks:
  - Release includes migration notes for index format or export schema changes.

7. `P3` **Nice** ? Add telemetry opt-in summary dashboard
- Dependencies: anonymized metric sink, dashboard ingestion path.
- Acceptance checks:
  - Users can export local support diagnostics package without network required.

## Suggested execution order

1. Parser foundation + CLI skeleton + operations guardrails (No-FFI policy, CI baseline)
2. Indexer MVP + search planner + strict output contract
3. Export pipeline + resume manifests + deterministic naming
4. UI parity layer and background job management
5. Search highlights, optional diagnostics, polish tasks

