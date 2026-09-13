use tokio::process::Child;

use crate::error::ToolError;

#[cfg(windows)]
pub(crate) async fn terminate_tree(child: &mut Child) -> Result<(), ToolError> {
    if let Some(process_id) = child.id() {
        let status = tokio::process::Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .status()
            .await?;
        if status.success() {
            return Ok(());
        }
    }
    child.kill().await.map_err(ToolError::Io)
}

#[cfg(not(windows))]
pub(crate) async fn terminate_tree(child: &mut Child) -> Result<(), ToolError> {
    child.kill().await.map_err(ToolError::Io)
}
