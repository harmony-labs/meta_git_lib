use log::debug;
use meta_core::config;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A clone task representing a single repository to clone
#[derive(Debug, Clone)]
pub struct CloneTask {
    /// Display name for progress output
    pub name: String,
    /// Git URL to clone from
    pub url: String,
    /// Target path to clone into
    pub target_path: PathBuf,
    /// Depth level (for display purposes)
    pub depth_level: usize,
    /// Whether this project is itself a meta-repo (declared with `meta: true` in config)
    pub is_meta: bool,
}

/// Thread-safe queue for managing clone tasks with dynamic discovery
pub struct CloneQueue {
    /// Pending tasks to process
    pending: Mutex<Vec<CloneTask>>,
    /// Completed task paths (to avoid duplicates)
    completed: Mutex<HashSet<PathBuf>>,
    /// Failed task paths
    failed: Mutex<HashSet<PathBuf>>,
    /// Total tasks discovered (for progress display)
    total_discovered: AtomicUsize,
    /// Total tasks completed
    total_completed: AtomicUsize,
    /// Git depth argument (if any)
    git_depth: Option<String>,
    /// Max meta depth for recursion (None = unlimited)
    meta_depth: Option<usize>,
}

impl CloneQueue {
    pub fn new(git_depth: Option<String>, meta_depth: Option<usize>) -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            completed: Mutex::new(HashSet::new()),
            failed: Mutex::new(HashSet::new()),
            total_discovered: AtomicUsize::new(0),
            total_completed: AtomicUsize::new(0),
            git_depth,
            meta_depth,
        }
    }

    /// Add a task to the queue if not already completed or pending
    pub fn push(&self, task: CloneTask) -> bool {
        let path = task.target_path.clone();

        // Check if already completed
        {
            let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
            if completed.contains(&path) {
                return false;
            }
        }

        // Add to pending
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            // Check if already in pending
            if pending.iter().any(|t| t.target_path == path) {
                return false;
            }
            pending.push(task);
            self.total_discovered.fetch_add(1, Ordering::SeqCst);
        }

        true
    }

    /// Add multiple tasks from a .meta file
    pub fn push_from_meta(&self, base_dir: &Path, depth_level: usize) -> anyhow::Result<usize> {
        // Check meta depth limit
        if let Some(max_depth) = self.meta_depth {
            if depth_level > max_depth {
                debug!(
                    "Skipping nested discovery at depth {} (max: {})",
                    depth_level, max_depth
                );
                return Ok(0);
            }
        }

        let Some((meta_path, _format)) = config::find_meta_config_in(base_dir) else {
            debug!(
                "No .meta config found in {}",
                base_dir.display()
            );
            return Ok(0);
        };

        let (projects, _) = config::parse_meta_config(&meta_path)?;
        debug!(
            "Discovered {} projects in {} at depth {}",
            projects.len(),
            base_dir.display(),
            depth_level
        );

        let mut added = 0;
        for project in projects {
            let target_path = base_dir.join(&project.path);

            // Skip if already exists
            if target_path.exists() {
                // But still check if it has a config file for nested discovery
                if config::find_meta_config_in(&target_path).is_some() {
                    // Queue it for discovery even though it's already cloned
                    added += self.push_from_meta(&target_path, depth_level + 1)?;
                }
                continue;
            }

            // Skip projects without a repo URL (cannot clone)
            let Some(url) = project.repo else {
                continue;
            };

            let task = CloneTask {
                name: project.name.clone(),
                url,
                target_path,
                depth_level,
                is_meta: project.meta,
            };

            let task_name = task.name.clone();
            let task_is_meta = task.is_meta;
            if self.push(task) {
                debug!(
                    "Queued clone task: {} (depth: {}, is_meta: {})",
                    task_name, depth_level, task_is_meta
                );
                added += 1;
            }
        }

        Ok(added)
    }

    /// Take a single task from the queue (for worker threads)
    pub fn take_one(&self) -> Option<CloneTask> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.pop()
    }

    /// Check if queue is finished (no pending and no active workers)
    pub fn is_finished(&self, active_workers: &AtomicUsize) -> bool {
        let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.is_empty() && active_workers.load(Ordering::SeqCst) == 0
    }

    /// Drain all pending tasks (for dry-run display)
    pub fn drain_all(&self) -> Vec<CloneTask> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.drain(..).collect()
    }

    /// Get current counts for display
    pub fn get_counts(&self) -> (usize, usize) {
        (
            self.total_completed.load(Ordering::SeqCst),
            self.total_discovered.load(Ordering::SeqCst),
        )
    }

    /// Get the git depth argument (if any)
    pub fn git_depth(&self) -> Option<&str> {
        self.git_depth.as_deref()
    }

    /// Mark a task as completed and check for nested .meta files
    pub fn mark_completed(&self, task: &CloneTask) -> anyhow::Result<usize> {
        self.total_completed.fetch_add(1, Ordering::SeqCst);

        {
            let mut completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
            completed.insert(task.target_path.clone());
        }

        // Check for nested .meta file and add children to queue
        let added = self.push_from_meta(&task.target_path, task.depth_level + 1)?;
        debug!(
            "mark_completed: {} -> {} nested tasks discovered",
            task.name, added
        );

        // Warn if the config declared meta: true but no nested .meta was found
        if task.is_meta && added == 0 {
            eprintln!(
                "warning: '{}' is declared with `meta: true` but no .meta config was found inside it",
                task.name
            );
        }

        Ok(added)
    }

    /// Mark a task as failed
    pub fn mark_failed(&self, task: &CloneTask) {
        self.total_completed.fetch_add(1, Ordering::SeqCst);

        let mut failed = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        failed.insert(task.target_path.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(name: &str, path: &Path) -> CloneTask {
        CloneTask {
            name: name.to_string(),
            url: format!("git@github.com:org/{name}.git"),
            target_path: path.to_path_buf(),
            depth_level: 0,
            is_meta: false,
        }
    }

    // ── push / dedup ──────────────────────────────────────────

    #[test]
    fn push_adds_task_and_increments_count() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        let task = make_task("repo1", &dir.path().join("repo1"));

        assert!(queue.push(task));
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn push_dedup_by_path() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo1");

        let task1 = make_task("repo1", &path);
        let task2 = make_task("repo1-dup", &path);

        assert!(queue.push(task1));
        assert!(!queue.push(task2)); // same path → rejected
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn push_rejects_completed_path() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo1");

        // Manually mark path as completed
        {
            let mut completed = queue.completed.lock().unwrap();
            completed.insert(path.clone());
        }

        let task = make_task("repo1", &path);
        assert!(!queue.push(task));
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 0);
    }

    // ── take_one ──────────────────────────────────────────────

    #[test]
    fn take_one_returns_none_when_empty() {
        let queue = CloneQueue::new(None, None);
        assert!(queue.take_one().is_none());
    }

    #[test]
    fn take_one_returns_task_when_available() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        queue.push(make_task("repo1", &dir.path().join("repo1")));

        let task = queue.take_one();
        assert!(task.is_some());
        assert_eq!(task.unwrap().name, "repo1");
        assert!(queue.take_one().is_none()); // now empty
    }

    // ── is_finished ───────────────────────────────────────────

    #[test]
    fn is_finished_true_when_empty_and_no_active() {
        let queue = CloneQueue::new(None, None);
        let active = AtomicUsize::new(0);
        assert!(queue.is_finished(&active));
    }

    #[test]
    fn is_finished_false_when_pending() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        queue.push(make_task("repo1", &dir.path().join("repo1")));

        let active = AtomicUsize::new(0);
        assert!(!queue.is_finished(&active));
    }

    #[test]
    fn is_finished_false_when_active_workers() {
        let queue = CloneQueue::new(None, None);
        let active = AtomicUsize::new(1);
        assert!(!queue.is_finished(&active));
    }

    // ── push_from_meta ────────────────────────────────────────

    #[test]
    fn push_from_meta_discovers_projects() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta"),
            r#"{"projects": {
                "alpha": "git@github.com:org/alpha.git",
                "beta": "git@github.com:org/beta.git"
            }}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 2);
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn push_from_meta_no_config_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let queue = CloneQueue::new(None, None);
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 0);
    }

    #[test]
    fn push_from_meta_respects_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta"),
            r#"{"projects": {"repo": "git@github.com:org/repo.git"}}"#,
        )
        .unwrap();

        // meta_depth = Some(0) means only depth 0 is allowed
        let queue = CloneQueue::new(None, Some(0));
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 1); // depth 0 is allowed

        // Trying at depth 1 should be blocked
        let added = queue.push_from_meta(dir.path(), 1).unwrap();
        assert_eq!(added, 0);
    }

    #[test]
    fn push_from_meta_skips_existing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // Create the project directory so it appears "already cloned"
        std::fs::create_dir(dir.path().join("existing")).unwrap();
        std::fs::write(
            dir.path().join(".meta"),
            r#"{"projects": {"existing": "git@github.com:org/existing.git"}}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 0); // skipped because dir exists
    }

    #[test]
    fn push_from_meta_skips_projects_without_repo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta"),
            r#"{"projects": {"no-repo": {"path": "no-repo"}}}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 0);
    }

    #[test]
    fn push_from_meta_preserves_is_meta_flag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta"),
            r#"{"projects": {
                "nested": {"repo": "git@github.com:org/nested.git", "meta": true},
                "plain": "git@github.com:org/plain.git"
            }}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        queue.push_from_meta(dir.path(), 0).unwrap();

        let tasks = queue.drain_all();
        let nested = tasks.iter().find(|t| t.name == "nested").unwrap();
        let plain = tasks.iter().find(|t| t.name == "plain").unwrap();

        assert!(nested.is_meta);
        assert!(!plain.is_meta);
    }

    // ── mark_completed / nested discovery ─────────────────────

    #[test]
    fn mark_completed_discovers_nested_meta() {
        let dir = tempfile::tempdir().unwrap();
        let child_dir = dir.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();

        // Child has its own .meta with a grandchild
        std::fs::write(
            child_dir.join(".meta"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        let task = CloneTask {
            name: "child".to_string(),
            url: "git@github.com:org/child.git".to_string(),
            target_path: child_dir,
            depth_level: 0,
            is_meta: true,
        };

        let added = queue.mark_completed(&task).unwrap();
        assert_eq!(added, 1);
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn mark_completed_no_nested_meta() {
        let dir = tempfile::tempdir().unwrap();
        let child_dir = dir.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();
        // No .meta inside child

        let queue = CloneQueue::new(None, None);
        let task = CloneTask {
            name: "child".to_string(),
            url: "git@github.com:org/child.git".to_string(),
            target_path: child_dir,
            depth_level: 0,
            is_meta: false,
        };

        let added = queue.mark_completed(&task).unwrap();
        assert_eq!(added, 0);
    }

    // ── get_counts ────────────────────────────────────────────

    #[test]
    fn get_counts_tracks_discovered_and_completed() {
        let dir = tempfile::tempdir().unwrap();
        let queue = CloneQueue::new(None, None);

        queue.push(make_task("a", &dir.path().join("a")));
        queue.push(make_task("b", &dir.path().join("b")));

        let (completed, discovered) = queue.get_counts();
        assert_eq!(discovered, 2);
        assert_eq!(completed, 0);
    }

    // ── drain_all ─────────────────────────────────────────────

    #[test]
    fn drain_all_empties_queue() {
        let dir = tempfile::tempdir().unwrap();
        let queue = CloneQueue::new(None, None);
        queue.push(make_task("a", &dir.path().join("a")));
        queue.push(make_task("b", &dir.path().join("b")));

        let tasks = queue.drain_all();
        assert_eq!(tasks.len(), 2);
        assert!(queue.take_one().is_none());
    }
}
