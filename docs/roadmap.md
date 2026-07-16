# pst-pst-pst Roadmap

Source of truth:
This roadmap is derived from `spec.md` and `README.md`.

## Objectives and non-negotiables

- No FFI: no native bindings, no COM/Outlook SDK, no `*-sys` runtime dependency path in production.
- Multithreaded-by-default: all heavy CPU/I/O paths run in bounded parallel workers unless `--single-thread` is set.
- Search parity: CLI and UI expose `full`, `indexed`, `hybrid`, and `auto` modes with the same schema and behavior.
- Native-only safety: local-first processing, read-only source handling by default, deterministic outputs when requested.
- License: MIT.

## Phased roadmap

| Phase | Milestone | Estimate | Dependencies |
| --- | --- | --- | --- |
| 0 | Foundation and hard constraints | 2–3 weeks | None (bootstrapping) |
| 1 | Parsers and core CLI pipeline | 4–6 weeks | Phase 0 |
| 2 | Index and search engine | 5–7 weeks | Phase 1 |
| 3 | Export, automation, and reliability | 4–6 weeks | Phase 1, Phase 2 |
| 4 | UI parity and production hardening | 4–6 weeks | Phase 1, 2, 3 |
| 5 | Release gate to v1.0 | 2–3 weeks | Phase 0–4 complete |

## Milestone 0 — Foundation and hard constraints

- Dependencies: none.
- Duration: 2–3 weeks.
- Outcomes:
  - Workspace crate layout (`core`, `common`, `parser`, `index`, `export`, `ui`, `cli`) is established and compile-gated for the required platforms.
  - No-FFI enforcement is automatic in CI (`deny` checks for forbidden crates, import policy, optional allowlist).
  - Shared domain types and config schema are defined for cross-crate compatibility.
  - Deterministic logging contract and parse manifest format are introduced.
  - Default runtime settings (jobs, io_jobs, cpu_jobs, single_thread flag semantics) are finalized.
- Done criteria:
  - Build fails if any prohibited FFI dependency is introduced.
  - Static check confirms no `*-sys` crate in the production dependency tree.
  - Local-only and read-only analysis defaults are documented and tested.
  - A `--single-thread` path exists for every heavy pipeline entry point.
- Risks and mitigations:
  - Risk: accidental native dependency is added through transitive crates. Mitigation: strict dependency policy + deny list in CI.
  - Risk: cross-platform build drift between x64 and arm64 targets. Mitigation: explicit matrix in CI from day one.

## Milestone 1 — Parsers and core CLI pipeline

- Dependencies: Milestone 0.
- Duration: 4–6 weeks.
- Outcomes:
  - Native Rust `.pst`, `.ost`, `.msg` readers with read-only traversal and header/container validation.
  - Shared query/filter parser implementing `field:op:value`, composition, and error mapping.
  - CLI commands implemented: `info`, `folders`, `messages`, `validate`.
  - Corruption-tolerant traversal with per-item recovery state and non-fatal diagnostics.
  - Baseline multithread execution for discovery and decode stages with bounded queues.
  - Baseline full-text search mode (`search --search-mode full`) available in CLI with streaming output support.
- Done criteria:
  - `info`, `folders`, `messages`, `validate` operate on representative `.pst`/`.ost`/`.msg` fixtures.
  - Parser continues through partial failures and emits structured error manifests.
  - Throughput and behavior are deterministic under `--deterministic`.
  - Default job pool uses CPU+IO concurrency; single-thread mode disables worker parallelism safely.
- Risks and mitigations:
  - Risk: malformed container variants break traversal completeness. Mitigation: golden corpora + robust partial-failure policy.
  - Risk: query language parser divergence from UI/CLI expectations. Mitigation: AST shared crate + language tests in both layers.

## Milestone 2 — Index and search engine

- Dependencies: Milestone 1.
- Duration: 5–7 weeks.
- Outcomes:
  - Persistent index crate added with manifest versioning and checkpoint strategy.
  - CLI `index` command implemented for build, refresh, and validation.
  - Search modes implemented end-to-end: `indexed`, `full`, `hybrid`, `auto`.
  - `search --search-mode auto` auto-selects indexed/full based on index health and staleness policy.
  - `match_source` and mode metadata are present for every hit.
  - Ranking stability and resume tokens are stable across mode transitions.
- Done criteria:
  - All four search modes are available with matching result schema.
  - `auto` mode chooses indexed first when index is valid; otherwise full-text fallback.
  - Hybrid mode returns correct results with bounded full-text verification and optional `include-unindexed`.
  - No-FFI rule remains enforced with added search/index dependencies.
- Risks and mitigations:
  - Risk: index staleness causes false positives/negatives. Mitigation: explicit staleness policy and refresh/rebuild behavior.
  - Risk: memory overhead from indexing on large stores. Mitigation: segmented merge, bounded queues, and profile-driven tuning.

## Milestone 3 — Export, automation, and reliability

- Dependencies: Milestone 1 and Milestone 2.
- Duration: 4–6 weeks.
- Outcomes:
  - `extract` and `export` commands implemented for `eml`, `mbox`, `json`, `jsonl`.
  - Deterministic export naming and manifest-based resume support.
  - Attachment export hardening: sanitization, MIME preservation where possible, hash-based dedupe.
  - `watch` command implemented for filesystem-based automation hooks.
  - Producer/consumer bounded concurrency for decode/export path.
- Done criteria:
  - Multi-message and attachment-heavy exports complete under bounded memory.
  - Restarting an interrupted export resumes from checkpoint with idempotent outcomes.
  - Path traversal protections pass security checks for nested filenames and symlink risk.
  - `watch` jobs emit structured audit events for each trigger and run result.
- Risks and mitigations:
  - Risk: export determinism breaks with concurrent writes. Mitigation: checkpoint ordering + stable worker merge.
  - Risk: automated workflows trigger duplicate work in watch mode. Mitigation: debounce and dedupe keyed by canonical file signatures.

## Milestone 4 — UI parity and production hardening

- Dependencies: Milestone 1, Milestone 2, Milestone 3.
- Duration: 4–6 weeks.
- Outcomes:
  - Native UI ships with folder tree, virtualized message list, message preview, timeline/filter modes.
  - Search pane exposes search-mode selection and policy toggles with visible result provenance (`match_source`).
  - Background indexing progress and diagnostics integrated into UI task manager.
  - CLI/UI parity for filters, search, export, and determinism.
  - Keyboard and command-palette flows validated with resumable tasks and progress telemetry.
- Done criteria:
  - Common behavior and output semantics match CLI contracts for metadata, filters, and search.
  - Search mode behavior in UI is behaviorally equivalent to CLI.
  - Long-running jobs remain responsive under 250 ms UI interaction target.
  - All operations remain thread-safe and bounded via cancellation-aware worker pools.
- Risks and mitigations:
  - Risk: UI thread starvation during background jobs. Mitigation: lock-free channels + strict producer/consumer queues.
  - Risk: inconsistent search mode state between UI and CLI. Mitigation: shared query/search service and schema tests.

## Milestone 5 — v1.0 release hardening

- Dependencies: Milestones 0–4 complete and integrated.
- Duration: 2–3 weeks.
- Outcomes:
  - Performance validation targets met (`>=1.0 GB/min` metadata parse baseline, +2x 4-core throughput vs single-thread).
  - Validation matrix executed for Windows/macOS/Linux with x64/arm64 where applicable.
  - Fuzz, golden, deterministic, and benchmark tests integrated in CI.
  - Release packaging, docs, and usage guide alignment completed.
- Done criteria:
  - No-crash policy confirmed under malformed fixtures with controlled recovery reporting.
  - Full text/indexed/hybrid search functional in CLI and UI with stable result schema.
  - Multi-platform release builds complete with documented install/run guidance.
  - `v1.0` milestone approved when all mandatory acceptance checks pass.

## Cross-cutting risk register

- No-FFI policy drift
  - Mitigation: CI policy job + periodic dependency SBOM review.
- Threading regressions
  - Mitigation: worker limits and backpressure tests in benchmarks; single-thread fallback validated.
- Cross-platform parity gaps
  - Mitigation: matrix-first CI and platform-specific bug-fix backlog with explicit owners.
- Search correctness under concurrency
  - Mitigation: deterministic test vectors for full/indexed/hybrid and randomized mode transition checks.
- Performance instability on large corpora
  - Mitigation: staged rollout, benchmark gates, and queue memory ceilings.

## Delivery flow and sequencing

- Phase 0 unlocks all secure and policy constraints.
- Phase 1 creates the foundation for parse/search correctness.
- Phase 2 introduces search engines without weakening no-FFI.
- Phase 3 adds deterministic export and automation, closing the data-out path.
- Phase 4 ensures investigation UX reaches CLI parity.
- Phase 5 executes final quality gates for production release.
