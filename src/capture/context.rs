use std::path::Path;
use std::process::Command;

/// Detect the git repository root from a given working directory.
pub fn detect_git_repo(cwd: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if output.status.success() {
        let repo = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Return just the directory name, not the full path
        Path::new(&repo)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
    } else {
        None
    }
}

/// Detect the current git branch from a given working directory.
pub fn detect_git_branch(cwd: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            None
        } else {
            Some(branch)
        }
    } else {
        None
    }
}
