# ADR: No FFI in core parsing/index/export pipeline

## Status

Accepted

## Context

`pst-pst-pst` is intended to run on Windows, macOS, and Linux with no external runtime requirement beyond the Rust binary.  
The product constraints already define "No FFI" and "Native Rust everywhere", so parser, decoder, index, and export behavior must remain in Rust to keep builds portable, auditable, and deployment-simple.

## Decision

We will implement all file ingestion, parsing, indexing, filtering, export, and search logic in pure Rust with no mandatory FFI dependencies in the runtime path.

Implementation constraints:

- No `*-sys` crates or direct COM/Outlook/API bindings in default dependency graph.
- No vendored C/C++ parser/index/export binaries required for normal operation.
- Use Rust-native crate alternatives for:
  - compression/decompression
  - hashing/checksumming
  - path, text, and timestamp handling
  - indexing/search primitives
  - UI and serialization layers
- Enforce crate-policy checks in CI so any new PR adding non-pure-Rust dependencies is visible and reviewable.

## Alternatives

- Use a native Outlook/COM or MAPI dependency for parsing and metadata retrieval.
- Wrap existing C/C++ libraries (e.g., libpst/libpff style wrappers) for parse or index work.
- Keep a mixed model where Rust code shells out to external binaries/daemons for parsing tasks.

## Consequences

- Positive:
  - Builds stay cross-platform with predictable toolchains.
  - Stronger supply-chain control and easier static/security auditing.
  - Simpler distribution (single artifact model, no platform-specific runtime DLL/SO shipping).
- Trade-offs:
  - Some features available in external libraries must be reimplemented or matched via safe Rust equivalents.
  - Lower short-term throughput for some formats until native Rust parity is complete.
  - Faster rejection path for feature requests that depend on proprietary platform APIs.

## Validation Plan

- Add dependency policy checks (CI):
  - fail on newly introduced `*-sys`/FFI-heavy crates unless explicitly exempted.
  - maintain an explicit allowlist for Rust-native alternatives.
- Add portability smoke checks that compile and run the core workspace on Linux/macOS/Windows matrix.
- Add parser/export parity tests for representative `.pst`, `.ost`, `.msg` fixtures to ensure no functional dependency on external runtimes.
- Add binary-size/runtime smoke checks to keep deployments free of bundled external runtime prerequisites.
