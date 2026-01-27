//! Utility functions for worktree management.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::types::WorktreeContext;

pub fn validate_worktree_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Worktree name cannot be empty");
    }
    if name.starts_with('.') {
        anyhow::bail!("Invalid worktree name '{name}': cannot start with '.'");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("Invalid worktree name '{name}': cannot contain path separators");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("Invalid worktree name '{name}': only ASCII alphanumeric characters, hyphens, and underscores allowed");
    }
    Ok(())
}

pub fn resolve_worktree_root(meta_dir: Option<&Path>) -> Result<PathBuf> {
    // 1. Check META_WORKTREES env var
    if let Ok(env_path) = std::env::var("META_WORKTREES") {
        return Ok(PathBuf::from(env_path));
    }
    // 2. Check worktrees_dir in .meta config
    if let Some(dir) = meta_dir {
        if let Some(configured) = read_worktrees_dir_from_config(dir) {
            return Ok(dir.join(configured));
        }
        // 3. Default: .worktrees/ relative to meta root
        return Ok(dir.join(".worktrees"));
    }
    // Fallback if no meta dir found
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(".worktrees"))
}

/// Read and parse the .meta config as a JSON Value.
/// Tries .meta, .meta.yaml, .meta.yml in order, parsing JSON or YAML as appropriate.
pub fn read_meta_config_value(meta_dir: &Path) -> Option<serde_json::Value> {
    for name in &[".meta", ".meta.yaml", ".meta.yml"] {
        let path = meta_dir.join(name);
        if !path.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        // Try JSON first
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
            return Some(v);
        }
        // Try YAML
        if let Ok(v) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            // Convert YAML Value to JSON Value for uniform access
            if let Ok(json_val) = serde_json::to_value(v) {
                return Some(json_val);
            }
        }
    }
    None
}

pub fn read_worktrees_dir_from_config(meta_dir: &Path) -> Option<String> {
    read_meta_config_value(meta_dir)?
        .get("worktrees_dir")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn find_meta_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    meta_cli::config::find_meta_config(&cwd, None)
        .map(|(path, _)| path.parent().unwrap_or(Path::new(".")).to_path_buf())
}

/// Like `find_meta_dir()` but returns an error if not found.
pub fn require_meta_dir() -> Result<PathBuf> {
    find_meta_dir()
        .ok_or_else(|| anyhow::anyhow!("Not inside a meta project (no .meta found)"))
}

/// Resolve worktree context for a named worktree.
/// Returns meta_dir (for hooks), worktree_root, and the specific worktree directory.
pub fn resolve_worktree_context(name: &str) -> Result<WorktreeContext> {
    let meta_dir = find_meta_dir();
    let worktree_root = resolve_worktree_root(meta_dir.as_deref())?;
    let wt_dir = worktree_root.join(name);
    Ok(WorktreeContext {
        meta_dir,
        worktree_root,
        wt_dir,
    })
}

/// Resolve worktree context and verify the worktree exists.
pub fn resolve_existing_worktree(name: &str) -> Result<WorktreeContext> {
    let ctx = resolve_worktree_context(name)?;
    if !ctx.wt_dir.exists() {
        anyhow::bail!("Worktree '{}' not found at {}", name, ctx.wt_dir.display());
    }
    Ok(ctx)
}

/// Resolve, validate existence, and discover repos in a named worktree.
/// Returns a non-empty repo list, or errors if worktree doesn't exist
/// or contains no repos.
pub fn discover_and_validate_worktree(
    name: &str,
) -> Result<Vec<meta_cli::worktree::WorktreeRepoInfo>> {
    let ctx = resolve_existing_worktree(name)?;
    let repos = meta_cli::worktree::discover_worktree_repos(&ctx.wt_dir)?;
    if repos.is_empty() {
        anyhow::bail!("No repos found in worktree '{name}'");
    }
    Ok(repos)
}

/// Load and parse the .meta config, returning the project list.
pub fn load_projects(meta_dir: &Path) -> Result<Vec<meta_cli::config::ProjectInfo>> {
    let (config_path, _) = meta_cli::config::find_meta_config(meta_dir, None)
        .ok_or_else(|| anyhow::anyhow!("No .meta config found in {}", meta_dir.display()))?;
    let (projects, _) = meta_cli::config::parse_meta_config(&config_path)?;
    Ok(projects)
}

/// Look up a project by alias, returning an error with valid aliases on miss.
pub fn lookup_project<'a>(
    projects: &'a [meta_cli::config::ProjectInfo],
    alias: &str,
) -> Result<&'a meta_cli::config::ProjectInfo> {
    projects.iter().find(|p| p.name == alias).ok_or_else(|| {
        let valid: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        anyhow::anyhow!(
            "Unknown repo alias: '{}'. Valid aliases: {}",
            alias,
            valid.join(", ")
        )
    })
}

pub fn resolve_branch(
    task_name: &str,
    branch_flag: Option<&str>,
    per_repo_branch: Option<&str>,
) -> String {
    per_repo_branch
        .or(branch_flag)
        .map(|s| s.to_string())
        .unwrap_or_else(|| task_name.to_string())
}

/// Parse a human-friendly duration string to seconds.
/// Supported formats: "30s", "5m", "1h", "2d", "1w", or bare seconds "300"
pub fn parse_duration(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("Empty duration string");
    }

    // Bare number: treat as seconds
    if s.chars().all(|c| c.is_ascii_digit()) {
        return s
            .parse::<u64>()
            .with_context(|| format!("Invalid duration: '{s}'"));
    }

    let (num_str, suffix) = s.split_at(s.len() - 1);
    let num: u64 = num_str
        .parse()
        .with_context(|| format!("Invalid duration number: '{num_str}'"))?;

    let multiplier = match suffix {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 604800,
        _ => anyhow::bail!(
            "Invalid duration suffix '{suffix}'. Use s (seconds), m (minutes), h (hours), d (days), or w (weeks)"
        ),
    };

    Ok(num * multiplier)
}

/// Format seconds into a human-friendly duration string.
/// Returns the largest appropriate unit (e.g., "2h" not "7200s").
pub fn format_duration(secs: i64) -> String {
    let abs_secs = secs.unsigned_abs();
    let prefix = if secs < 0 { "-" } else { "" };

    if abs_secs >= 604800 && abs_secs.is_multiple_of(604800) {
        let weeks = abs_secs / 604800;
        format!("{prefix}{weeks}w")
    } else if abs_secs >= 86400 && abs_secs.is_multiple_of(86400) {
        let days = abs_secs / 86400;
        format!("{prefix}{days}d")
    } else if abs_secs >= 3600 && abs_secs.is_multiple_of(3600) {
        let hours = abs_secs / 3600;
        format!("{prefix}{hours}h")
    } else if abs_secs >= 60 && abs_secs.is_multiple_of(60) {
        let mins = abs_secs / 60;
        format!("{prefix}{mins}m")
    } else {
        format!("{prefix}{abs_secs}s")
    }
}

/// Parse `--from-pr owner/repo#N` format and resolve the PR's head branch.
/// Returns (owner/repo, pr_number, head_branch_name).
pub fn resolve_from_pr(from_pr: &str) -> Result<(String, u32, String)> {
    use std::process::Command;

    // Parse format: owner/repo#N
    let hash_pos = from_pr.rfind('#').ok_or_else(|| {
        anyhow::anyhow!("Invalid --from-pr format: '{from_pr}'. Expected: owner/repo#N")
    })?;

    let repo_spec = &from_pr[..hash_pos];
    // Validate repo spec format: must be owner/repo
    if !repo_spec.contains('/') || repo_spec.starts_with('/') || repo_spec.ends_with('/') {
        anyhow::bail!("Invalid repo spec '{repo_spec}' in --from-pr. Expected: owner/repo#N");
    }
    let pr_num: u32 = from_pr[hash_pos + 1..]
        .parse()
        .with_context(|| format!("Invalid PR number in '{from_pr}'"))?;

    // Resolve head branch via gh CLI
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo_spec,
            "--json",
            "headRefName",
            "-q",
            ".headRefName",
        ])
        .output()
        .with_context(|| "Failed to run 'gh' CLI. Is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Failed to resolve PR #{} in {}: {}",
            pr_num,
            repo_spec,
            stderr.trim()
        );
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        anyhow::bail!("Empty head branch for PR #{pr_num} in {repo_spec}");
    }

    Ok((repo_spec.to_string(), pr_num, branch))
}

/// Check if a repo's remote URL matches the given owner/repo spec.
pub fn repo_matches_spec(repo_path: &Path, spec: &str) -> bool {
    use std::process::Command;

    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // Match against github.com:owner/repo or github.com/owner/repo
            url.contains(spec) || url.contains(&spec.replace('/', ":"))
        }
        _ => false,
    }
}

pub fn ensure_worktrees_in_gitignore(
    meta_dir: &Path,
    worktrees_dirname: &str,
    quiet: bool,
) -> Result<()> {
    let gitignore_path = meta_dir.join(".gitignore");
    let pattern = format!("{worktrees_dirname}/");

    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if content.lines().any(|line| {
            line.trim() == pattern.trim_end_matches('/') || line.trim() == pattern
        }) {
            return Ok(()); // already present
        }
        // Append
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&gitignore_path)?;
        writeln!(file, "{pattern}")?;
    } else {
        std::fs::write(&gitignore_path, format!("{pattern}\n"))?;
    }
    if !quiet {
        log::info!("Added '{pattern}' to .gitignore");
    }
    Ok(())
}
