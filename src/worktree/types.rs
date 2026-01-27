//! Types for worktree management.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ==================== Domain Types ====================

/// A repo specifier: `alias` or `alias:branch`
#[derive(Debug, Clone)]
pub struct RepoSpec {
    pub alias: String,
    pub branch: Option<String>,
}

impl std::fmt::Display for RepoSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.branch {
            Some(branch) => write!(f, "{}:{}", self.alias, branch),
            None => write!(f, "{}", self.alias),
        }
    }
}

impl std::str::FromStr for RepoSpec {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.split_once(':') {
            Some((alias, branch)) => Ok(RepoSpec {
                alias: alias.to_string(),
                branch: Some(branch.to_string()),
            }),
            None => Ok(RepoSpec {
                alias: s.to_string(),
                branch: None,
            }),
        }
    }
}

impl From<&CreateRepoEntry> for StoreRepoEntry {
    fn from(r: &CreateRepoEntry) -> Self {
        StoreRepoEntry {
            alias: r.alias.clone(),
            branch: r.branch.clone(),
            created_branch: r.created_branch,
        }
    }
}

// ==================== Internal Types ====================

/// Resolved worktree context for operations that need meta_dir, worktree_root, and worktree path.
pub struct WorktreeContext {
    pub meta_dir: Option<PathBuf>,
    pub worktree_root: PathBuf,
    pub wt_dir: PathBuf,
}

// ==================== Centralized Store Types ====================

/// Top-level store structure at `~/.meta/worktree.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WorktreeStoreData {
    pub worktrees: HashMap<String, WorktreeStoreEntry>,
}

/// Individual worktree entry in the centralized store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeStoreEntry {
    pub name: String,
    pub project: String,
    pub created_at: String,
    pub ephemeral: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    pub repos: Vec<StoreRepoEntry>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, String>,
}

/// Repo entry within a store entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreRepoEntry {
    pub alias: String,
    pub branch: String,
    pub created_branch: bool,
}

// ==================== JSON Output Structures ====================

#[derive(Debug, Serialize)]
pub struct CreateOutput {
    pub name: String,
    pub root: String,
    pub repos: Vec<CreateRepoEntry>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ephemeral: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct CreateRepoEntry {
    pub alias: String,
    pub path: String,
    pub branch: String,
    pub created_branch: bool,
}

#[derive(Debug, Serialize)]
pub struct ListOutput {
    pub worktrees: Vec<ListEntry>,
}

#[derive(Debug, Serialize)]
pub struct AddOutput {
    pub name: String,
    pub repos: Vec<CreateRepoEntry>,
}

#[derive(Debug, Serialize)]
pub struct DestroyOutput {
    pub name: String,
    pub path: String,
    pub repos_removed: usize,
}

#[derive(Debug, Serialize)]
pub struct ListEntry {
    pub name: String,
    pub root: String,
    pub has_meta_root: bool,
    pub repos: Vec<ListRepoEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_remaining_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize)]
pub struct ListRepoEntry {
    pub alias: String,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub name: String,
    pub repos: Vec<StatusRepoEntry>,
}

#[derive(Debug, Serialize)]
pub struct StatusRepoEntry {
    pub alias: String,
    pub path: String,
    pub branch: String,
    pub dirty: bool,
    pub modified_count: usize,
    pub untracked_count: usize,
    pub ahead: u32,
    pub behind: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modified_files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DiffOutput {
    pub name: String,
    pub base: String,
    pub repos: Vec<DiffRepoEntry>,
    pub totals: DiffTotals,
}

#[derive(Debug, Serialize)]
pub struct DiffRepoEntry {
    pub alias: String,
    pub base_ref: String,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DiffTotals {
    pub repos_changed: usize,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Debug, Serialize)]
pub struct PruneOutput {
    pub removed: Vec<PruneEntry>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PruneEntry {
    pub name: String,
    pub path: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<u64>,
}

// ==================== Git Status ====================

/// Combined git status summary from a single `git status --porcelain` call.
pub struct GitStatusSummary {
    pub dirty: bool,
    pub modified_files: Vec<String>,
    pub untracked_count: usize,
}
