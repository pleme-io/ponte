//! Small persisted state ponte keeps between separate CLI invocations.
//!
//! shigoto's `InProcessScheduler` is in-memory-only per run — a fresh
//! `ponte` process has no memory of a previous run — so the
//! run-to-run no-op guard (only touch a repo when its typescape/zoekt
//! content actually changed) needs its own tiny durable record. This
//! is deliberately NOT wired through an env var: callers (tests, prod)
//! pass an explicit `state_dir`, keeping the module hermetic.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RunState {
    pub last_content_hash: Option<String>,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Slug a repo path into a filesystem-safe key for its state/audit file
/// names. Non-alphanumeric bytes collapse to `-`; canonicalizes first
/// so relative and absolute paths to the same repo share one slug.
#[must_use]
pub fn repo_slug(repo_path: &Path) -> String {
    let canonical = repo_path
        .canonicalize()
        .unwrap_or_else(|_| repo_path.to_path_buf());
    let raw = canonical.to_string_lossy();
    let slug: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    slug.trim_matches('-').to_string()
}

fn state_path(state_dir: &Path, repo_path: &Path) -> PathBuf {
    state_dir.join(format!("{}.json", repo_slug(repo_path)))
}

/// Read the persisted state for `repo_path`. Missing/corrupt state
/// reads as `RunState::default()` — a fresh repo has no prior run,
/// not an error.
#[must_use]
pub fn load(state_dir: &Path, repo_path: &Path) -> RunState {
    let path = state_path(state_dir, repo_path);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist `state` for `repo_path`. Creates `state_dir` if absent.
pub fn save(state_dir: &Path, repo_path: &Path, state: &RunState) -> anyhow::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_path(state_dir, repo_path);
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_replaces_non_alphanumeric() {
        let slug = repo_slug(Path::new("/tmp/does-not-exist-xyz/my repo!"));
        assert!(!slug.contains('/'));
        assert!(!slug.contains(' '));
        assert!(!slug.contains('!'));
    }

    #[test]
    fn load_missing_state_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let state = load(dir.path(), Path::new("/tmp/ponte-test-nonexistent-repo"));
        assert!(state.last_content_hash.is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Path::new("/tmp/ponte-test-repo-roundtrip");
        let state = RunState {
            last_content_hash: Some("abc123".into()),
            last_run_at: Some(chrono::Utc::now()),
        };
        save(dir.path(), repo, &state).unwrap();
        let loaded = load(dir.path(), repo);
        assert_eq!(loaded, state);
    }

    #[test]
    fn two_repos_get_distinct_state_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo_a = Path::new("/tmp/ponte-test-repo-a");
        let repo_b = Path::new("/tmp/ponte-test-repo-b");
        save(
            dir.path(),
            repo_a,
            &RunState {
                last_content_hash: Some("a".into()),
                last_run_at: None,
            },
        )
        .unwrap();
        save(
            dir.path(),
            repo_b,
            &RunState {
                last_content_hash: Some("b".into()),
                last_run_at: None,
            },
        )
        .unwrap();
        assert_eq!(load(dir.path(), repo_a).last_content_hash.unwrap(), "a");
        assert_eq!(load(dir.path(), repo_b).last_content_hash.unwrap(), "b");
    }
}
