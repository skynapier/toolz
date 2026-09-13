use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

use crate::{error::ToolError, platform::process::terminate_tree};

const MAX_TIMEOUT_SECONDS: u64 = 600;
const TAIL_BYTES: usize = 16 * 1024;
const DANGEROUS_SHELLS: &[&str] = &[
    "bash",
    "bash.exe",
    "cmd",
    "cmd.exe",
    "dash",
    "fish",
    "ksh",
    "powershell",
    "powershell.exe",
    "pwsh",
    "pwsh.exe",
    "sh",
    "zsh",
];

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunCaptureRequest {
    /// Absolute repository root and command working-directory boundary.
    pub repo_root: String,
    /// Executable allowed by Toolz configuration; paths and shells are rejected.
    pub program: String,
    /// Argument vector passed directly to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional working directory below `repo_root`. Defaults to `repo_root`.
    pub cwd: Option<String>,
    /// Explicit environment additions. Inherited environment is retained.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Timeout in seconds. Defaults to 120; hard limit is 600.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunCaptureResult {
    pub exit_code: Option<i32>,
    pub success: bool,
    pub timed_out: bool,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_log_path: String,
    pub stderr_log_path: String,
    pub elapsed_ms: u128,
}

pub(crate) async fn execute(
    request: &RunCaptureRequest,
    allowlist: &[Vec<String>],
) -> Result<RunCaptureResult, ToolError> {
    validate(request, allowlist)?;
    let root = Path::new(&request.repo_root)
        .canonicalize()
        .map_err(ToolError::Io)?;
    if !root.is_dir() {
        return Err(ToolError::InvalidInput(
            "repo_root must be a directory".to_owned(),
        ));
    }
    let cwd = resolve_cwd(&root, request.cwd.as_deref())?;
    let (stdout_path, stderr_path) = log_paths(&root, &request.program)?;
    let started = std::time::Instant::now();
    let mut child = Command::new(&request.program)
        .args(&request.args)
        .envs(&request.env)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::Io(std::io::Error::other("stdout capture unavailable")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::Io(std::io::Error::other("stderr capture unavailable")))?;
    let stdout_task = tokio::spawn(capture_stream(stdout, stdout_path.clone()));
    let stderr_task = tokio::spawn(capture_stream(stderr, stderr_path.clone()));
    let status = timeout(Duration::from_secs(request.timeout_seconds), child.wait()).await;
    let (status, timed_out) = if let Ok(result) = status {
        (Some(result?), false)
    } else {
        terminate_tree(&mut child).await?;
        let _ = child.wait().await;
        (None, true)
    };
    let stdout = stdout_task.await.map_err(join_error)??;
    let stderr = stderr_task.await.map_err(join_error)??;
    Ok(RunCaptureResult {
        exit_code: status.as_ref().and_then(std::process::ExitStatus::code),
        success: status.is_some_and(|value| value.success()),
        timed_out,
        stdout_tail: stdout.tail,
        stderr_tail: stderr.tail,
        stdout_bytes: stdout.bytes,
        stderr_bytes: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        stdout_log_path: stdout_path.to_string_lossy().into_owned(),
        stderr_log_path: stderr_path.to_string_lossy().into_owned(),
        elapsed_ms: started.elapsed().as_millis(),
    })
}

fn validate(request: &RunCaptureRequest, allowlist: &[Vec<String>]) -> Result<(), ToolError> {
    let base = Path::new(&request.program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(&request.program)
        .to_ascii_lowercase();
    if base != request.program.to_ascii_lowercase() || DANGEROUS_SHELLS.contains(&base.as_str()) {
        return Err(ToolError::InvalidInput(
            "program paths and shell executables are forbidden".to_owned(),
        ));
    }
    if !allowlist.iter().any(|tokens| {
        tokens
            .first()
            .is_some_and(|program| program == &request.program)
            && tokens.len().saturating_sub(1) <= request.args.len()
            && tokens[1..]
                .iter()
                .zip(&request.args)
                .all(|(expected, actual)| expected == actual)
    }) {
        return Err(ToolError::InvalidInput(
            "command is not allowed by Toolz configuration".to_owned(),
        ));
    }
    if request.timeout_seconds == 0 || request.timeout_seconds > MAX_TIMEOUT_SECONDS {
        return Err(ToolError::LimitExceeded(format!(
            "timeout_seconds must be between 1 and {MAX_TIMEOUT_SECONDS}"
        )));
    }
    if request
        .env
        .keys()
        .any(|key| key.contains('=') || key.contains('\0'))
    {
        return Err(ToolError::InvalidInput(
            "environment key contains an invalid character".to_owned(),
        ));
    }
    Ok(())
}

fn resolve_cwd(root: &Path, requested: Option<&str>) -> Result<PathBuf, ToolError> {
    let Some(requested) = requested else {
        return Ok(root.to_path_buf());
    };
    let path = root.join(requested).canonicalize().map_err(ToolError::Io)?;
    if !path.starts_with(root) {
        return Err(ToolError::PathOutsideRoot);
    }
    Ok(path)
}

fn log_paths(root: &Path, program: &str) -> Result<(PathBuf, PathBuf), ToolError> {
    let directory = root.join(".toolz").join("runs");
    std::fs::create_dir_all(&directory)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ToolError::Io(std::io::Error::other(error)))?
        .as_nanos();
    let slug = program.replace(|character: char| !character.is_ascii_alphanumeric(), "_");
    let base = format!("{stamp}-{slug}");
    Ok((
        directory.join(format!("{base}.stdout.log")),
        directory.join(format!("{base}.stderr.log")),
    ))
}

struct CapturedStream {
    tail: String,
    bytes: u64,
    truncated: bool,
}

async fn capture_stream<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    path: PathBuf,
) -> Result<CapturedStream, ToolError> {
    let mut file = tokio::fs::File::create(path).await?;
    let mut tail = Vec::with_capacity(TAIL_BYTES);
    let mut buffer = [0_u8; 8 * 1024];
    let mut bytes = 0_u64;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count]).await?;
        bytes = bytes.saturating_add(count as u64);
        tail.extend_from_slice(&buffer[..count]);
        if tail.len() > TAIL_BYTES {
            tail.drain(..tail.len() - TAIL_BYTES);
        }
    }
    file.flush().await?;
    Ok(CapturedStream {
        tail: String::from_utf8_lossy(&tail).into_owned(),
        bytes,
        truncated: bytes > TAIL_BYTES as u64,
    })
}

fn join_error(error: tokio::task::JoinError) -> ToolError {
    ToolError::Io(std::io::Error::other(error))
}
const fn default_timeout() -> u64 {
    120
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{RunCaptureRequest, execute, validate};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn request(root: &std::path::Path) -> RunCaptureRequest {
        RunCaptureRequest {
            repo_root: root.display().to_string(),
            program: "cargo".to_owned(),
            args: vec!["--version".to_owned()],
            cwd: None,
            env: BTreeMap::new(),
            timeout_seconds: 10,
        }
    }

    #[tokio::test]
    async fn captures_an_allowlisted_program_without_a_shell() {
        let root = tempdir().expect("test directory");
        let allowlist = vec![vec!["cargo".to_owned(), "--version".to_owned()]];
        let result = execute(&request(root.path()), &allowlist)
            .await
            .expect("capture");
        assert!(result.success);
        assert!(result.stdout_tail.starts_with("cargo "));
        assert!(std::path::Path::new(&result.stdout_log_path).is_file());
    }

    #[test]
    fn rejects_shells_even_if_configured() {
        let root = tempdir().expect("test directory");
        let mut value = request(root.path());
        value.program = "pwsh".to_owned();
        assert!(validate(&value, &[vec!["pwsh".to_owned()]]).is_err());
    }
}
