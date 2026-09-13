use std::{path::PathBuf, str::FromStr};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    run_capture: RunCaptureConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunCaptureConfig {
    #[serde(default)]
    allow: Vec<Vec<String>>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read Toolz config {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse Toolz config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid Toolz config: {0}")]
    Invalid(String),
}

impl Config {
    /// Loads the explicit `TOOLZ_CONFIG` path or `~/.config/toolz/toolz.toml`.
    ///
    /// # Errors
    ///
    /// Returns an error when an explicit file cannot be read, TOML is invalid, or a
    /// configured allowlist entry is empty.
    pub fn load() -> Result<Self, ConfigError> {
        let explicit = std::env::var_os("TOOLZ_CONFIG").map(PathBuf::from);
        let path = explicit.clone().or_else(default_path);
        let mut config = if let Some(path) = path {
            match std::fs::read_to_string(&path) {
                Ok(contents) => toml::from_str(&contents)
                    .map_err(|source| ConfigError::Parse { path, source })?,
                Err(source)
                    if source.kind() == std::io::ErrorKind::NotFound && explicit.is_none() =>
                {
                    Self::default()
                }
                Err(source) => return Err(ConfigError::Read { path, source }),
            }
        } else {
            Self::default()
        };

        if let Ok(value) = std::env::var("TOOLZ_RUN_ALLOWLIST") {
            config.run_capture.allow = parse_legacy_allowlist(&value);
        }
        config.validate()?;
        Ok(config)
    }

    #[must_use]
    pub fn run_allowlist(&self) -> &[Vec<String>] {
        &self.run_capture.allow
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self
            .run_capture
            .allow
            .iter()
            .any(|entry| entry.is_empty() || entry.iter().any(String::is_empty))
        {
            return Err(ConfigError::Invalid(
                "run_capture.allow entries must contain non-empty tokens".to_owned(),
            ));
        }
        Ok(())
    }
}

fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join(".config").join("toolz").join("toolz.toml"))
}

fn parse_legacy_allowlist(value: &str) -> Vec<Vec<String>> {
    value
        .split(',')
        .filter_map(|entry| {
            let tokens: Vec<_> = entry
                .split_whitespace()
                .map(String::from_str)
                .collect::<Result<_, _>>()
                .ok()?;
            (!tokens.is_empty()).then_some(tokens)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::Config;

    #[test]
    fn parses_structured_allowlist() {
        let config: Config = toml::from_str(
            r#"
                [run_capture]
                allow = [["cargo", "test"], ["pytest"]]
            "#,
        )
        .expect("valid config");
        assert_eq!(config.run_allowlist()[0], ["cargo", "test"]);
    }
}
