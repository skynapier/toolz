# AGENTS.md

## Purpose

- Toolz is a deterministic, offline Rust MCP server with exactly seven public tools.
- Do not add model, embedding, HTTP, GitHub, Atlassian, PDF, or web features.
- Treat `../local-coder-mcp` as a read-only migration oracle. Never modify it.

## Structure

- MCP routing: `src/server.rs`; tools: `src/tools/`; index: `src/index/`.
- Language adapters: `src/languages/`; OS-specific code: `src/platform/`.
- Keep modules below 500 lines; split before 700 lines excluding tests.

## Rules

- Use Rust 2024 and the toolchain pinned by `rust-toolchain.toml`.
- Production code must not use `unwrap()`, `expect()`, `panic!()`, `todo!()`, or `unimplemented!()`.
- Use typed serde/schemars I/O, stable error codes, bounded output, and stable sorting.
- Stdout is reserved for MCP JSON-RPC. Send diagnostics only to stderr or JSONL.
- Canonicalize every input path and reject anything outside `repo_root`.
- `run_capture` must keep shell disabled, enforce its allowlist/timeout, drain both streams, and terminate the child process tree.
- Never return or log complete secrets.
- Keep `local_index` internal; do not expose index build/status as an MCP tool.
- Isolate necessary unsafe code in `src/platform/` with `SAFETY` comments and tests.

## Verify before finishing

- Run `cargo fmt --all -- --check`.
- Run `cargo clippy --all-targets --all-features -- -D warnings`.
- Run `cargo test --all-targets --all-features` and `cargo deny check`.
- Schema changes must update generated snapshots; security/platform changes need integration tests. Never weaken behavior or coverage to make checks pass.
- Report verification results and remaining risks.
