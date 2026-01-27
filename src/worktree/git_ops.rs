//! Git operations for worktree management.

use anyhow::Result;
use std::path::Path;
use std::process::{Command, Stdio};

use super::types::GitStatusSummary;

pub fn git_worktree_add(
    repo_path: &Path,
    worktree_dest: &Path,
    branch: &str,
    from_ref: Option<&str>,
) -> Result<bool> {
    // If from_ref is specified, verify it exists in this repo
    if let Some(ref_name) = from_ref {
        let ref_exists = Command::new("git")
            .args(["rev-parse", "--verify", ref_name])
            .current_dir(repo_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success();

        if !ref_exists {
            anyhow::bail!(
                "Ref '{}' not found in repo '{}'",
                ref_name,
                repo_path.display()
            );
        }

        // Create branch from the specified ref
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                &worktree_dest.to_string_lossy(),
                ref_name,
            ])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "git worktree add failed for '{}' (branch: {}, ref: {}): {}",
                repo_path.display(),
                branch,
                ref_name,
                stderr.trim()
            );
        }
        return Ok(true); // Always creates a new branch from ref
    }

    // Check if branch exists locally
    let branch_exists = Command::new("git")
        .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
        .current_dir(repo_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success();

    // Also check if branch exists on remote
    let remote_branch_exists = if !branch_exists {
        Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                &format!("refs/remotes/origin/{branch}"),
            ])
            .current_dir(repo_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success()
    } else {
        false
    };

    let dest_str = worktree_dest.to_string_lossy();
    let remote_ref = format!("origin/{branch}");

    let wt_args: Vec<&str> = if branch_exists {
        // Use existing local branch
        vec!["worktree", "add", &dest_str, branch]
    } else if remote_branch_exists {
        // Create local tracking branch from remote
        vec![
            "worktree", "add", "--track", "-b", branch, &dest_str, &remote_ref,
        ]
    } else {
        // Create new branch from HEAD
        vec!["worktree", "add", "-b", branch, &dest_str]
    };

    let output = Command::new("git")
        .args(&wt_args)
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git worktree add failed for '{}' (branch: {}): {}",
            repo_path.display(),
            branch,
            stderr.trim()
        );
    }

    // Return whether we created a new branch
    let created_branch = !branch_exists && !remote_branch_exists;
    Ok(created_branch)
}

pub fn git_worktree_remove(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let wt_str = worktree_path.to_string_lossy();
    args.push(&wt_str);

    let output = Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree remove failed: {}", stderr.trim());
    }
    Ok(())
}

pub fn git_status_summary(repo_path: &Path) -> Result<GitStatusSummary> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output()?;

    let mut modified_files = Vec::new();
    let mut untracked_count = 0;

    // git status --porcelain format: "XY filename"
    // Positions 0-1: index (X) and work-tree (Y) status codes
    // Position 2: space separator
    // Position 3+: filename
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.len() < 3 {
            continue;
        }
        let status = &line[..2];
        let file = &line[3..];

        if status == "??" {
            untracked_count += 1;
        } else if !file.is_empty() {
            // Tracked file with modifications (staged, unstaged, or both).
            // For renames ("R  old -> new"), extract the new name.
            let name = file.split(" -> ").last().unwrap_or(file);
            modified_files.push(name.to_string());
        }
    }

    let dirty = !modified_files.is_empty() || untracked_count > 0;
    Ok(GitStatusSummary {
        dirty,
        modified_files,
        untracked_count,
    })
}

pub fn git_ahead_behind(repo_path: &Path) -> Result<(u32, u32)> {
    let output = Command::new("git")
        .args(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
        .current_dir(repo_path)
        .stderr(Stdio::null())
        .output()?;

    if !output.status.success() {
        // No upstream configured
        return Ok((0, 0));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = text.trim().split('\t').collect();
    if parts.len() == 2 {
        let ahead = parts[0].parse::<u32>().unwrap_or(0);
        let behind = parts[1].parse::<u32>().unwrap_or(0);
        Ok((ahead, behind))
    } else {
        Ok((0, 0))
    }
}

pub fn git_diff_stat(
    worktree_path: &Path,
    base_ref: &str,
) -> Result<(usize, usize, usize, Vec<String>)> {
    // Try three-dot diff first (changes since divergence)
    let numstat_output = Command::new("git")
        .args(["diff", "--numstat", &format!("{base_ref}...HEAD")])
        .current_dir(worktree_path)
        .stderr(Stdio::null())
        .output()?;

    let numstat_text = if numstat_output.status.success() {
        String::from_utf8_lossy(&numstat_output.stdout).to_string()
    } else {
        // Fallback to two-dot diff
        let fallback = Command::new("git")
            .args(["diff", "--numstat", &format!("{base_ref}..HEAD")])
            .current_dir(worktree_path)
            .stderr(Stdio::null())
            .output()?;
        if fallback.status.success() {
            String::from_utf8_lossy(&fallback.stdout).to_string()
        } else {
            String::new()
        }
    };

    let mut files_changed = 0;
    let mut insertions = 0;
    let mut deletions = 0;
    let mut files = Vec::new();

    for line in numstat_text.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            files_changed += 1;
            insertions += parts[0].parse::<usize>().unwrap_or(0);
            deletions += parts[1].parse::<usize>().unwrap_or(0);
            files.push(parts[2].to_string());
        }
    }

    Ok((files_changed, insertions, deletions, files))
}

/// Remove all worktree repos in correct order (children first, "." last).
/// In force mode, continues past failures and prints warnings.
/// Returns the number of repos that failed to remove (always 0 in non-force mode,
/// since non-force mode returns on first error).
pub fn remove_worktree_repos(
    repos: &[meta_cli::worktree::WorktreeRepoInfo],
    force: bool,
    verbose: bool,
) -> Result<usize> {
    let mut failures = 0;

    // Remove child repos first
    for r in repos.iter().filter(|r| r.alias != ".") {
        if verbose {
            eprintln!(
                "Removing worktree for '{}' at {}",
                r.alias,
                r.path.display()
            );
        }
        if let Err(e) = git_worktree_remove(&r.source_path, &r.path, force) {
            if force {
                failures += 1;
                log::warn!("Failed to remove worktree for '{}': {e}", r.alias);
            } else {
                return Err(e);
            }
        }
    }

    // Remove "." last (children live inside it)
    if let Some(r) = repos.iter().find(|r| r.alias == ".") {
        if verbose {
            eprintln!("Removing meta repo worktree at {}", r.path.display());
        }
        if let Err(e) = git_worktree_remove(&r.source_path, &r.path, force) {
            if force {
                failures += 1;
                log::warn!("Failed to remove meta repo worktree: {e}");
            } else {
                return Err(e);
            }
        }
    }

    Ok(failures)
}

/// Fetch a branch from origin if not locally available.
pub fn git_fetch_branch(repo_path: &Path, branch: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["fetch", "origin", branch])
        .current_dir(repo_path)
        .stderr(Stdio::piped())
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to fetch branch '{}': {}", branch, stderr.trim());
    }
    Ok(())
}
