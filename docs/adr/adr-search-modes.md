# ADR: Search mode strategy for query execution

## Status

Accepted

## Context

Search over large stores needs to balance correctness, latency, and resource cost:

- `full` scan is correct but slower at scale,
- `indexed` is fast but incomplete when the index is missing/stale,
- `hybrid` can combine speed and correctness guarantees,
- default behavior must stay predictable for users and scripts with one command.

The spec requires all modes to return the same result schema and track match provenance.

## Decision

We will support four modes (`full`, `indexed`, `hybrid`, `auto`) with a strict `index-policy` model, implemented in Rust:

- `full`: decode + scan every candidate message content directly; no index required.
- `indexed`: query persisted index only; if no valid index is available, behavior follows policy.
- `hybrid`: get candidate message IDs from index, then verify matches by scanning message content; optionally include non-indexed regions when explicitly enabled.
- `auto`: use `indexed` when a valid fresh index exists and query coverage allows it; otherwise fall back to `full` or `hybrid`.
- `index-policy`: `allow`, `require`, `build`, `refresh` controls fallback and freshness behavior.
- Every hit includes `match_source` (`full`, `indexed`, `hybrid`) for transparency.

## Alternatives

- Single-mode implementation (`full` only) for simplicity.
- Pure indexed-only model with strict failure on missing/stale indexes.
- Always-hybrid model (candidates + verification always) with no explicit mode switch.
- External search engine/service integration outside Rust process boundary.

## Consequences

- Positive:
  - Users get low-latency paths for common repeated queries.
  - Correctness is still achievable without a prebuilt index.
  - Deterministic behavior is supported through policy and per-mode result metadata.
- Trade-offs:
  - Additional complexity in planner logic (`auto` and policy interactions).
  - Hybrid mode requires both index-query and scan pipelines to stay in sync.
  - Index lifecycle (freshness, corruption, staleness) adds operational surface area.

## Validation Plan

- Add integration tests for:
  - `full` mode correctness baseline.
  - `indexed` behavior under fresh, missing, and stale index states.
  - `hybrid` candidate coverage + scan verification and `match_source` labeling.
  - `auto` policy transitions across dataset sizes.
- Add corpus benchmarks:
  - scan-heavy dataset (`full`) vs indexed repeat query performance (`indexed`/`hybrid`).
  - correctness checks on repeated runs for same seed (`--deterministic` parity).
- Add regression tests for `include-unindexed` and strict policy failure paths.

