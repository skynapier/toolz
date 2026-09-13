use std::path::{Path, PathBuf};

use crate::error::ToolError;

pub(crate) fn resolve_existing(
    root: &Path,
    requested: &Path,
) -> Result<(PathBuf, PathBuf), ToolError> {
    let root = root.canonicalize().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ToolError::NotFound(root.display().to_string()),
        _ => ToolError::Io(error),
    })?;
    if !root.is_dir() {
        return Err(ToolError::InvalidInput(
            "repo_root must be a directory".to_owned(),
        ));
    }

    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let canonical = candidate
        .canonicalize()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => ToolError::NotFound(candidate.display().to_string()),
            _ => ToolError::Io(error),
        })?;

    if !canonical.starts_with(&root) {
        return Err(ToolError::PathOutsideRoot);
    }
    Ok((root, canonical))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::resolve_existing;

    #[test]
    fn accepts_a_file_below_root() {
        let root = tempdir().expect("test directory");
        let file = root.path().join("file.txt");
        fs::write(&file, "ok").expect("fixture write");
        let (_, resolved) = resolve_existing(root.path(), "file.txt".as_ref()).expect("resolve");
        assert_eq!(resolved, file.canonicalize().expect("canonical fixture"));
    }

    #[test]
    fn rejects_a_file_outside_root() {
        let root = tempdir().expect("root directory");
        let outside = tempdir().expect("outside directory");
        let file = outside.path().join("secret.txt");
        fs::write(&file, "no").expect("fixture write");
        assert!(resolve_existing(root.path(), &file).is_err());
    }
}
