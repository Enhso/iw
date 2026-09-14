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
        // `uv run` forks the Python interpreter as its own child rather
        // than exec'ing into it, so `kill_on_drop` above only kills `uv`
        // itself on timeout and leaves the Python grandchild running,
        // reparented. Making `uv` the leader of its own process group lets
        // the timeout branch below kill that whole group, not just `uv`.
        #[cfg(unix)]
        command.process_group(0);

        let child = command
            .spawn()
            .map_err(|err| ResearchError::Spawn(err.to_string()))?;
        // Guards the process group `child` leads: if this future is ever
        // dropped before the group has been waited on (a timeout below, or
        // the caller cancelling this whole `run` call, e.g. via
        // `JoinHandle::abort`), the guard's `Drop` impl kills the group.
        #[cfg(unix)]
        let mut guard = ProcessGroupGuard::new(child.id());

        let output = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(wait_result) => {
                #[cfg(unix)]
                guard.disarm();
                wait_result.map_err(|err| ResearchError::Spawn(err.to_string()))?
            }
            Err(_elapsed) => {
                return Err(ResearchError::Timeout(self.timeout));
            }
        };

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

/// Kills the process group led by `pid` after a timeout.
///
/// `uv run` forks the Python interpreter as its own child rather than
/// exec'ing into it, so killing only the immediate `uv` process (what
/// `Command::kill_on_drop` does) leaves that Python grandchild running,
/// reparented, after the timeout fires. `run` makes `uv` the leader of its
/// own process group before spawning it, so `pid` doubles as the group id
/// and signaling the group reaches both `uv` and the interpreter it
/// forked.
///
/// A missing pid, or one that does not fit in the signed `pid_t` `killpg`
/// expects, is logged and treated as nothing left to kill. `ESRCH` (the
/// group is already gone) is treated as success; any other failure is
/// logged. Neither case is propagated: the caller already has a
/// [`ResearchError::Timeout`] to return regardless of whether the kill
/// itself succeeded.
#[cfg(unix)]
fn kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid else {
        tracing::warn!("research worker had no pid to kill after timing out");
        return;
    };
    let Ok(pid) = i32::try_from(pid) else {
        tracing::warn!(
            pid,
            "research worker pid does not fit in pid_t; cannot kill its process group"
        );
        return;
    };
    match nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    ) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(err) => {
            tracing::warn!(
                pid,
                error = %err,
                "failed to kill research worker process group after timeout"
            );
        }
    }
}

/// Guards a spawned worker's process group, killing it via
/// [`kill_process_group`] on drop unless [`Self::disarm`] was called
/// first.
///
/// `Command::kill_on_drop` (set in [`ResearchWorker::run`]) only reaches
/// the immediate `uv` process, not the Python grandchild it forks, so it
/// cannot alone clean up either a timeout or a cancelled `run` call (the
/// caller dropping `run`'s future, e.g. via `JoinHandle::abort`). Both
/// cases drop this guard while it is still armed, which is exactly when
/// the whole process group needs killing.
#[cfg(unix)]
struct ProcessGroupGuard {
    pid: Option<u32>,
    armed: bool,
}

#[cfg(unix)]
impl ProcessGroupGuard {
    /// Arms a guard for the process group led by `pid`.
    fn new(pid: Option<u32>) -> Self {
        Self { pid, armed: true }
    }

    /// Disarms this guard so its `Drop` impl does not kill the process
    /// group. Call this once the worker has been waited on, successfully
    /// or not: the group no longer needs an unconditional kill.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_process_group(self.pid);
        }
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
    #[cfg(unix)]
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

    #[cfg(unix)]
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

    /// Reads the grandchild pid `timeout_kills_worker_process_group`'s
    /// stub script writes as soon as it starts, retrying briefly in case
    /// the file has not appeared yet by the time `run()` returns.
    #[cfg(target_os = "linux")]
    async fn read_grandchild_pid(pid_file: &std::path::Path) -> i32 {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(contents) = std::fs::read_to_string(pid_file) {
                if let Ok(pid) = contents.trim().parse::<i32>() {
                    return pid;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild pid file {} never appeared",
                pid_file.display()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Returns whether `pid` is still a live, non-zombie process, by
    /// inspecting `/proc/<pid>/stat`. A missing `/proc/<pid>` entry, or a
    /// process state of `Z` (zombie, waiting to be reaped), both count as
    /// dead.
    #[cfg(target_os = "linux")]
    fn process_is_alive(pid: i32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // The second field is the command name in parens, which may itself
        // contain spaces or parens, so find the state field after the last
        // ')' rather than splitting naively on whitespace.
        let Some((_, after_comm)) = stat.rsplit_once(')') else {
            return false;
        };
        after_comm.split_whitespace().next() != Some("Z")
    }

    /// Proves `ResearchWorker::run` kills the worker's whole process group
    /// on timeout, not just the immediate `uv` process: the stub script
    /// backgrounds a `sleep 30` grandchild and records its pid before
    /// waiting on it, so if only the direct child were killed (what
    /// `kill_on_drop` alone does), the grandchild would be reparented and
    /// keep running.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_kills_worker_process_group() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let script_path = dir.path().join("iw-slow-uv-stub.sh");
        let pid_file = dir.path().join("grandchild.pid");
        let script_contents = format!(
            "#!/bin/sh\nsleep 30 &\necho $! > \"{}\"\nwait\n",
            pid_file.display()
        );
        std::fs::write(&script_path, script_contents).expect("write stub script");
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

        let result = worker.run("does this kill the whole process group?").await;
        assert!(matches!(result, Err(ResearchError::Timeout(_))));

        let grandchild_pid = read_grandchild_pid(&pid_file).await;

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if !process_is_alive(grandchild_pid) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild process {grandchild_pid} is still running 3s after timeout"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Proves `ResearchWorker::run` kills the worker's whole process group
    /// when the caller cancels the request (drops `run`'s future), not
    /// just on a timeout: starts `run()` inside a `tokio::spawn`, waits for
    /// the same grandchild-backgrounding stub script's pid file, then
    /// aborts the task instead of waiting for it to finish.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancelled_run_kills_worker_process_group() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let script_path = dir.path().join("iw-slow-uv-stub.sh");
        let pid_file = dir.path().join("grandchild.pid");
        let script_contents = format!(
            "#!/bin/sh\nsleep 30 &\necho $! > \"{}\"\nwait\n",
            pid_file.display()
        );
        std::fs::write(&script_path, script_contents).expect("write stub script");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("stat stub script")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).expect("chmod stub script");

        let worker = ResearchWorker {
            uv_bin: script_path,
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(30),
        };

        let handle = tokio::spawn(async move {
            worker
                .run("does cancellation kill the whole process group?")
                .await
        });

        let grandchild_pid = read_grandchild_pid(&pid_file).await;

        handle.abort();
        let _ = handle.await;

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if !process_is_alive(grandchild_pid) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild process {grandchild_pid} is still running 3s after cancellation"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
