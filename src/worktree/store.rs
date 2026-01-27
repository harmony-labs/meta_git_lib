//! Centralized worktree store operations.
//!
//! Manages `~/.meta/worktree.json` — the persistent record of all worktrees.

use anyhow::Result;
use std::path::{Path, PathBuf};

use super::types::{StoreRepoEntry, WorktreeStoreData, WorktreeStoreEntry};

/// Derive the store key from a worktree path.
fn store_key(worktree_path: &Path) -> String {
    worktree_path.to_string_lossy().to_string()
}

fn store_path() -> PathBuf {
    meta_core::data_dir::data_file("worktree")
}

fn store_lock_path(data_path: &Path) -> PathBuf {
    data_path.with_extension("lock")
}

/// Return (data_path, lock_path) for the worktree store.
fn store_paths() -> (PathBuf, PathBuf) {
    let data_path = store_path();
    let lock_path = store_lock_path(&data_path);
    (data_path, lock_path)
}

/// Add a worktree entry to the centralized store.
pub fn store_add(worktree_path: &Path, entry: WorktreeStoreEntry) -> Result<()> {
    meta_core::data_dir::ensure_meta_dir()?;
    let (data_path, lock_path) = store_paths();
    let key = store_key(worktree_path);

    meta_core::store::update::<WorktreeStoreData, _>(&data_path, &lock_path, |store| {
        store.worktrees.insert(key, entry);
    })
}

/// Remove a worktree entry from the centralized store.
pub fn store_remove(worktree_path: &Path) -> Result<()> {
    let (data_path, lock_path) = store_paths();
    if !data_path.exists() {
        return Ok(());
    }
    let key = store_key(worktree_path);

    meta_core::store::update::<WorktreeStoreData, _>(&data_path, &lock_path, |store| {
        store.worktrees.remove(&key);
    })
}

/// Get all entries from the store.
pub fn store_list() -> Result<WorktreeStoreData> {
    meta_core::store::read(&store_path())
}

/// Add repos to an existing worktree entry in the store.
pub fn store_extend_repos(worktree_path: &Path, repos: Vec<StoreRepoEntry>) -> Result<()> {
    let (data_path, lock_path) = store_paths();
    let key = store_key(worktree_path);

    meta_core::store::update::<WorktreeStoreData, _>(&data_path, &lock_path, move |store| {
        if let Some(entry) = store.worktrees.get_mut(&key) {
            entry.repos.extend(repos);
        }
    })
}

/// Remove multiple worktree entries from the store in a single lock cycle.
pub fn store_remove_batch(keys: &[String]) -> Result<()> {
    let (data_path, lock_path) = store_paths();
    if !data_path.exists() {
        return Ok(());
    }

    meta_core::store::update::<WorktreeStoreData, _>(&data_path, &lock_path, |store| {
        for key in keys {
            store.worktrees.remove(key);
        }
    })
}

/// Compute TTL remaining seconds for a store entry.
/// Returns `None` if no TTL is set. Negative means expired.
/// On malformed `created_at`, warns and treats as not expired.
pub fn entry_ttl_remaining(entry: &WorktreeStoreEntry, now_epoch: i64) -> Option<i64> {
    entry.ttl_seconds.map(|ttl| {
        let created = match chrono::DateTime::parse_from_rfc3339(&entry.created_at) {
            Ok(dt) => dt.timestamp(),
            Err(e) => {
                log::warn!(
                    "Malformed created_at '{}' in store entry '{}': {}",
                    entry.created_at,
                    entry.name,
                    e
                );
                return i64::MAX;
            }
        };
        created + ttl as i64 - now_epoch
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_entry(created_at: &str, ttl_seconds: Option<u64>) -> WorktreeStoreEntry {
        WorktreeStoreEntry {
            name: "test-wt".to_string(),
            project: "/tmp/project".to_string(),
            created_at: created_at.to_string(),
            ephemeral: ttl_seconds.is_some(),
            ttl_seconds,
            repos: vec![],
            custom: HashMap::new(),
        }
    }

    // ── store_key ───────────────────────────────────────────

    #[test]
    fn store_key_returns_path_string() {
        let path = std::path::Path::new("/home/user/.worktrees/feat-1");
        assert_eq!(store_key(path), "/home/user/.worktrees/feat-1");
    }

    // ── entry_ttl_remaining ─────────────────────────────────

    #[test]
    fn ttl_remaining_none_when_no_ttl() {
        let entry = make_entry("2025-01-01T00:00:00Z", None);
        assert!(entry_ttl_remaining(&entry, 1_700_000_000).is_none());
    }

    #[test]
    fn ttl_remaining_positive_when_not_expired() {
        // created_at = 2025-01-01T00:00:00Z = epoch 1735689600
        // ttl = 3600s (1 hour)
        // now = 1735689600 + 1800 (30 min later)
        // remaining = 1735689600 + 3600 - (1735689600 + 1800) = 1800
        let entry = make_entry("2025-01-01T00:00:00Z", Some(3600));
        let created_epoch = 1_735_689_600i64;
        let remaining = entry_ttl_remaining(&entry, created_epoch + 1800).unwrap();
        assert_eq!(remaining, 1800);
    }

    #[test]
    fn ttl_remaining_negative_when_expired() {
        // created + ttl < now → negative
        let entry = make_entry("2025-01-01T00:00:00Z", Some(3600));
        let created_epoch = 1_735_689_600i64;
        let remaining = entry_ttl_remaining(&entry, created_epoch + 7200).unwrap();
        assert_eq!(remaining, -3600);
    }

    #[test]
    fn ttl_remaining_zero_at_exact_expiry() {
        let entry = make_entry("2025-01-01T00:00:00Z", Some(3600));
        let created_epoch = 1_735_689_600i64;
        let remaining = entry_ttl_remaining(&entry, created_epoch + 3600).unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn ttl_remaining_malformed_date_returns_max() {
        let entry = make_entry("not-a-date", Some(3600));
        let remaining = entry_ttl_remaining(&entry, 1_700_000_000).unwrap();
        assert_eq!(remaining, i64::MAX);
    }
}
