//! `InvokeOpenWikiJob` — subprocess-execs the packaged `openwiki`
//! binary against the target repo, unmodified.
//!
//! Mode (`--init` vs `--update`) is auto-detected by checking whether
//! `openwiki/` already exists in the target repo — OpenWiki's own
//! documented behavior (its README: "creates initial documentation in
//! `openwiki/` when no wiki exists. If `openwiki/` already exists, it
//! refreshes that documentation from repository changes."). ponte just
//! makes that detection explicit rather than relying on OpenWiki's own
//! internal branch.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shigoto_types::{JobScope, JobSubject, OutputSink, RecordingJob};
use tokio::sync::Mutex as AsyncMutex;

use crate::context::ContextOutcome;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunMode {
    Init,
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OpenWikiOutcome {
    /// Upstream context was unchanged — skipped invoking openwiki
    /// entirely. Invoking OpenWiki is a paid, non-deterministic LLM
    /// call, so it is never re-run when there is nothing new to feed
    /// it; this is the idempotency guard shigoto's own contract can't
    /// give us for free (unlike `git pull`, "run an LLM agent again"
    /// has no native no-op floor).
    Skipped,
    Ran { mode: RunMode, stdout_tail: String },
}

#[derive(Debug, thiserror::Error)]
pub enum OpenWikiError {
    #[error("upstream ContextAssembleJob did not produce an outcome")]
    MissingUpstreamOutcome,
    #[error("failed to spawn {binary:?}: {source}")]
    Spawn {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("openwiki timed out after {0:?}")]
    Timeout(Duration),
    #[error("openwiki exited with status {0}")]
    NonZeroExit(std::process::ExitStatus),
}

pub struct InvokeOpenWikiJob {
    pub repo_path: PathBuf,
    pub repo_name: String,
    pub openwiki_binary: PathBuf,
    /// Outer bound on the whole subprocess run. OpenWiki's own
    /// 120s/100KB ceiling bounds individual tool calls inside its own
    /// agent loop, not the full CLI invocation across many of them.
    pub timeout: Duration,
    pub upstream_context: Arc<AsyncMutex<Option<ContextOutcome>>>,
    /// Written at the end of `execute_body` for `RouteJob` to read —
    /// see `ContextAssembleJob::outcome_cell` for why this is a shared
    /// cell rather than a framework-provided input.
    pub own_outcome: Arc<AsyncMutex<Option<OpenWikiOutcome>>>,
    pub output_sink: Option<Arc<dyn OutputSink<OpenWikiOutcome>>>,
}

impl InvokeOpenWikiJob {
    fn run_mode(&self) -> RunMode {
        if self.repo_path.join("openwiki").is_dir() {
            RunMode::Update
        } else {
            RunMode::Init
        }
    }

    async fn run_openwiki(&self, mode: RunMode) -> Result<String, OpenWikiError> {
        let flag = match mode {
            RunMode::Init => "--init",
            RunMode::Update => "--update",
        };
        let mut cmd = tokio::process::Command::new(&self.openwiki_binary);
        cmd.arg(flag)
            .arg("--print")
            .current_dir(&self.repo_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd.spawn().map_err(|source| OpenWikiError::Spawn {
            binary: self.openwiki_binary.clone(),
            source,
        })?;

        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| OpenWikiError::Timeout(self.timeout))?
            .map_err(|source| OpenWikiError::Spawn {
                binary: self.openwiki_binary.clone(),
                source,
            })?;

        if !output.status.success() {
            return Err(OpenWikiError::NonZeroExit(output.status));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[async_trait::async_trait]
impl RecordingJob for InvokeOpenWikiJob {
    type Output = OpenWikiOutcome;
    type Error = OpenWikiError;
    const KIND: &'static str = "ponte.invoke-openwiki";

    fn scope(&self) -> JobScope {
        JobScope::Repo {
            workspace: "ponte".into(),
            repo: self.repo_name.clone(),
        }
    }

    fn subject(&self) -> JobSubject {
        JobSubject::Path(self.repo_path.clone())
    }

    fn output_sink(&self) -> Option<&Arc<dyn OutputSink<Self::Output>>> {
        self.output_sink.as_ref()
    }

    async fn execute_body(&self) -> Result<OpenWikiOutcome, OpenWikiError> {
        let upstream = self
            .upstream_context
            .lock()
            .await
            .clone()
            .ok_or(OpenWikiError::MissingUpstreamOutcome)?;

        let outcome = if !upstream.changed {
            OpenWikiOutcome::Skipped
        } else {
            let mode = self.run_mode();
            let stdout = self.run_openwiki(mode).await?;
            // Cap the captured tail so a chatty --print doesn't bloat
            // the audit/output-sink record; char-based (not byte
            // slicing) so this never panics on a UTF-8 boundary.
            let stdout_tail: String = stdout
                .chars()
                .rev()
                .take(2000)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            OpenWikiOutcome::Ran { mode, stdout_tail }
        };

        *self.own_outcome.lock().await = Some(outcome.clone());
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed_outcome() -> Arc<AsyncMutex<Option<ContextOutcome>>> {
        Arc::new(AsyncMutex::new(Some(ContextOutcome {
            changed: true,
            content_hash: "deadbeef".into(),
            summary_path: PathBuf::from("/dev/null"),
            typescape_present: false,
            zoekt_reachable: false,
        })))
    }

    fn unchanged_outcome() -> Arc<AsyncMutex<Option<ContextOutcome>>> {
        Arc::new(AsyncMutex::new(Some(ContextOutcome {
            changed: false,
            content_hash: "deadbeef".into(),
            summary_path: PathBuf::from("/dev/null"),
            typescape_present: false,
            zoekt_reachable: false,
        })))
    }

    fn job(repo_path: PathBuf, binary: &str, upstream: Arc<AsyncMutex<Option<ContextOutcome>>>) -> InvokeOpenWikiJob {
        InvokeOpenWikiJob {
            repo_path,
            repo_name: "test".into(),
            openwiki_binary: PathBuf::from(binary),
            timeout: Duration::from_secs(5),
            upstream_context: upstream,
            own_outcome: Arc::new(AsyncMutex::new(None)),
            output_sink: None,
        }
    }

    #[tokio::test]
    async fn skips_subprocess_when_upstream_unchanged() {
        let j = job(PathBuf::from("."), "/does/not/matter", unchanged_outcome());
        let outcome = j.execute_body().await.unwrap();
        assert!(matches!(outcome, OpenWikiOutcome::Skipped));
        assert!(matches!(
            j.own_outcome.lock().await.as_ref().unwrap(),
            OpenWikiOutcome::Skipped
        ));
    }

    #[tokio::test]
    async fn errors_when_upstream_outcome_missing() {
        let j = job(PathBuf::from("."), "/does/not/matter", Arc::new(AsyncMutex::new(None)));
        let err = j.execute_body().await.unwrap_err();
        assert!(matches!(err, OpenWikiError::MissingUpstreamOutcome));
    }

    #[tokio::test]
    async fn init_mode_when_no_openwiki_dir() {
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path().to_path_buf(), "echo", changed_outcome());
        assert_eq!(j.run_mode(), RunMode::Init);
        let outcome = j.execute_body().await.unwrap();
        match outcome {
            OpenWikiOutcome::Ran { mode, .. } => assert_eq!(mode, RunMode::Init),
            OpenWikiOutcome::Skipped => panic!("expected Ran"),
        }
    }

    #[tokio::test]
    async fn update_mode_when_openwiki_dir_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("openwiki")).unwrap();
        let j = job(dir.path().to_path_buf(), "echo", changed_outcome());
        assert_eq!(j.run_mode(), RunMode::Update);
        let outcome = j.execute_body().await.unwrap();
        match outcome {
            OpenWikiOutcome::Ran { mode, .. } => assert_eq!(mode, RunMode::Update),
            OpenWikiOutcome::Skipped => panic!("expected Ran"),
        }
    }

    #[tokio::test]
    async fn nonzero_exit_surfaces_as_error() {
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path().to_path_buf(), "false", changed_outcome());
        let err = j.execute_body().await.unwrap_err();
        assert!(matches!(err, OpenWikiError::NonZeroExit(_)));
    }

    #[tokio::test]
    async fn missing_binary_surfaces_spawn_error() {
        let dir = tempfile::tempdir().unwrap();
        let j = job(
            dir.path().to_path_buf(),
            "/definitely/not/a/real/binary/ponte-test",
            changed_outcome(),
        );
        let err = j.execute_body().await.unwrap_err();
        assert!(matches!(err, OpenWikiError::Spawn { .. }));
    }
}
