use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rusqlite::{Connection, params};

use crate::{
    error::ToolError,
    languages::{Relation, relations},
};

#[derive(Debug)]
pub struct FileRecord {
    pub absolute: PathBuf,
    pub relative: String,
}

pub(crate) fn source_files(root: &Path) -> Result<Vec<FileRecord>, ToolError> {
    let root = root.canonicalize().map_err(ToolError::Io)?;
    let mut files = Vec::new();
    for entry in WalkBuilder::new(&root)
        .hidden(false)
        .git_ignore(true)
        .build()
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) || !is_source(entry.path()) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&root)
            .map_err(|_| ToolError::PathOutsideRoot)?
            .to_string_lossy()
            .replace('\\', "/");
        files.push(FileRecord {
            absolute: entry.into_path(),
            relative,
        });
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(files)
}

fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "c" | "cc"
                    | "cpp"
                    | "cs"
                    | "go"
                    | "h"
                    | "hpp"
                    | "js"
                    | "jsx"
                    | "mjs"
                    | "py"
                    | "rs"
                    | "ts"
                    | "tsx"
            )
        })
}

pub(crate) struct RelationCache {
    connection: Connection,
    root: String,
}

impl RelationCache {
    pub(crate) fn open(root: &Path) -> Option<Self> {
        let directory = cache_directory()?;
        std::fs::create_dir_all(&directory).ok()?;
        let connection = Connection::open(directory.join("relations.sqlite3")).ok()?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS relations (
               repo_root TEXT NOT NULL,
               path TEXT NOT NULL,
               content_hash TEXT NOT NULL,
               payload TEXT NOT NULL,
               PRIMARY KEY (repo_root, path)
             );",
            )
            .ok()?;
        Some(Self {
            connection,
            root: root.to_string_lossy().into_owned(),
        })
    }

    pub(crate) fn get_or_parse(
        &self,
        path: &Path,
        source: &str,
    ) -> Result<Vec<Relation>, ToolError> {
        let key = path.to_string_lossy();
        let hash = blake3::hash(source.as_bytes()).to_hex().to_string();
        let cached = self.connection.query_row(
            "SELECT payload FROM relations WHERE repo_root = ?1 AND path = ?2 AND content_hash = ?3",
            params![self.root, key.as_ref(), hash],
            |row| row.get::<_, String>(0),
        ).ok().and_then(|payload| serde_json::from_str(&payload).ok());
        if let Some(cached) = cached {
            return Ok(cached);
        }
        let parsed = relations(path, source)?;
        if let Ok(payload) = serde_json::to_string(&parsed) {
            let _ = self.connection.execute(
                "INSERT INTO relations (repo_root, path, content_hash, payload) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(repo_root, path) DO UPDATE SET content_hash=excluded.content_hash, payload=excluded.payload",
                params![self.root, key.as_ref(), hash, payload],
            );
        }
        Ok(parsed)
    }
}

fn cache_directory() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("TOOLZ_CACHE_DIR") {
        return Some(PathBuf::from(explicit));
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("toolz").join("cache"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| path.join("Library").join("Caches").join("toolz"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|path| path.join(".cache"))
            })
            .map(|path| path.join("toolz"))
    }
}
