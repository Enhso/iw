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
    /// Loads configuration from environment variables, applying the
    /// documented default for any variable that is unset.
    ///
    /// # Errors
    /// Returns [`ConfigError::InvalidValue`] if `IW_DB_ENGINE` is set to
    /// anything other than `mem` or `sqlite`, or if
    /// `IW_WORKER_TIMEOUT_SECS` is set to a value that does not parse as a
    /// `u64`.
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind = env::var("IW_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

        let db_engine = match env::var("IW_DB_ENGINE") {
            Ok(value) if value == "mem" => DbEngine::Memory,
            Ok(value) if value == "sqlite" => DbEngine::Sqlite,
            Ok(value) => {
                return Err(ConfigError::InvalidValue {
                    var: "IW_DB_ENGINE",
                    value,
                })
            }
            Err(_) => DbEngine::Memory,
        };

        let db_path = env::var("IW_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/iw.sqlite"));

        let uv_bin = env::var("IW_UV_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("uv"));

        let python_dir = env::var("IW_PYTHON_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("python"));

        let fixture_dir = env::var("IW_FIXTURE_DIR").ok().map(PathBuf::from);

        let worker_timeout = match env::var("IW_WORKER_TIMEOUT_SECS") {
            Ok(value) => {
                let secs: u64 = value.parse().map_err(|_| ConfigError::InvalidValue {
                    var: "IW_WORKER_TIMEOUT_SECS",
                    value: value.clone(),
                })?;
                Duration::from_secs(secs)
            }
            Err(_) => Duration::from_secs(300),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate process environment variables, since
    /// `cargo test` runs tests within one process by default and
    /// `Config::from_env` reads global state.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Removes every `IW_*` variable this module reads, so each test below
    /// starts from a clean slate regardless of what an earlier test set.
    fn clear_env() {
        for var in [
            "IW_BIND",
            "IW_DB_ENGINE",
            "IW_DB_PATH",
            "IW_UV_BIN",
            "IW_PYTHON_DIR",
            "IW_FIXTURE_DIR",
            "IW_WORKER_TIMEOUT_SECS",
        ] {
            // Safety: guarded by `ENV_LOCK` so no other thread reads or
            // writes these variables concurrently.
            unsafe {
                env::remove_var(var);
            }
        }
    }

    #[test]
    fn defaults_apply_when_unset() {
        let _guard = ENV_LOCK.lock().expect("lock env mutex");
        clear_env();

        let config = Config::from_env().expect("defaults parse");

        assert_eq!(config.bind, "127.0.0.1:8080");
        assert_eq!(config.db_engine, DbEngine::Memory);
        assert_eq!(config.db_path, PathBuf::from("data/iw.sqlite"));
        assert_eq!(config.uv_bin, PathBuf::from("uv"));
        assert_eq!(config.python_dir, PathBuf::from("python"));
        assert_eq!(config.fixture_dir, None);
        assert_eq!(config.worker_timeout, Duration::from_secs(300));

        clear_env();
    }

    #[test]
    fn invalid_worker_timeout_is_rejected() {
        let _guard = ENV_LOCK.lock().expect("lock env mutex");
        clear_env();
        // Safety: guarded by `ENV_LOCK`.
        unsafe {
            env::set_var("IW_WORKER_TIMEOUT_SECS", "not-a-number");
        }

        let result = Config::from_env();
        clear_env();

        match result {
            Err(ConfigError::InvalidValue { var, value }) => {
                assert_eq!(var, "IW_WORKER_TIMEOUT_SECS");
                assert_eq!(value, "not-a-number");
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }
}
