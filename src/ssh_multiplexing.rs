//! SSH multiplexing configuration helper for optimizing parallel git operations.
//!
//! When running multiple git commands in parallel (e.g., `meta git update`),
//! SSH connections to the same host can be rate-limited. SSH multiplexing
//! allows multiple sessions to share a single TCP connection, avoiding this issue.

use console::style;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

/// Patterns that indicate SSH rate-limiting or connection issues
const SSH_ERROR_PATTERNS: &[&str] = &[
    "Connection closed by",
    "Operation timed out",
    "ssh_dispatch_run_fatal",
    "Connection reset by peer",
    "Connection refused",
];

/// Check if an error message indicates SSH rate-limiting
pub fn is_ssh_rate_limit_error(error_output: &str) -> bool {
    SSH_ERROR_PATTERNS
        .iter()
        .any(|pattern| error_output.contains(pattern))
}

/// Get the path to the SSH config file
fn ssh_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh").join("config"))
}

/// Get the path to the SSH sockets directory
fn ssh_sockets_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh").join("sockets"))
}

/// Check if SSH multiplexing is configured for github.com
pub fn is_multiplexing_configured() -> bool {
    let Some(config_path) = ssh_config_path() else {
        return false;
    };

    let Ok(content) = fs::read_to_string(&config_path) else {
        return false;
    };

    // Simple check: look for ControlMaster in the config
    // A more thorough check would parse the SSH config format
    content.contains("ControlMaster") && content.contains("ControlPath")
}

/// The configuration block to add for SSH multiplexing
fn multiplexing_config_block() -> String {
    r#"
# SSH multiplexing for faster parallel git operations
Host github.com
    ControlMaster auto
    ControlPath ~/.ssh/sockets/%r@%h-%p
    ControlPersist 600
"#
    .to_string()
}

/// Prompt user and set up SSH multiplexing
/// Returns Ok(true) if setup was completed, Ok(false) if user declined
pub fn prompt_and_setup_multiplexing() -> io::Result<bool> {
    println!();
    println!("{}", style("SSH Multiplexing Setup").bold().cyan());
    println!();
    println!("Multiple SSH connections to GitHub are being rate-limited.");
    println!("SSH multiplexing allows parallel git operations to share a single connection,");
    println!("which avoids rate limiting and speeds up operations.");
    println!();
    println!(
        "This will add the following to {}:",
        style("~/.ssh/config").yellow()
    );
    println!("{}", style(multiplexing_config_block()).dim());

    print!("Would you like to set this up now? [y/N]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if input.trim().to_lowercase() != "y" {
        println!("Setup cancelled. You can set this up manually later.");
        return Ok(false);
    }

    setup_multiplexing()?;
    Ok(true)
}

/// Set up SSH multiplexing (creates sockets dir and updates config)
pub fn setup_multiplexing() -> io::Result<()> {
    // Create sockets directory
    let Some(sockets_dir) = ssh_sockets_dir() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Could not determine home directory",
        ));
    };

    if !sockets_dir.exists() {
        fs::create_dir_all(&sockets_dir)?;
        println!("{} Created {}", style("✓").green(), sockets_dir.display());
    }

    // Update SSH config
    let Some(config_path) = ssh_config_path() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Could not determine home directory",
        ));
    };

    // Read existing config or start fresh
    let existing_config = fs::read_to_string(&config_path).unwrap_or_default();

    // Check if github.com host block already exists
    if existing_config.contains("Host github.com") {
        println!(
            "{} Found existing 'Host github.com' in SSH config.",
            style("!").yellow()
        );
        println!("  Please manually add ControlMaster settings to your existing config.");
        println!("  Add these lines under your 'Host github.com' block:");
        println!("{}", style("    ControlMaster auto").dim());
        println!("{}", style("    ControlPath ~/.ssh/sockets/%r@%h-%p").dim());
        println!("{}", style("    ControlPersist 600").dim());
        return Ok(());
    }

    // Ensure .ssh directory exists
    if let Some(ssh_dir) = config_path.parent() {
        if !ssh_dir.exists() {
            fs::create_dir_all(ssh_dir)?;
        }
    }

    // Append our config block
    let new_config = if existing_config.is_empty() {
        multiplexing_config_block()
    } else {
        format!(
            "{}\n{}",
            existing_config.trim_end(),
            multiplexing_config_block()
        )
    };

    fs::write(&config_path, new_config)?;
    println!("{} Updated {}", style("✓").green(), config_path.display());

    println!();
    println!(
        "{} SSH multiplexing is now configured!",
        style("✓").green().bold()
    );
    println!("  Parallel git operations will now share a single SSH connection.");

    Ok(())
}

/// Print a hint about SSH multiplexing (for use after detecting rate-limit errors)
pub fn print_multiplexing_hint() {
    println!();
    println!("{}", style("Hint:").yellow().bold());
    println!("  Some SSH connections failed, possibly due to rate limiting.");
    println!(
        "  Run {} to set up SSH multiplexing,",
        style("meta git setup-ssh").cyan()
    );
    println!("  which allows parallel operations to share a single connection.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ssh_rate_limit_error() {
        // Test all known SSH error patterns
        assert!(is_ssh_rate_limit_error(
            "Connection closed by 140.82.113.4 port 22"
        ));
        assert!(is_ssh_rate_limit_error(
            "ssh: connect to host github.com port 22: Operation timed out"
        ));
        assert!(is_ssh_rate_limit_error(
            "ssh_dispatch_run_fatal: Connection to 140.82.114.3 port 22"
        ));
        assert!(is_ssh_rate_limit_error("Connection reset by peer"));
        assert!(is_ssh_rate_limit_error("Connection refused"));

        // Test non-matching cases
        assert!(!is_ssh_rate_limit_error("Already up to date."));
        assert!(!is_ssh_rate_limit_error("fatal: not a git repository"));
        assert!(!is_ssh_rate_limit_error(
            "error: pathspec 'foo' did not match any file(s)"
        ));
        assert!(!is_ssh_rate_limit_error(""));
    }

    #[test]
    fn test_ssh_config_path() {
        // ssh_config_path should return Some path when HOME is set
        let path = ssh_config_path();
        // This test just verifies the function doesn't panic
        // The actual path depends on the HOME environment variable
        if std::env::var("HOME").is_ok() {
            assert!(path.is_some());
            let path = path.unwrap();
            assert!(path.ends_with("config"));
            assert!(path.to_str().unwrap().contains(".ssh"));
        }
    }

    #[test]
    fn test_ssh_sockets_dir() {
        // ssh_sockets_dir should return Some path when HOME is set
        let path = ssh_sockets_dir();
        if std::env::var("HOME").is_ok() {
            assert!(path.is_some());
            let path = path.unwrap();
            assert!(path.ends_with("sockets"));
            assert!(path.to_str().unwrap().contains(".ssh"));
        }
    }

    #[test]
    fn test_multiplexing_config_block() {
        let block = multiplexing_config_block();
        assert!(block.contains("Host github.com"));
        assert!(block.contains("ControlMaster auto"));
        assert!(block.contains("ControlPath"));
        assert!(block.contains("ControlPersist"));
    }

    #[test]
    fn test_is_ssh_rate_limit_error_case_sensitivity() {
        // Should match exact case
        assert!(is_ssh_rate_limit_error("Connection closed by"));
        // Should not match different case (patterns are case-sensitive)
        assert!(!is_ssh_rate_limit_error("connection closed by"));
    }
}
