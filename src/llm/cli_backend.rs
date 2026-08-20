//! Use an AI coding CLI that's already installed and logged in, instead of an
//! API key. If `claude` or `codex` is on PATH, recall shells out to it in
//! headless mode — the user's existing subscription does the work.

use anyhow::{anyhow, Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a headless invocation may run before recall gives up.
const TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Claude,
    Codex,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }

    fn parse(name: &str) -> Option<Kind> {
        match name.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(Kind::Claude),
            "codex" => Some(Kind::Codex),
            _ => None,
        }
    }
}

/// An installed CLI recall can drive.
#[derive(Debug, Clone)]
pub struct CliBackend {
    pub kind: Kind,
    pub binary: PathBuf,
    /// Model override; None lets the tool pick its own default.
    pub model: Option<String>,
}

impl CliBackend {
    pub fn label(&self) -> String {
        match &self.model {
            Some(model) => format!("{} ({})", self.kind.as_str(), model),
            None => self.kind.as_str().to_string(),
        }
    }
}

/// Find an installed CLI. `preferred` names one explicitly; otherwise Claude
/// Code wins when both are present, since its headless mode is the cheapest.
pub fn detect(preferred: Option<&str>, model: Option<&str>) -> Option<CliBackend> {
    let candidates: Vec<Kind> = match preferred.and_then(Kind::parse) {
        Some(kind) => vec![kind],
        None => PREFERENCE.to_vec(),
    };

    for kind in candidates {
        if let Some(binary) = find_in_path(kind.as_str()) {
            return Some(CliBackend {
                kind,
                binary,
                model: model.filter(|m| !m.is_empty()).map(str::to_string),
            });
        }
    }

    None
}

/// The order recall tries them in: Claude Code first, then Codex.
pub const PREFERENCE: [Kind; 2] = [Kind::Claude, Kind::Codex];

/// Every CLI backend recall can see, for status output.
pub fn detect_all() -> Vec<Kind> {
    PREFERENCE
        .into_iter()
        .filter(|kind| find_in_path(kind.as_str()).is_some())
        .collect()
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Run the prompt through the backend and return its answer.
///
/// The prompt goes in over stdin rather than argv: conversation context can run
/// to tens of kilobytes, which argv limits would truncate.
pub fn invoke(backend: &CliBackend, prompt: &str) -> Result<String> {
    match backend.kind {
        Kind::Claude => invoke_claude(backend, prompt),
        Kind::Codex => invoke_codex(backend, prompt),
    }
}

fn invoke_claude(backend: &CliBackend, prompt: &str) -> Result<String> {
    let mut command = Command::new(&backend.binary);
    // `--no-session-persistence` keeps recall's own questions out of the user's
    // Claude Code history, where they would otherwise show up as real sessions.
    command.arg("-p").arg("--no-session-persistence");
    if let Some(model) = &backend.model {
        command.arg("--model").arg(model);
    }

    let output = run(command, prompt, "claude")?;
    Ok(output.trim().to_string())
}

fn invoke_codex(backend: &CliBackend, prompt: &str) -> Result<String> {
    // `codex exec` streams progress to stdout, so the answer is collected from
    // the last-message file instead of the stream.
    let answer_file = std::env::temp_dir().join(format!(
        "recall-codex-{}-{}.txt",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));

    let mut command = Command::new(&backend.binary);
    command
        .arg("exec")
        // `--ephemeral` is Codex's equivalent: no session file is written.
        .arg("--ephemeral")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--skip-git-repo-check")
        .arg("--color")
        .arg("never")
        .arg("-o")
        .arg(&answer_file);
    if let Some(model) = &backend.model {
        command.arg("--model").arg(model);
    }

    let stdout = run(command, prompt, "codex")?;

    let answer = std::fs::read_to_string(&answer_file).unwrap_or(stdout);
    let _ = std::fs::remove_file(&answer_file);
    Ok(answer.trim().to_string())
}

/// Spawn the command, feed it the prompt, and wait with a timeout.
fn run(mut command: Command, prompt: &str, name: &str) -> Result<String> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to start `{}`", name))?;

    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Failed to open stdin for `{}`", name))?
        .write_all(prompt.as_bytes())
        .with_context(|| format!("Failed to send the prompt to `{}`", name))?;

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait()? {
            Some(_) => break,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow!(
                    "`{}` did not answer within {}s",
                    name,
                    TIMEOUT.as_secs()
                ));
            }
        }
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "`{}` failed: {}",
            name,
            stderr.trim().lines().last().unwrap_or("no output")
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parsing_accepts_known_names() {
        assert_eq!(Kind::parse("claude"), Some(Kind::Claude));
        assert_eq!(Kind::parse("Claude-Code"), Some(Kind::Claude));
        assert_eq!(Kind::parse("codex"), Some(Kind::Codex));
        assert_eq!(Kind::parse("gemini"), None);
    }

    #[test]
    fn label_mentions_the_model_only_when_overridden() {
        let mut backend = CliBackend {
            kind: Kind::Claude,
            binary: PathBuf::from("/usr/bin/claude"),
            model: None,
        };
        assert_eq!(backend.label(), "claude");
        backend.model = Some("haiku".into());
        assert_eq!(backend.label(), "claude (haiku)");
    }

    #[test]
    fn detect_falls_back_when_the_preference_is_unknown() {
        // A typo in config shouldn't disable the feature; auto-detection resumes.
        assert_eq!(
            detect(Some("gemini"), None).map(|b| b.kind),
            detect(None, None).map(|b| b.kind)
        );
    }

    #[test]
    fn find_in_path_locates_a_real_binary() {
        assert!(find_in_path("sh").is_some());
        assert!(find_in_path("definitely-not-a-real-binary-xyz").is_none());
    }
}
