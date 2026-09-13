use rmcp::{
    ErrorData, Json, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
};
use serde_json::{Map, Value};

use crate::config::{Config, ConfigError};
use crate::tools::{
    dependency_trace::{self, DependencyTraceRequest, DependencyTraceResult},
    diff_inspect::{self, DiffInspectRequest, DiffInspectResult},
    failure_extract::{self, FailureExtractRequest, FailureExtractResult},
    repo_read::{self, RepoReadRequest, RepoReadResult},
    repo_search::{self, RepoSearchRequest, RepoSearchResult},
    run_capture::{self, RunCaptureRequest, RunCaptureResult},
    secret_scan::{self, SecretScanRequest, SecretScanResult},
};

#[derive(Debug, Clone)]
pub struct ToolzServer {
    tool_router: ToolRouter<Self>,
    config: Config,
}

impl ToolzServer {
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(Config::default())
    }

    /// Builds a server using the shared Toolz configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration cannot be read or validated.
    pub fn from_environment() -> Result<Self, ConfigError> {
        Ok(Self::with_config(Config::load()?))
    }

    fn with_config(config: Config) -> Self {
        let mut tool_router = Self::tool_router();
        normalize_tool_schemas(&mut tool_router);
        Self {
            tool_router,
            config,
        }
    }

    #[must_use]
    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }
}

// JSON Schema treats `format` as an optional annotation. Schemars adds Rust-specific
// values such as `uint64`, which some MCP hosts warn about even though the schemas are
// otherwise valid. Toolz only needs the numeric type and bounds, so omit every format
// annotation at the protocol boundary for consistent host behavior.
fn normalize_tool_schemas(router: &mut ToolRouter<ToolzServer>) {
    for route in router.map.values_mut() {
        strip_formats_from_object(std::sync::Arc::make_mut(&mut route.attr.input_schema));
        if let Some(schema) = &mut route.attr.output_schema {
            strip_formats_from_object(std::sync::Arc::make_mut(schema));
        }
    }
}

fn strip_formats_from_object(object: &mut Map<String, Value>) {
    object.remove("format");
    for value in object.values_mut() {
        strip_formats(value);
    }
}

fn strip_formats(value: &mut Value) {
    match value {
        Value::Object(object) => strip_formats_from_object(object),
        Value::Array(values) => {
            for value in values {
                strip_formats(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

impl Default for ToolzServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = tool_router)]
impl ToolzServer {
    #[tool(
        name = "repo_search",
        description = "Search repository text or list files while honoring Git ignore rules. Content mode accepts literal or Rust-regex queries; files mode supports glob-only discovery. Results are stable and bounded.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn repo_search(
        &self,
        Parameters(request): Parameters<RepoSearchRequest>,
    ) -> Result<Json<RepoSearchResult>, ErrorData> {
        repo_search::execute(&request).map(Json).map_err(Into::into)
    }

    #[tool(
        name = "repo_read",
        description = "Read a bounded line range from one repository file. Paths are canonicalized and must remain below repo_root. Use this instead of a shell when exact source text is needed. Output reports truncation.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn repo_read(
        &self,
        Parameters(request): Parameters<RepoReadRequest>,
    ) -> Result<Json<RepoReadResult>, ErrorData> {
        repo_read::execute(&request)
            .await
            .map(Json)
            .map_err(Into::into)
    }

    #[tool(
        name = "dependency_trace",
        description = "Trace deterministic import, export, and textual-reference evidence for a symbol, module, or source path. It scans supported source files under repo_root and returns bounded, sorted edges; it does not infer intent.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn dependency_trace(
        &self,
        Parameters(request): Parameters<DependencyTraceRequest>,
    ) -> Result<Json<DependencyTraceResult>, ErrorData> {
        dependency_trace::execute(&request)
            .map(Json)
            .map_err(Into::into)
    }

    #[tool(
        name = "diff_inspect",
        description = "Inspect working-tree, staged, or revision Git differences. Returns sorted per-file insertion/deletion counts and an optional bounded patch. Git is invoked directly without a shell or model.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn diff_inspect(
        &self,
        Parameters(request): Parameters<DiffInspectRequest>,
    ) -> Result<Json<DiffInspectResult>, ErrorData> {
        diff_inspect::execute(&request)
            .map(Json)
            .map_err(Into::into)
    }

    #[tool(
        name = "run_capture",
        description = "Run an allowlisted development executable directly, never through a shell. Captures stdout and stderr concurrently with timeout and byte limits. Use failure_extract separately when structured failures are needed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn run_capture(
        &self,
        Parameters(request): Parameters<RunCaptureRequest>,
    ) -> Result<Json<RunCaptureResult>, ErrorData> {
        run_capture::execute(&request, self.config.run_allowlist())
            .await
            .map(Json)
            .map_err(Into::into)
    }

    #[tool(
        name = "failure_extract",
        description = "Extract deterministic compiler, test, runtime, and generic failure records from supplied command output. This tool does not diagnose causes or use a model; it returns bounded structured evidence.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn failure_extract(
        &self,
        Parameters(request): Parameters<FailureExtractRequest>,
    ) -> Result<Json<FailureExtractResult>, ErrorData> {
        failure_extract::execute(&request)
            .map(Json)
            .map_err(Into::into)
    }

    #[tool(
        name = "secret_scan",
        description = "Scan only added Git diff lines for deterministic secret patterns. Findings contain rule, path, line, and a redacted preview; complete suspected secrets are never returned. Supports working-tree or staged changes.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn secret_scan(
        &self,
        Parameters(request): Parameters<SecretScanRequest>,
    ) -> Result<Json<SecretScanResult>, ErrorData> {
        secret_scan::execute(&request).map(Json).map_err(Into::into)
    }
}

#[tool_handler(
    router = self.tool_router,
    name = "toolz",
    version = "0.1.0",
    instructions = "Toolz provides deterministic, offline repository operations. Prefer its bounded structured tools for search, reading, dependency evidence, diff inspection, command capture, failure extraction, and secret scanning. It never invokes a model, embedding service, shell, or network API. Supply an explicit repository root. Treat truncation as a request to narrow the query. Tool errors include stable toolz_code data."
)]
impl ServerHandler for ToolzServer {}
