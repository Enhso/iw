//! Spawns the Python research worker as a subprocess and parses its output
//! into an [`ExtractionPayload`].

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::model::{ExtractionPayload, PayloadError};

/// Launches the Python research worker (`uv run --directory <python_dir>
/// iw-research --question <question> [--fixture-dir <dir>]`) and parses
/// the single JSON document it prints to stdout.
#[derive(Debug, Clone)]
pub struct ResearchWorker {
    /// Path to (or name of) the `uv` binary.
    pub uv_bin: PathBuf,
    /// The `uv`-managed Python worker project directory.
    pub python_dir: PathBuf,
    /// When set, passed to the worker as `--fixture-dir`, running it in
    /// offline fixture mode instead of hitting live sources and the LLM.
    pub fixture_dir: Option<PathBuf>,
    /// Maximum time to wait for the worker to exit.
    pub timeout: Duration,
}

/// A failure spawning, running, or interpreting the output of the research
/// worker subprocess.
#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    /// The child process could not be spawned or awaited (e.g. `uv_bin`
    /// does not exist).
    #[error("failed to spawn research worker: {0}")]
    Spawn(String),
    /// The worker did not exit within [`ResearchWorker::timeout`].
    #[error("research worker timed out after {0:?}")]
    Timeout(Duration),
    /// The worker exited with a non-zero status.
    #[error("research worker exited with {status}: {stderr_tail}")]
    Failed {
        /// The process's exit status, formatted.
        status: String,
        /// The last 2000 characters of the worker's stderr.
        stderr_tail: String,
    },
    /// The worker's stdout was not a valid [`ExtractionPayload`] document.
    #[error("research worker returned invalid JSON: {0}")]
    InvalidJson(String),
    /// The worker's output parsed but failed [`ExtractionPayload::validate`].
    #[error("research worker payload rejected: {0}")]
    InvalidPayload(#[from] PayloadError),
}

impl ResearchWorker {
    /// Runs the research worker for `question` and returns its parsed,
    /// validated [`ExtractionPayload`].
    ///
    /// # Errors
    /// Returns [`ResearchError::Spawn`] if the child process cannot be
    /// spawned or awaited, [`ResearchError::Timeout`] if it does not exit
    /// within [`Self::timeout`], [`ResearchError::Failed`] if it exits
    /// with a non-zero status, [`ResearchError::InvalidJson`] if stdout is
    /// not a valid [`ExtractionPayload`] document, or
    /// [`ResearchError::InvalidPayload`] if the parsed payload fails
    /// [`ExtractionPayload::validate`].
    pub async fn run(&self, question: &str) -> Result<ExtractionPayload, ResearchError> {
        let mut command = tokio::process::Command::new(&self.uv_bin);
        command
            .arg("run")
            .arg("--directory")
            .arg(&self.python_dir)
            .arg("iw-research")
            .arg("--question")
            .arg(question);
        if let Some(fixture_dir) = &self.fixture_dir {
            // `uv run --directory <python_dir>` changes the child's working
            // directory to `python_dir`, so a relative `fixture_dir` would
            // resolve against the wrong base. Resolve against this
            // process's cwd first; `std::path::absolute` does not touch the
            // filesystem, so a nonexistent directory still reaches the
            // worker and surfaces as `ResearchError::Failed`.
            let absolute_fixture_dir = std::path::absolute(fixture_dir)
                .map_err(|err| ResearchError::Spawn(err.to_string()))?;
            command.arg("--fixture-dir").arg(absolute_fixture_dir);
        }
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = command
            .spawn()
            .map_err(|err| ResearchError::Spawn(err.to_string()))?;

        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| ResearchError::Timeout(self.timeout))?
            .map_err(|err| ResearchError::Spawn(err.to_string()))?;

        if !output.status.success() {
            let stderr_tail = tail_chars(&output.stderr, 2000);
            tracing::warn!(
                status = %output.status,
                stderr_tail = %stderr_tail,
                "research worker exited with a non-zero status"
            );
            return Err(ResearchError::Failed {
                status: output.status.to_string(),
                stderr_tail,
            });
        }

        let payload: ExtractionPayload = serde_json::from_slice(&output.stdout)
            .map_err(|err| ResearchError::InvalidJson(err.to_string()))?;
        payload.validate()?;
        Ok(payload)
    }
}

/// Returns the last `max_chars` characters of `bytes`, decoded lossily as
/// UTF-8.
fn tail_chars(bytes: &[u8], max_chars: usize) -> String {
    let decoded = String::from_utf8_lossy(bytes);
    let total = decoded.chars().count();
    if total <= max_chars {
        decoded.into_owned()
    } else {
        decoded.chars().skip(total - max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Returns this crate's manifest directory, the repository root.
    fn manifest_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[tokio::test]
    async fn nonexistent_uv_bin_fails_to_spawn() {
        let worker = ResearchWorker {
            uv_bin: PathBuf::from("/nonexistent/iw-uv-binary-that-does-not-exist"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(5),
        };

        let result = worker.run("does this spawn?").await;
        assert!(matches!(result, Err(ResearchError::Spawn(_))));
    }

    #[tokio::test]
    async fn missing_fixture_dir_fails_with_nonempty_stderr_tail() {
        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: Some(manifest_dir().join("fixtures/does-not-exist")),
            timeout: Duration::from_secs(60),
        };

        let result = worker.run("Does a missing fixture dir fail cleanly?").await;
        match result {
            Err(ResearchError::Failed { stderr_tail, .. }) => {
                assert!(!stderr_tail.is_empty());
            }
            other => panic!("expected ResearchError::Failed, got {other:?}"),
        }
    }

    /// `cargo test` runs unit tests with the crate's manifest directory as
    /// the working directory, so a relative `fixture_dir` here exercises
    /// exactly the resolution `ResearchWorker::run` must perform: `uv run
    /// --directory python` changes the child's cwd to `python/`, so the
    /// relative path must be resolved against this process's cwd, not the
    /// child's, before being passed to the worker.
    #[tokio::test]
    async fn relative_fixture_dir_is_resolved() {
        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: Some(PathBuf::from("fixtures/offline")),
            timeout: Duration::from_secs(300),
        };

        let payload = worker
            .run("Does a relative fixture dir resolve correctly?")
            .await
            .expect("relative fixture dir resolves");
        assert_eq!(payload.claims.len(), 7);
    }

    #[tokio::test]
    async fn timeout_returns_timeout_error() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let script_path = dir.path().join("iw-slow-uv-stub.sh");
        std::fs::write(&script_path, "#!/bin/sh\nsleep 30\n").expect("write stub script");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("stat stub script")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).expect("chmod stub script");

        let worker = ResearchWorker {
            uv_bin: script_path,
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_millis(200),
        };

        let started = std::time::Instant::now();
        let result = worker.run("does this time out?").await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "run() should return promptly once the timeout fires"
        );
        assert!(matches!(result, Err(ResearchError::Timeout(_))));
    }
}
