# toolz

Toolz is a deterministic, offline Rust MCP server for bounded code search, reading,
dependency tracing, diff inspection, Git status, command capture, and failure extraction.

The implementation is being migrated from the read-only behavior of
`../local-coder-mcp`. Toolz does not call models, embedding services, or network APIs.

## Public tools

- `repo_search`
- `repo_read`
- `dependency_trace`
- `diff_inspect`
- `run_capture`
- `failure_extract`
- `git_status`

`local_index` is an internal implementation detail and is not exposed through MCP.

## Bounded reads and compact output

`repo_read` accepts `start_line` plus either inclusive `end_line` or `max_lines`.
For lines 150–180 use `start_line: 150, end_line: 180` (31 lines), or
`start_line: 150, max_lines: 31`. Supplying both bounds fails. With neither,
the default is 400 lines. Unknown request fields fail instead of being ignored.

`repo_search` uses `include` for Globset globs, including `{src,tests}/**/*.rs`;
`path_glob` is accepted as a compatibility alias. Search traversal is sorted before
limits are applied. `include_ignored` explicitly opts into ignored files.

Both tools accept `compact: true`. Reads return a range/truncation header followed
by raw source text; searches return `path:line:column: preview` rows, at most one
per matching source line, followed by scan/truncation information. Compact mode
returns only TextContent, avoiding a duplicate structured payload. Default mode
keeps the JSON result and structuredContent; search keeps occurrence records.
These two tools omit outputSchema because their response mode is selectable.
Reconnect the MCP host after replacing the release binary to load the new schema.

## Diff and Git status scope

`diff_inspect` accepts `paths`, repository-relative paths matched literally, to limit
the diff, and `ignore_whitespace: true` to drop whitespace-only changes like `git diff -w`.
`git_status` runs `git status --porcelain=v2 --branch` and `git log` with optional locks
disabled, so it never rewrites the index. It returns branch, HEAD, upstream, ahead/behind,
changed paths sorted by path (`max_changes`, default 200), and the last `log` commits
(default 5).

## Build and run

Build a release binary and configure an MCP host to launch it over STDIO:

```powershell
cargo build --release
.\target\release\toolz.exe
```

Toolz writes MCP JSON-RPC only to stdout. Diagnostics go to stderr. Every repository
operation requires an explicit `repo_root`, and file paths are restricted to that root.

`run_capture` denies every command unless it appears in the shared configuration:

```toml
# Copy toolz.example.toml to ~/.config/toolz/toolz.toml and edit it.
[run_capture]
allow = [
  ["cargo", "test"],
  ["cargo", "check"],
  ["dotnet", "test"],
  ["npm", "test"],
  ["go", "test"],
  ["pytest"],
]
```

Commands are started directly without a shell. Shell programs and executable paths are
rejected even if they appear in the allowlist. Full output logs are written below
`<repo_root>/.toolz/runs`; the MCP response contains bounded tails.

Toolz reads `~/.config/toolz/toolz.toml` on Windows, macOS, and Linux. Set
`TOOLZ_CONFIG` only when an explicit alternative path is required. The legacy
`TOOLZ_RUN_ALLOWLIST` environment variable remains an override for compatibility; Toolz
does not load `.env` files.

## Host configuration

Use the absolute release-binary path. Codex and Codey use the same entry in their
respective home configuration; the shared Toolz policy is not duplicated here:

```toml
[mcp_servers.toolz]
command = 'C:\absolute\path\to\toolz\target\release\toolz.exe'
```

Claude Code can register the same server at user scope:

```powershell
claude mcp add toolz -s user -- "C:\absolute\path\to\toolz\target\release\toolz.exe"
claude mcp get toolz
```

Back up host configuration before replacing another server. Rollback is limited to
removing `toolz` from each host and restoring that backup; Toolz does not alter the old
server repository.

## Contract policy

Rust request/result structs are the source of truth for MCP schemas. Server startup
removes optional JSON Schema `format` annotations because some hosts warn about
Rust-specific values such as `uint64`; numeric types and bounds remain intact. Contract
tests lock the seven-tool set, schema payload budgets, annotations, and a reviewed schema
digest. Intentional public changes require a deliberate digest update.

## Development checks

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo deny check
```
