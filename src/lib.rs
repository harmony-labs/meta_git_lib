use std::path::Path;
use std::process::{Command, Stdio};
use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use console::style;

/// Clone a git repository into the target directory, with progress bar.
pub fn clone_repo_with_progress(url: &str, target_dir: &Path, pb: Option<&ProgressBar>) -> Result<()> {
    if target_dir.exists() {
        if let Some(pb) = pb {
            pb.finish_with_message(format!("{}: already exists, skipping", target_dir.display()));
        } else {
            println!("{}: already exists, skipping", target_dir.display());
        }
        return Ok(())
    }
    if let Some(pb) = pb {
        pb.set_message(format!("Cloning {}", url));
    } else {
        println!("Cloning {} into {}", url, target_dir.display());
    }
    let mut child = Command::new("git")
        .arg("clone")
        .arg(url)
        .arg(target_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let status = child.wait()?;
    if let Some(pb) = pb {
        if status.success() {
            pb.finish_with_message(format!("{} ✓", style(target_dir.display()).green()));
        } else {
            pb.finish_with_message(format!("Failed to clone {} into {}", url, target_dir.display()));
        }
    } else {
        if status.success() {
            println!("{} ✓", style(target_dir.display()).green());
        } else {
            println!("Failed to clone {} into {}", url, target_dir.display());
        }
    }
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Failed to clone {} into {}", url, target_dir.display())
    }
}
