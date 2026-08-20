//! Handing a session back to the tool that created it.

use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

use super::models::{AiSession, Source};

/// A command line plus the directory it should run in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub args: Vec<String>,
    pub cwd: String,
}

impl CommandSpec {
    /// A copy-pasteable rendering of the command.
    pub fn display(&self) -> String {
        self.args.join(" ")
    }
}

/// The command that reopens a session in its own tool.
pub fn resume_command(session: &AiSession, dir: Option<&str>) -> CommandSpec {
    let args = match session.source {
        Source::Claude => vec![
            "claude".to_string(),
            "--resume".to_string(),
            session.session_id.clone(),
        ],
        Source::Codex => vec![
            "codex".to_string(),
            "resume".to_string(),
            session.session_id.clone(),
        ],
    };

    CommandSpec {
        args,
        cwd: dir.unwrap_or(&session.project).to_string(),
    }
}

/// Replace the current process with the resume command, so the assistant owns
/// the terminal exactly as if it had been launched directly.
pub fn exec(spec: &CommandSpec) -> Result<()> {
    let (program, args) = spec
        .args
        .split_first()
        .ok_or_else(|| anyhow!("Empty resume command"))?;

    let cwd = Path::new(&spec.cwd);
    let mut command = Command::new(program);
    command.args(args);
    if cwd.is_dir() {
        command.current_dir(cwd);
    }

    let status = command.status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow!("`{}` is not installed or not on PATH", program)
        } else {
            anyhow!("Failed to run `{}`: {}", program, error)
        }
    })?;

    if !status.success() {
        return Err(anyhow!(
            "`{}` exited with {}",
            spec.display(),
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "a signal".into())
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::models::session_uid;

    fn session(source: Source) -> AiSession {
        AiSession {
            uid: session_uid(source, "abc123"),
            source,
            session_id: "abc123".into(),
            project: "/repos/thing".into(),
            title: None,
            started_at: 0,
            last_activity: 0,
            model: None,
            message_count: 0,
            file_path: "/tmp/abc123.jsonl".into(),
            file_mtime: 0,
            file_size: 0,
            custom_name: None,
        }
    }

    #[test]
    fn claude_resumes_by_session_id_in_the_project_dir() {
        let spec = resume_command(&session(Source::Claude), None);
        assert_eq!(spec.args, vec!["claude", "--resume", "abc123"]);
        assert_eq!(spec.cwd, "/repos/thing");
    }

    #[test]
    fn codex_uses_its_resume_subcommand() {
        let spec = resume_command(&session(Source::Codex), None);
        assert_eq!(spec.args, vec!["codex", "resume", "abc123"]);
    }

    #[test]
    fn an_explicit_directory_wins_over_the_project() {
        let spec = resume_command(&session(Source::Claude), Some("/elsewhere"));
        assert_eq!(spec.cwd, "/elsewhere");
    }

    #[test]
    fn display_is_copy_pasteable() {
        assert_eq!(
            resume_command(&session(Source::Codex), None).display(),
            "codex resume abc123"
        );
    }
}
