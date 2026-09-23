//! Spawns the Python research worker as a subprocess and parses its output.
//!
//! Worker protocol v2 (`docs/contracts.md` §A in the `betomcat` repository):
//! the worker is a subcommand CLI, `research` or `classify-family`. Rust
//! writes one JSON request to the worker's **stdin** and reads exactly one
//! JSON document from its **stdout**; the worker logs only to stderr. Exit
//! code 0 = success. The old `--question` flag form is gone.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::{ExtractionPayload, GateLogEntry, PayloadError};

/// `research`/`classify-family` request `context` object
/// (`docs/contracts.md` §A1/§A3): resolution details for the question,
/// carried through unmodified to the worker's LLM prompts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResearchContext {
    #[serde(default)]
    pub resolution_criteria: String,
    #[serde(default)]
    pub fine_print: String,
    #[serde(default)]
    pub background: String,
}

/// The family a `research` request is already tagged to
/// (`docs/contracts.md` §A1 `family`).
#[derive(Debug, Clone, Serialize)]
pub struct ResearchFamily {
    pub id: String,
    pub label: String,
    /// RFC 3339 UTC, when this family has prior research history.
    pub last_seen: Option<String>,
}

/// `research` subcommand request body, written to the worker's stdin
/// (`docs/contracts.md` §A1).
///
/// `max_news`/`max_wiki` are `Option<u32>` on the Rust side (the HTTP
/// caller may omit them) but the worker's `ResearchRequest.max_news` /
/// `.max_wiki` are non-optional `int` fields with their own defaults (12,
/// 3): sending an explicit JSON `null` fails Pydantic validation, so
/// `None` here must be **omitted** from the JSON, not nulled, letting the
/// worker's own default apply. Every other optional field is `X | None` on
/// the worker side, where an explicit `null` is accepted; they are also
/// omitted when absent, for a consistently small wire payload.
#[derive(Debug, Clone, Serialize)]
pub struct ResearchRequest {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ResearchContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<ResearchFamily>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub news_since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_news: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_wiki: Option<u32>,
}

/// One live family offered as a `classify-family` candidate
/// (`docs/contracts.md` §A3 `families[]`).
#[derive(Debug, Clone, Serialize)]
pub struct ClassifyFamilyCandidate {
    pub id: String,
    pub label: String,
    pub description: String,
}

/// `classify-family` subcommand request body, written to the worker's
/// stdin (`docs/contracts.md` §A3).
#[derive(Debug, Clone, Serialize)]
pub struct ClassifyFamilyRequest {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ResearchContext>,
    pub families: Vec<ClassifyFamilyCandidate>,
}

/// `classify-family` subcommand response, read from the worker's stdout
/// (`docs/contracts.md` §A4). `family_id` is present only when `decision`
/// is `"matched"`; `label`/`description` only when `"minted"`.
#[derive(Debug, Clone, Deserialize)]
pub struct ClassifyFamilyResponse {
    /// One of `matched | minted | none`.
    pub decision: String,
    #[serde(default)]
    pub family_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub probability: Option<f64>,
    #[serde(default)]
    pub jev_confidence: Option<f64>,
    pub method: String,
    #[serde(default)]
    pub gate_log: Vec<GateLogEntry>,
}

/// Launches the Python research worker as a subcommand CLI and parses the
/// single JSON document it prints to stdout.
///
/// By default the worker command is `<uv_bin> run --directory <python_dir>
/// iw-research <subcommand> [--fixture-dir <dir>]`. Setting `worker_cmd`
/// (`IW_WORKER_CMD`) replaces the `<uv_bin> run --directory <python_dir>
/// iw-research` prefix with an arbitrary whitespace-split command, so tests
/// can point at a stub script instead of a real `uv`/Python environment;
/// the subcommand and `--fixture-dir` are still appended after it.
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
    /// Overrides the `<uv_bin> run --directory <python_dir> iw-research`
    /// prefix (`IW_WORKER_CMD`). `None` uses the default `uv` invocation.
    pub worker_cmd: Option<Vec<String>>,
}

/// A failure spawning, running, or interpreting the output of the research
/// worker subprocess.
#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    /// The child process could not be spawned or awaited (e.g. `uv_bin`
    /// does not exist, or `worker_cmd` is empty).
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
    /// The worker's stdout was not a valid JSON document of the shape
    /// expected for the subcommand that was run.
    #[error("research worker returned invalid JSON: {0}")]
    InvalidJson(String),
    /// The worker's `research` output parsed but failed
    /// [`ExtractionPayload::validate`].
    #[error("research worker payload rejected: {0}")]
    InvalidPayload(#[from] PayloadError),
}

impl ResearchWorker {
    /// Runs the `research` subcommand for `request` and returns its parsed,
    /// validated [`ExtractionPayload`].
    ///
    /// # Errors
    /// See [`Self::spawn_and_collect`] for the spawn/timeout/exit-status
    /// failure modes; additionally returns [`ResearchError::InvalidJson`]
    /// if stdout is not a valid [`ExtractionPayload`] document, or
    /// [`ResearchError::InvalidPayload`] if it fails
    /// [`ExtractionPayload::validate`].
    pub async fn run_research(
        &self,
        request: &ResearchRequest,
    ) -> Result<ExtractionPayload, ResearchError> {
        let stdin =
            serde_json::to_vec(request).map_err(|err| ResearchError::Spawn(err.to_string()))?;
        let stdout = self.spawn_and_collect("research", &stdin).await?;
        let payload: ExtractionPayload = serde_json::from_slice(&stdout)
            .map_err(|err| ResearchError::InvalidJson(err.to_string()))?;
        payload.validate()?;
        Ok(payload)
    }

    /// Runs the `classify-family` subcommand for `request` and returns its
    /// parsed [`ClassifyFamilyResponse`]. Unlike `research`, this response
    /// has no [`ExtractionPayload`]-shaped validation to run.
    ///
    /// # Errors
    /// See [`Self::spawn_and_collect`]; additionally returns
    /// [`ResearchError::InvalidJson`] if stdout is not a valid
    /// [`ClassifyFamilyResponse`] document.
    pub async fn run_classify_family(
        &self,
        request: &ClassifyFamilyRequest,
    ) -> Result<ClassifyFamilyResponse, ResearchError> {
        let stdin =
            serde_json::to_vec(request).map_err(|err| ResearchError::Spawn(err.to_string()))?;
        let stdout = self.spawn_and_collect("classify-family", &stdin).await?;
        serde_json::from_slice(&stdout).map_err(|err| ResearchError::InvalidJson(err.to_string()))
    }

    /// Spawns the worker for `subcommand`, writes `stdin_json` to its
    /// stdin, and returns its raw stdout bytes once it exits successfully.
    ///
    /// # Errors
    /// Returns [`ResearchError::Spawn`] if the child process cannot be
    /// spawned or awaited, [`ResearchError::Timeout`] if it does not exit
    /// within [`Self::timeout`], or [`ResearchError::Failed`] if it exits
    /// with a non-zero status.
    async fn spawn_and_collect(
        &self,
        subcommand: &str,
        stdin_json: &[u8],
    ) -> Result<Vec<u8>, ResearchError> {
        let mut command = self.build_command(subcommand)?;

        let mut child = command
            .spawn()
            .map_err(|err| ResearchError::Spawn(err.to_string()))?;
        // Guards the process group `child` leads: if this future is ever
        // dropped before the group has been waited on (a timeout below, or
        // the caller cancelling this whole call, e.g. via
        // `JoinHandle::abort`), the guard's `Drop` impl kills the group.
        #[cfg(unix)]
        let mut guard = ProcessGroupGuard::new(child.id());

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ResearchError::Spawn("child stdin was not piped".to_string()))?;
        let stdin_bytes = stdin_json.to_vec();

        // Write stdin concurrently with draining stdout/stderr (rather
        // than write-then-wait): a worker that starts writing output
        // before it has finished reading stdin, or a stdin payload larger
        // than the OS pipe buffer, would otherwise deadlock both sides.
        let wait_with_stdin_write = async move {
            use tokio::io::AsyncWriteExt;
            let write_result = stdin.write_all(&stdin_bytes).await;
            drop(stdin); // closes the pipe, sending EOF to the worker.
            let output = child.wait_with_output().await;
            (write_result, output)
        };

        let (write_result, wait_result) =
            match tokio::time::timeout(self.timeout, wait_with_stdin_write).await {
                Ok((write_result, wait_result)) => {
                    #[cfg(unix)]
                    guard.disarm();
                    (write_result, wait_result)
                }
                Err(_elapsed) => {
                    return Err(ResearchError::Timeout(self.timeout));
                }
            };
        // A write failure (e.g. the worker exited before reading all of
        // stdin) is not itself fatal: the exit status below is the
        // authoritative signal, and a worker that errors on truncated
        // input will already report that failure via a non-zero exit.
        drop(write_result);
        let output = wait_result.map_err(|err| ResearchError::Spawn(err.to_string()))?;

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

        Ok(output.stdout)
    }

    /// Builds the `tokio::process::Command` for `subcommand`, applying the
    /// `worker_cmd` override when set and appending `--fixture-dir` when
    /// [`Self::fixture_dir`] is set.
    ///
    /// # Errors
    /// Returns [`ResearchError::Spawn`] if `worker_cmd` is `Some(&[])`, or
    /// if a relative `fixture_dir` cannot be resolved to an absolute path.
    fn build_command(&self, subcommand: &str) -> Result<tokio::process::Command, ResearchError> {
        let mut command = match &self.worker_cmd {
            Some(parts) => {
                let (program, rest) = parts.split_first().ok_or_else(|| {
                    ResearchError::Spawn("IW_WORKER_CMD must not be empty".to_string())
                })?;
                let mut command = tokio::process::Command::new(program);
                command.args(rest);
                command
            }
            None => {
                let mut command = tokio::process::Command::new(&self.uv_bin);
                command
                    .arg("run")
                    .arg("--directory")
                    .arg(&self.python_dir)
                    .arg("iw-research");
                command
            }
        };
        command.arg(subcommand);

        if let Some(fixture_dir) = &self.fixture_dir {
            // `uv run --directory <python_dir>` (and, in general, whatever
            // `worker_cmd` points at) may run with a different working
            // directory than this process, so a relative `fixture_dir`
            // must be resolved against THIS process's cwd first;
            // `std::path::absolute` does not touch the filesystem, so a
            // nonexistent directory still reaches the worker and surfaces
            // as `ResearchError::Failed`.
            let absolute_fixture_dir = std::path::absolute(fixture_dir)
                .map_err(|err| ResearchError::Spawn(err.to_string()))?;
            command.arg("--fixture-dir").arg(absolute_fixture_dir);
        }

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // `uv run` forks the Python interpreter as its own child rather
        // than exec'ing into it, so `kill_on_drop` above only kills `uv`
        // itself on timeout and leaves the Python grandchild running,
        // reparented. Making the worker the leader of its own process
        // group lets the timeout branch kill that whole group, not just
        // the immediate child (harmless for a `worker_cmd` stub that has
        // no grandchild of its own).
        #[cfg(unix)]
        command.process_group(0);

        Ok(command)
    }
}

/// Kills the process group led by `pid` after a timeout.
///
/// `uv run` forks the Python interpreter as its own child rather than
/// exec'ing into it, so killing only the immediate `uv` process (what
/// `Command::kill_on_drop` does) leaves that Python grandchild running,
/// reparented, after the timeout fires. `build_command` makes the worker
/// the leader of its own process group before spawning it, so `pid`
/// doubles as the group id and signaling the group reaches both the
/// immediate process and anything it forked.
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
/// `Command::kill_on_drop` (set in [`ResearchWorker::build_command`]) only
/// reaches the immediate worker process, not a Python grandchild it may
/// fork, so it cannot alone clean up either a timeout or a cancelled call
/// (the caller dropping the future, e.g. via `JoinHandle::abort`). Both
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

    fn sample_research_request() -> ResearchRequest {
        ResearchRequest {
            question: "Does the stub worker round-trip a research request?".to_string(),
            question_id: None,
            context: None,
            family: None,
            providers: None,
            news_since: None,
            max_news: None,
            max_wiki: None,
        }
    }

    /// A worker pointed at [`fixtures/rust/stub_worker.sh`], which ignores
    /// stdin and emits a canned response keyed on the subcommand argument
    /// (`docs/plan-phase1.md`-style fixture, but for the v2 worker
    /// protocol; see the file itself). Proves the stdin/stdout subcommand
    /// protocol and the `IW_WORKER_CMD` seam without needing `uv`/Python.
    fn stub_worker() -> ResearchWorker {
        ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(10),
            worker_cmd: Some(vec![manifest_dir()
                .join("fixtures/rust/stub_worker.sh")
                .to_string_lossy()
                .into_owned()]),
        }
    }

    #[tokio::test]
    async fn stub_worker_research_round_trips_via_stdin_stdout() {
        let worker = stub_worker();
        let payload = worker
            .run_research(&sample_research_request())
            .await
            .expect("stub worker research succeeds");
        assert_eq!(payload.schema_version, 2);
        assert_eq!(payload.claims.len(), 7);
    }

    #[tokio::test]
    async fn stub_worker_classify_family_round_trips_via_stdin_stdout() {
        let worker = stub_worker();
        let request = ClassifyFamilyRequest {
            question: "Will the ECB cut its deposit rate at the October 2026 meeting?".to_string(),
            context: None,
            families: vec![ClassifyFamilyCandidate {
                id: "fam:ecb-rate-decisions".to_string(),
                label: "ECB rate decisions".to_string(),
                description: "Questions on ECB monetary-policy decisions.".to_string(),
            }],
        };
        let response = worker
            .run_classify_family(&request)
            .await
            .expect("stub worker classify-family succeeds");
        assert_eq!(response.decision, "matched");
        assert_eq!(
            response.family_id.as_deref(),
            Some("fam:ecb-rate-decisions")
        );
    }

    #[tokio::test]
    async fn nonexistent_uv_bin_fails_to_spawn() {
        let worker = ResearchWorker {
            uv_bin: PathBuf::from("/nonexistent/iw-uv-binary-that-does-not-exist"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(5),
            worker_cmd: None,
        };

        let result = worker.run_research(&sample_research_request()).await;
        assert!(matches!(result, Err(ResearchError::Spawn(_))));
    }

    #[tokio::test]
    async fn empty_worker_cmd_fails_to_spawn() {
        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(5),
            worker_cmd: Some(vec![]),
        };

        let result = worker.run_research(&sample_research_request()).await;
        assert!(matches!(result, Err(ResearchError::Spawn(_))));
    }

    #[tokio::test]
    async fn missing_fixture_dir_fails_with_nonempty_stderr_tail() {
        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: Some(manifest_dir().join("fixtures/does-not-exist")),
            timeout: Duration::from_secs(60),
            worker_cmd: None,
        };

        let result = worker.run_research(&sample_research_request()).await;
        match result {
            Err(ResearchError::Failed { stderr_tail, .. }) => {
                assert!(!stderr_tail.is_empty());
            }
            other => panic!("expected ResearchError::Failed, got {other:?}"),
        }
    }

    /// Cross-boundary proof against the REAL Python worker in fixture mode,
    /// using the new `research` subcommand + stdin protocol. This only
    /// passes once the C2 chunk (the Python worker) implements the v2
    /// subcommand protocol; see the build log / final report for whether
    /// it passed at the time this chunk (C1) was finished. Not `#[ignore]`d
    /// per instructions: it documents the contract this chunk depends on.
    ///
    /// `cargo test` runs unit tests with the crate's manifest directory as
    /// the working directory, so a relative `fixture_dir` here exercises
    /// exactly the resolution `build_command` must perform.
    #[tokio::test]
    async fn question_roundtrip_across_python_boundary() {
        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: Some(PathBuf::from("fixtures/offline")),
            timeout: Duration::from_secs(300),
            worker_cmd: None,
        };

        let request = ResearchRequest {
            question: "Can export controls durably slow China's access to advanced \
                       semiconductor manufacturing capability?"
                .to_string(),
            question_id: None,
            context: None,
            family: None,
            providers: None,
            news_since: None,
            max_news: None,
            max_wiki: None,
        };

        let payload = worker
            .run_research(&request)
            .await
            .expect("real Python worker (v2 subcommand protocol) round-trips a research request");
        assert_eq!(payload.schema_version, 2);
    }

    /// Same cross-boundary proof as
    /// [`question_roundtrip_across_python_boundary`], for the
    /// `classify-family` subcommand: an empty `families` list against the
    /// real worker in fixture mode should mint a new family.
    #[tokio::test]
    async fn classify_family_roundtrip_across_python_boundary() {
        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: Some(PathBuf::from("fixtures/offline")),
            timeout: Duration::from_secs(300),
            worker_cmd: None,
        };

        let request = ClassifyFamilyRequest {
            question: "Can export controls durably slow China's access to advanced \
                       semiconductor manufacturing capability?"
                .to_string(),
            context: None,
            families: vec![],
        };

        let response = worker
            .run_classify_family(&request)
            .await
            .expect("real Python worker round-trips a classify-family request");
        assert_eq!(response.decision, "minted");
        assert!(response.label.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_returns_timeout_error() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let script_path = dir.path().join("iw-slow-worker-stub.sh");
        std::fs::write(&script_path, "#!/bin/sh\ncat >/dev/null\nsleep 30\n")
            .expect("write stub script");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("stat stub script")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).expect("chmod stub script");

        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_millis(200),
            worker_cmd: Some(vec![script_path.to_string_lossy().into_owned()]),
        };

        let started = std::time::Instant::now();
        let result = worker.run_research(&sample_research_request()).await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "run_research() should return promptly once the timeout fires"
        );
        assert!(matches!(result, Err(ResearchError::Timeout(_))));
    }

    /// Reads the grandchild pid `timeout_kills_worker_process_group`'s
    /// stub script writes as soon as it starts, retrying briefly in case
    /// the file has not appeared yet by the time `run_research()` returns.
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

    /// Proves `ResearchWorker::run_research` kills the worker's whole
    /// process group on timeout, not just the immediate child: the stub
    /// script backgrounds a `sleep 30` grandchild and records its pid
    /// before waiting on it, so if only the direct child were killed (what
    /// `kill_on_drop` alone does), the grandchild would be reparented and
    /// keep running.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_kills_worker_process_group() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let script_path = dir.path().join("iw-slow-worker-stub.sh");
        let pid_file = dir.path().join("grandchild.pid");
        let script_contents = format!(
            "#!/bin/sh\ncat >/dev/null\nsleep 30 &\necho $! > \"{}\"\nwait\n",
            pid_file.display()
        );
        std::fs::write(&script_path, script_contents).expect("write stub script");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("stat stub script")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).expect("chmod stub script");

        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_millis(200),
            worker_cmd: Some(vec![script_path.to_string_lossy().into_owned()]),
        };

        let result = worker.run_research(&sample_research_request()).await;
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

    /// Proves `ResearchWorker::run_research` kills the worker's whole
    /// process group when the caller cancels the request (drops the
    /// future), not just on a timeout: starts the call inside a
    /// `tokio::spawn`, waits for the same grandchild-backgrounding stub
    /// script's pid file, then aborts the task instead of waiting for it
    /// to finish.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancelled_run_kills_worker_process_group() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let script_path = dir.path().join("iw-slow-worker-stub.sh");
        let pid_file = dir.path().join("grandchild.pid");
        let script_contents = format!(
            "#!/bin/sh\ncat >/dev/null\nsleep 30 &\necho $! > \"{}\"\nwait\n",
            pid_file.display()
        );
        std::fs::write(&script_path, script_contents).expect("write stub script");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("stat stub script")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).expect("chmod stub script");

        let worker = ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(30),
            worker_cmd: Some(vec![script_path.to_string_lossy().into_owned()]),
        };

        let handle =
            tokio::spawn(async move { worker.run_research(&sample_research_request()).await });

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
