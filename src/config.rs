//! Runtime configuration for the Axum service, loaded once from process
//! environment variables at startup.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

/// Runtime configuration for the Axum service.
///
/// Every field is populated by [`Config::from_env`] from a documented
/// environment variable, falling back to a documented default when that
/// variable is unset. LLM credentials (`LLM_API_BASE`, `LLM_API_KEY`,
/// `LLM_MODEL`) are intentionally not read here: they pass through to the
/// Python research worker via its inherited environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// Socket address the Axum service binds to (`IW_BIND`, default
    /// `127.0.0.1:8080`).
    pub bind: String,
    /// Which mnestic storage engine to open (`IW_DB_ENGINE`, default
    /// `mem`).
    pub db_engine: DbEngine,
    /// Path to the sqlite database file, used when `db_engine` is
    /// [`DbEngine::Sqlite`] (`IW_DB_PATH`, default `data/iw.sqlite`).
    pub db_path: PathBuf,
    /// Path to (or name of) the `uv` binary used to spawn the research
    /// worker (`IW_UV_BIN`, default `uv`).
    pub uv_bin: PathBuf,
    /// The `uv`-managed Python worker project directory (`IW_PYTHON_DIR`,
    /// default `python`).
    pub python_dir: PathBuf,
    /// Fixture directory passed to the research worker in offline mode
    /// (`IW_FIXTURE_DIR`, unset means live mode).
    pub fixture_dir: Option<PathBuf>,
    /// Maximum time to wait for the research worker before timing out
    /// (`IW_WORKER_TIMEOUT_SECS`, default `300`).
    pub worker_timeout: Duration,
}

/// Which mnestic storage engine [`Config::db_engine`] selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbEngine {
    /// In-memory, non-persistent storage (`mem`).
    Memory,
    /// Storage persisted to a sqlite file at [`Config::db_path`]
    /// (`sqlite`).
    Sqlite,
}

/// A [`Config::from_env`] failure: an environment variable was set to a
/// value that could not be parsed.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// `var` was set to `value`, which is not a valid value for it.
    #[error("invalid value for {var}: {value}")]
    InvalidValue {
        /// The environment variable name.
        var: &'static str,
        /// The value it was set to.
        value: String,
    },
}

impl Config {
    /// Loads configuration by calling `lookup` for each documented
    /// environment variable, applying its documented default whenever
    /// `lookup` returns `None`. Holds all the parsing; [`Self::from_env`]
    /// is the thinnest possible wrapper around it.
    ///
    /// # Errors
    /// Returns [`ConfigError::InvalidValue`] if `lookup("IW_DB_ENGINE")`
    /// returns anything other than `None`, `Some("mem")`, or
    /// `Some("sqlite")`, or if `lookup("IW_WORKER_TIMEOUT_SECS")` returns
    /// `Some` a value that does not parse as a `u64`.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(lookup: F) -> Result<Self, ConfigError> {
        let bind = lookup("IW_BIND").unwrap_or_else(|| "127.0.0.1:8080".to_string());

        let db_engine = match lookup("IW_DB_ENGINE") {
            Some(value) if value == "mem" => DbEngine::Memory,
            Some(value) if value == "sqlite" => DbEngine::Sqlite,
            Some(value) => {
                return Err(ConfigError::InvalidValue {
                    var: "IW_DB_ENGINE",
                    value,
                })
            }
            None => DbEngine::Memory,
        };

        let db_path = lookup("IW_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data/iw.sqlite"));

        let uv_bin = lookup("IW_UV_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("uv"));

        let python_dir = lookup("IW_PYTHON_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python"));

        let fixture_dir = lookup("IW_FIXTURE_DIR").map(PathBuf::from);

        let worker_timeout = match lookup("IW_WORKER_TIMEOUT_SECS") {
            Some(value) => {
                let secs: u64 = value.parse().map_err(|_| ConfigError::InvalidValue {
                    var: "IW_WORKER_TIMEOUT_SECS",
                    value: value.clone(),
                })?;
                Duration::from_secs(secs)
            }
            None => Duration::from_secs(300),
        };

        Ok(Self {
            bind,
            db_engine,
            db_path,
            uv_bin,
            python_dir,
            fixture_dir,
            worker_timeout,
        })
    }

    /// Loads configuration from environment variables, applying the
    /// documented default for any variable that is unset. A thin wrapper
    /// around [`Self::from_lookup`].
    ///
    /// # Errors
    /// See [`Self::from_lookup`].
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| env::var(key).ok())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Builds a `lookup` closure over `vars` for [`Config::from_lookup`],
    /// so tests never touch real process environment variables.
    fn lookup_over(vars: HashMap<&'static str, &'static str>) -> impl Fn(&str) -> Option<String> {
        move |key| vars.get(key).map(|value| (*value).to_string())
    }

    #[test]
    fn defaults_apply_when_unset() {
        let config = Config::from_lookup(lookup_over(HashMap::new())).expect("defaults parse");

        assert_eq!(config.bind, "127.0.0.1:8080");
        assert_eq!(config.db_engine, DbEngine::Memory);
        assert_eq!(config.db_path, PathBuf::from("data/iw.sqlite"));
        assert_eq!(config.uv_bin, PathBuf::from("uv"));
        assert_eq!(config.python_dir, PathBuf::from("python"));
        assert_eq!(config.fixture_dir, None);
        assert_eq!(config.worker_timeout, Duration::from_secs(300));
    }

    #[test]
    fn invalid_worker_timeout_is_rejected() {
        let vars = HashMap::from([("IW_WORKER_TIMEOUT_SECS", "not-a-number")]);

        let result = Config::from_lookup(lookup_over(vars));

        match result {
            Err(ConfigError::InvalidValue { var, value }) => {
                assert_eq!(var, "IW_WORKER_TIMEOUT_SECS");
                assert_eq!(value, "not-a-number");
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }
}
