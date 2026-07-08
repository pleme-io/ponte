//! `RouteJob` — reconciles OpenWiki's generated `openwiki/` output
//! against the repo's existing `CLAUDE.md`, per the `context` skill's
//! pointers-over-inlining principle: a small pointer section, never a
//! duplicated copy of OpenWiki's own content.
//!
//! Confluence routing (the akeyless-architecture-docs concern) is not
//! implemented here — Phase 1 targets pleme-io repos only.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use shigoto_types::{JobScope, JobSubject, OutputSink, RecordingJob};
use tokio::sync::Mutex as AsyncMutex;

use crate::openwiki::OpenWikiOutcome;

const POINTER_MARKER: &str = "<!-- ponte:openwiki-pointer -->";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RouteOutcome {
    /// Upstream openwiki invocation was skipped, missing, or failed —
    /// nothing to route. Per the plan: never route a partial doc.
    Skipped,
    PointerAdded,
    PointerAlreadyPresent,
}

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("failed to read/write CLAUDE.md: {0}")]
    Io(#[from] std::io::Error),
}

pub struct RouteJob {
    pub repo_path: PathBuf,
    pub repo_name: String,
    /// Same cell `InvokeOpenWikiJob::own_outcome` writes into.
    pub upstream_outcome: Arc<AsyncMutex<Option<OpenWikiOutcome>>>,
    pub output_sink: Option<Arc<dyn OutputSink<RouteOutcome>>>,
}

impl RouteJob {
    fn pointer_block() -> String {
        format!(
            "\n{marker}\n## Generated documentation\n\nOpenWiki-generated docs for this repo live under [`openwiki/`](./openwiki/quickstart.md) — start at `openwiki/quickstart.md`.\n{marker}\n",
            marker = POINTER_MARKER
        )
    }

    async fn ensure_pointer(&self) -> Result<RouteOutcome, RouteError> {
        let claude_md = self.repo_path.join("CLAUDE.md");
        let existing = tokio::fs::read_to_string(&claude_md)
            .await
            .unwrap_or_default();

        if existing.contains(POINTER_MARKER) {
            return Ok(RouteOutcome::PointerAlreadyPresent);
        }

        let mut updated = existing;
        updated.push_str(&Self::pointer_block());
        tokio::fs::write(&claude_md, updated).await?;
        Ok(RouteOutcome::PointerAdded)
    }
}

#[async_trait::async_trait]
impl RecordingJob for RouteJob {
    type Output = RouteOutcome;
    type Error = RouteError;
    const KIND: &'static str = "ponte.route";

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

    async fn execute_body(&self) -> Result<RouteOutcome, RouteError> {
        let upstream = self.upstream_outcome.lock().await.clone();
        match upstream {
            Some(OpenWikiOutcome::Ran { .. }) => self.ensure_pointer().await,
            Some(OpenWikiOutcome::Skipped) | None => Ok(RouteOutcome::Skipped),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openwiki::RunMode;

    fn ran_outcome() -> Arc<AsyncMutex<Option<OpenWikiOutcome>>> {
        Arc::new(AsyncMutex::new(Some(OpenWikiOutcome::Ran {
            mode: RunMode::Init,
            stdout_tail: String::new(),
        })))
    }

    fn job(repo_path: PathBuf, upstream: Arc<AsyncMutex<Option<OpenWikiOutcome>>>) -> RouteJob {
        RouteJob {
            repo_path,
            repo_name: "test".into(),
            upstream_outcome: upstream,
            output_sink: None,
        }
    }

    #[tokio::test]
    async fn skips_when_upstream_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = job(
            dir.path().to_path_buf(),
            Arc::new(AsyncMutex::new(Some(OpenWikiOutcome::Skipped))),
        )
        .execute_body()
        .await
        .unwrap();
        assert!(matches!(outcome, RouteOutcome::Skipped));
    }

    #[tokio::test]
    async fn skips_when_upstream_missing() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = job(dir.path().to_path_buf(), Arc::new(AsyncMutex::new(None)))
            .execute_body()
            .await
            .unwrap();
        assert!(matches!(outcome, RouteOutcome::Skipped));
    }

    #[tokio::test]
    async fn adds_pointer_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# My repo\n").unwrap();
        let outcome = job(dir.path().to_path_buf(), ran_outcome())
            .execute_body()
            .await
            .unwrap();
        assert!(matches!(outcome, RouteOutcome::PointerAdded));
        let contents = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(contents.contains("openwiki/quickstart.md"));
        assert!(contents.contains("# My repo"));
    }

    #[tokio::test]
    async fn creates_claude_md_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        job(dir.path().to_path_buf(), ran_outcome())
            .execute_body()
            .await
            .unwrap();
        assert!(dir.path().join("CLAUDE.md").exists());
    }

    #[tokio::test]
    async fn second_run_is_idempotent_and_never_duplicates_pointer() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# My repo\n").unwrap();

        job(dir.path().to_path_buf(), ran_outcome())
            .execute_body()
            .await
            .unwrap();
        let outcome = job(dir.path().to_path_buf(), ran_outcome())
            .execute_body()
            .await
            .unwrap();

        assert!(matches!(outcome, RouteOutcome::PointerAlreadyPresent));
        let contents = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        // Marker appears exactly twice (open + close of one block) —
        // a second run must not have appended a duplicate block.
        assert_eq!(contents.matches(POINTER_MARKER).count(), 2);
    }
}
