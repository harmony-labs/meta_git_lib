//! Workspace snapshot functionality for meta repositories.
//!
//! Captures and restores the git state of all repos in a meta workspace,
//! enabling safe batch operations with rollback capability.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use meta_cli::git_utils;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const SNAPSHOTS_DIR: &str = ".meta-snapshots";

/// State of a single repository at snapshot time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoState {
    /// The commit SHA at snapshot time
    pub sha: String,
    /// The branch name (None if detached HEAD)
    pub branch: Option<String>,
    /// Whether the repo had uncommitted changes
    pub dirty: bool,
    /// Whether a stash was created during restore (only set after restore)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stash_created: bool,
}

/// A complete workspace snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Name of the snapshot
    pub name: String,
    /// When the snapshot was created
    pub created: DateTime<Utc>,
    /// State of each repository (key is relative path from meta root)
    pub repos: HashMap<String, RepoState>,
}

/// Summary info about a snapshot (for listing)
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotInfo {
    pub name: String,
    pub created: DateTime<Utc>,
    pub repo_count: usize,
    pub dirty_count: usize,
}

/// Result of a restore operation for a single repo
#[derive(Debug, Clone, Serialize)]
pub struct RestoreResult {
    pub repo: String,
    pub success: bool,
    pub stashed: bool,
    pub message: String,
}

/// Capture the current git state of a repository
pub fn capture_repo_state(repo_path: &Path) -> Result<RepoState> {
    // Get current SHA
    let sha_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to run git rev-parse HEAD")?;

    if !sha_output.status.success() {
        anyhow::bail!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&sha_output.stderr)
        );
    }

    let sha = String::from_utf8_lossy(&sha_output.stdout).trim().to_string();

    let branch = git_utils::current_branch(repo_path);

    let dirty = git_utils::is_dirty(repo_path).unwrap_or(false);

    Ok(RepoState {
        sha,
        branch,
        dirty,
        stash_created: false,
    })
}

/// Restore a repository to a snapshot state
pub fn restore_repo_state(repo_path: &Path, state: &RepoState, force: bool) -> Result<RestoreResult> {
    let repo_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    // Check if repo is dirty and needs stashing
    let is_dirty = git_utils::is_dirty(repo_path).unwrap_or(false);

    let mut stashed = false;

    // If dirty, stash changes first
    if is_dirty {
        if !force {
            // In non-force mode, we've already confirmed with user
        }

        let stash_output = Command::new("git")
            .args(["stash", "push", "-m", "meta-snapshot-auto-stash"])
            .current_dir(repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to stash changes")?;

        if !stash_output.status.success() {
            return Ok(RestoreResult {
                repo: repo_name,
                success: false,
                stashed: false,
                message: format!(
                    "Failed to stash: {}",
                    String::from_utf8_lossy(&stash_output.stderr)
                ),
            });
        }
        stashed = true;
    }

    // Checkout to the snapshot SHA
    let checkout_output = Command::new("git")
        .args(["checkout", &state.sha])
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to checkout SHA")?;

    if !checkout_output.status.success() {
        let stderr = String::from_utf8_lossy(&checkout_output.stderr);
        // Check if SHA doesn't exist (likely garbage collected)
        if stderr.contains("did not match any") || stderr.contains("not a commit") {
            return Ok(RestoreResult {
                repo: repo_name,
                success: false,
                stashed,
                message: format!(
                    "SHA {} no longer exists. Check git reflog for recovery options.",
                    &state.sha[..8]
                ),
            });
        }
        return Ok(RestoreResult {
            repo: repo_name,
            success: false,
            stashed,
            message: format!("Failed to checkout: {}", stderr),
        });
    }

    // If was on a branch, restore branch pointer
    if let Some(ref branch) = state.branch {
        let branch_output = Command::new("git")
            .args(["checkout", "-B", branch, &state.sha])
            .current_dir(repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to restore branch")?;

        if !branch_output.status.success() {
            // Non-fatal: we're at the right SHA, just not on the branch
            return Ok(RestoreResult {
                repo: repo_name,
                success: true,
                stashed,
                message: format!(
                    "Restored to {} (couldn't restore branch '{}')",
                    &state.sha[..8],
                    branch
                ),
            });
        }

        Ok(RestoreResult {
            repo: repo_name,
            success: true,
            stashed,
            message: format!("{} -> {}", &state.sha[..8], branch),
        })
    } else {
        Ok(RestoreResult {
            repo: repo_name,
            success: true,
            stashed,
            message: format!("{} (detached)", &state.sha[..8]),
        })
    }
}

/// Save a snapshot to disk
pub fn save_snapshot(meta_root: &Path, snapshot: &Snapshot) -> Result<()> {
    let snapshots_dir = meta_root.join(SNAPSHOTS_DIR);
    fs::create_dir_all(&snapshots_dir).context("Failed to create snapshots directory")?;

    let snapshot_path = snapshots_dir.join(format!("{}.json", snapshot.name));
    let json = serde_json::to_string_pretty(snapshot).context("Failed to serialize snapshot")?;

    fs::write(&snapshot_path, json).context("Failed to write snapshot file")?;

    Ok(())
}

/// Load a snapshot from disk
pub fn load_snapshot(meta_root: &Path, name: &str) -> Result<Snapshot> {
    let snapshot_path = meta_root.join(SNAPSHOTS_DIR).join(format!("{}.json", name));

    if !snapshot_path.exists() {
        anyhow::bail!("Snapshot '{}' not found", name);
    }

    let json = fs::read_to_string(&snapshot_path).context("Failed to read snapshot file")?;
    let snapshot: Snapshot = serde_json::from_str(&json).context("Failed to parse snapshot")?;

    Ok(snapshot)
}

/// List all snapshots in a meta root
pub fn list_snapshots(meta_root: &Path) -> Result<Vec<SnapshotInfo>> {
    let snapshots_dir = meta_root.join(SNAPSHOTS_DIR);

    if !snapshots_dir.exists() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();

    for entry in fs::read_dir(&snapshots_dir).context("Failed to read snapshots directory")? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(json) = fs::read_to_string(&path) {
                if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&json) {
                    let dirty_count = snapshot.repos.values().filter(|r| r.dirty).count();
                    snapshots.push(SnapshotInfo {
                        name: snapshot.name,
                        created: snapshot.created,
                        repo_count: snapshot.repos.len(),
                        dirty_count,
                    });
                }
            }
        }
    }

    // Sort by creation time, newest first
    snapshots.sort_by(|a, b| b.created.cmp(&a.created));

    Ok(snapshots)
}

/// Delete a snapshot
pub fn delete_snapshot(meta_root: &Path, name: &str) -> Result<()> {
    let snapshot_path = meta_root.join(SNAPSHOTS_DIR).join(format!("{}.json", name));

    if !snapshot_path.exists() {
        anyhow::bail!("Snapshot '{}' not found", name);
    }

    fs::remove_file(&snapshot_path).context("Failed to delete snapshot file")?;

    Ok(())
}

/// Check if a path is a git repository
pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists() || path.join(".git").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_repo(dir: &Path) -> Result<()> {
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()?;
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()?;
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()?;
        fs::write(dir.join("README.md"), "# Test")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(dir)
            .output()?;
        Ok(())
    }

    #[test]
    fn test_capture_clean_repo() {
        let temp = TempDir::new().unwrap();
        create_test_repo(temp.path()).unwrap();

        let state = capture_repo_state(temp.path()).unwrap();
        assert!(!state.sha.is_empty());
        assert!(!state.dirty);
        // Default branch could be main or master
        assert!(state.branch.is_some());
    }

    #[test]
    fn test_capture_dirty_repo() {
        let temp = TempDir::new().unwrap();
        create_test_repo(temp.path()).unwrap();

        // Make it dirty
        fs::write(temp.path().join("new_file.txt"), "dirty").unwrap();

        let state = capture_repo_state(temp.path()).unwrap();
        assert!(state.dirty);
    }

    #[test]
    fn test_save_and_load_snapshot() {
        let temp = TempDir::new().unwrap();

        let snapshot = Snapshot {
            name: "test-snapshot".to_string(),
            created: Utc::now(),
            repos: HashMap::from([(
                ".".to_string(),
                RepoState {
                    sha: "abc123".to_string(),
                    branch: Some("main".to_string()),
                    dirty: false,
                    stash_created: false,
                },
            )]),
        };

        save_snapshot(temp.path(), &snapshot).unwrap();
        let loaded = load_snapshot(temp.path(), "test-snapshot").unwrap();

        assert_eq!(loaded.name, "test-snapshot");
        assert_eq!(loaded.repos.len(), 1);
        assert_eq!(loaded.repos["."].sha, "abc123");
    }

    #[test]
    fn test_list_snapshots() {
        let temp = TempDir::new().unwrap();

        // Create two snapshots
        for name in ["snap1", "snap2"] {
            let snapshot = Snapshot {
                name: name.to_string(),
                created: Utc::now(),
                repos: HashMap::new(),
            };
            save_snapshot(temp.path(), &snapshot).unwrap();
        }

        let list = list_snapshots(temp.path()).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_delete_snapshot() {
        let temp = TempDir::new().unwrap();

        let snapshot = Snapshot {
            name: "to-delete".to_string(),
            created: Utc::now(),
            repos: HashMap::new(),
        };
        save_snapshot(temp.path(), &snapshot).unwrap();

        assert!(load_snapshot(temp.path(), "to-delete").is_ok());

        delete_snapshot(temp.path(), "to-delete").unwrap();

        assert!(load_snapshot(temp.path(), "to-delete").is_err());
    }

    #[test]
    fn test_is_git_repo() {
        let temp = TempDir::new().unwrap();
        assert!(!is_git_repo(temp.path()));

        create_test_repo(temp.path()).unwrap();
        assert!(is_git_repo(temp.path()));
    }
}
