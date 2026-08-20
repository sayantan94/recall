//! `recall setup` — everything a new user needs, in one command.
//!
//! Order matters: agent-session indexing runs first because it is the only
//! step with instant retroactive payoff. Asking to edit someone's `~/.zshrc`
//! is the last thing recall does, after it has already proved useful.

use anyhow::{Context, Result};
use colored::Colorize;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::ai::indexer;
use crate::ai::models::AiSession;
use crate::ai::store::{self, Filter};
use crate::llm::cli_backend;

/// Marker recall looks for when deciding whether the hook is already installed.
const HOOK_MARKER: &str = "recall init zsh";
const RULE: usize = 60;

pub fn run(assume_yes: bool) -> Result<()> {
    let conn = crate::db::schema::open_db()?;

    println!();
    println!("  {} {}", "◉".cyan(), "Setting up recall".bold());
    println!("  {}", "─".repeat(RULE).dimmed());

    let latest = index_agent_sessions(&conn)?;
    println!();
    let hook = install_shell_hook(assume_yes)?;
    println!();
    let backend = report_ask_backend();

    println!("  {}", "─".repeat(RULE).dimmed());
    print_next_steps(latest.as_ref(), hook, backend.is_some());
    Ok(())
}

/// Index Claude Code and Codex transcripts, and report the most recent session
/// back to the user — proof the index is real and personal.
fn index_agent_sessions(conn: &rusqlite::Connection) -> Result<Option<AiSession>> {
    println!(
        "  {} {} {}",
        "┌".dimmed(),
        section("Agent sessions"),
        "reading transcripts Claude Code & Codex already keep on disk".dimmed()
    );

    let started = Instant::now();
    let report = indexer::index_all(conn, false)?;
    let elapsed = started.elapsed();
    let stats = store::stats(conn)?;

    if stats.sessions == 0 {
        println!(
            "  {} {}",
            "└".dimmed(),
            "no Claude Code or Codex transcripts found — this lights up as soon as you use either"
                .dimmed()
        );
        return Ok(None);
    }

    for (source, count) in &stats.per_source {
        if *count > 0 {
            println!(
                "  {}   {:<9} {}",
                "│".dimmed(),
                source.as_str().magenta(),
                format!("{} sessions", count).dimmed()
            );
        }
    }
    println!(
        "  {} {} {}",
        "└".dimmed(),
        "✓".green(),
        format!(
            "{} conversations indexed across {} projects in {:.1}s — searchable right now",
            stats.sessions,
            stats.projects,
            elapsed.as_secs_f32()
        )
        .dimmed()
    );

    if !report.failed.is_empty() {
        println!(
            "  {}   {}",
            " ".dimmed(),
            format!("{} transcripts could not be parsed", report.failed.len()).yellow()
        );
    }

    Ok(store::list_sessions(conn, &Filter::with_limit(1))?
        .into_iter()
        .next())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStatus {
    AlreadyInstalled,
    Installed,
    Declined,
    NotZsh,
}

fn install_shell_hook(assume_yes: bool) -> Result<HookStatus> {
    println!(
        "  {} {} {}",
        "┌".dimmed(),
        section("Shell recording"),
        "captures every command you run from now on".dimmed()
    );

    let shell = std::env::var("SHELL").unwrap_or_default();
    if !shell.ends_with("zsh") {
        println!(
            "  {} {} {}",
            "└".dimmed(),
            "skipped".yellow(),
            format!("recall records zsh only; your shell is {}", shell).dimmed()
        );
        return Ok(HookStatus::NotZsh);
    }

    let zshrc = zshrc_path();
    if hook_present(&zshrc) {
        println!(
            "  {} {} {}",
            "└".dimmed(),
            "✓".green(),
            "already installed in ~/.zshrc".dimmed()
        );
        return Ok(HookStatus::AlreadyInstalled);
    }

    let line = hook_line()?;
    println!(
        "  {}   {}",
        "│".dimmed(),
        "recall never edits ~/.zshrc without asking.".dimmed()
    );
    println!("  {}   {}", "│".dimmed(), line.cyan());
    println!(
        "  {}   {}",
        "│".dimmed(),
        "This also wraps new shells in `script` so command output is captured.".dimmed()
    );

    // Never prompt when nobody is watching: `recall setup` stays safe in scripts.
    if !assume_yes && !std::io::stdin().is_terminal() {
        println!(
            "  {} {}",
            "└".dimmed(),
            "not installed — add the line above to ~/.zshrc, or re-run `recall setup --yes`"
                .dimmed()
        );
        return Ok(HookStatus::Declined);
    }

    if !assume_yes && !confirm("  │   Append it to ~/.zshrc?")? {
        println!(
            "  {} {}",
            "└".dimmed(),
            "left unchanged — copy the line above whenever you want it".dimmed()
        );
        return Ok(HookStatus::Declined);
    }

    append_hook(&zshrc, &line)?;
    println!(
        "  {} {} {}",
        "└".dimmed(),
        "✓".green(),
        "added — every new terminal starts recording automatically".dimmed()
    );
    Ok(HookStatus::Installed)
}

fn report_ask_backend() -> Option<String> {
    println!(
        "  {} {} {}",
        "┌".dimmed(),
        section("Ask engine"),
        "lets you ask `recall \"what broke yesterday\"`".dimmed()
    );

    let installed = cli_backend::detect_all();
    if let Some(kind) = installed.first() {
        let names: Vec<&str> = installed.iter().map(|k| k.as_str()).collect();
        println!(
            "  {} {} {}",
            "└".dimmed(),
            "✓".green(),
            format!("{} found on PATH — no API key needed", names.join(" and ")).dimmed()
        );
        return Some(kind.as_str().to_string());
    }

    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        println!(
            "  {} {} {}",
            "└".dimmed(),
            "✓".green(),
            "using ANTHROPIC_API_KEY".dimmed()
        );
        return Some("anthropic".to_string());
    }

    println!(
        "  {} {} {}",
        "└".dimmed(),
        "optional".yellow(),
        "install Claude Code or Codex, or set ANTHROPIC_API_KEY in ~/.recall/env".dimmed()
    );
    None
}

fn print_next_steps(latest: Option<&AiSession>, hook: HookStatus, has_backend: bool) {
    println!("  {}", "Try it now:".bold());
    println!();

    match latest {
        Some(session) => {
            step("recall agents resume", "reopen your latest session:");
            println!(
                "    {}{}  {}",
                " ".repeat(STEP_WIDTH + 2),
                truncate(
                    &session
                        .title
                        .as_deref()
                        .unwrap_or("(untitled)")
                        .replace('\n', " "),
                    44
                )
                .cyan(),
                format!("{} · {}", session.source.as_str(), ago(session.last_activity)).dimmed()
            );
            step(
                "recall agents search \"...\"",
                "search every conversation you've had",
            );
        }
        None => step("recall today", "today's commands (fills up from here)"),
    }

    step("recall", "browse everything in the TUI — `a` for agent sessions");

    if has_backend {
        step(
            "recall \"what broke yesterday\"",
            "ask your history in plain English",
        );
    }

    if hook == HookStatus::Installed {
        println!();
        println!(
            "  {} {}",
            "●".dimmed(),
            "Open a new terminal (or run `source ~/.zshrc`) to start recording commands.".dimmed()
        );
    }

    println!();
}

/// Width of the command column in the "Try it now" block.
const STEP_WIDTH: usize = 30;

/// Padding has to happen before colouring: ANSI escapes count toward a format
/// width specifier and would throw the column off.
fn step(command: &str, description: &str) {
    println!(
        "    {}  {}",
        format!("{:<width$}", command, width = STEP_WIDTH)
            .white()
            .bold(),
        description.dimmed()
    );
}

fn section(label: &str) -> colored::ColoredString {
    format!("{:<16}", label).white().bold()
}

fn zshrc_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zshrc")
}

/// The eval line, with the absolute path of the running binary baked in, so it
/// keeps working whether recall was installed globally or built in a repo.
fn hook_line() -> Result<String> {
    let exe = std::env::current_exe().context("Could not determine the recall binary path")?;
    Ok(format!("eval \"$({} init zsh)\"", exe.display()))
}

fn hook_present(zshrc: &Path) -> bool {
    let file = match std::fs::File::open(zshrc) {
        Ok(file) => file,
        Err(_) => return false,
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains(HOOK_MARKER) && !line.trim_start().starts_with('#'))
}

fn append_hook(zshrc: &Path, line: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(zshrc)
        .with_context(|| format!("Failed to open {}", zshrc.display()))?;
    writeln!(file, "\n# recall — records shell history\n{}", line)
        .with_context(|| format!("Failed to write to {}", zshrc.display()))?;
    Ok(())
}

fn confirm(question: &str) -> Result<bool> {
    print!("{} {} ", question.dimmed(), "[y/N]".cyan());
    std::io::stdout().flush().ok();

    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Rough relative time, e.g. "2h ago".
fn ago(millis: i64) -> String {
    let now = chrono::Utc::now().timestamp_millis();
    let minutes = (now - millis).max(0) / 60_000;

    if minutes < 60 {
        format!("{}m ago", minutes.max(1))
    } else if minutes < 60 * 24 {
        format!("{}h ago", minutes / 60)
    } else {
        format!("{}d ago", minutes / (60 * 24))
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("recall-{}-{}", name, std::process::id()));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn hook_is_detected_when_present() {
        let path = temp_file("present", "export FOO=1\neval \"$(/bin/recall init zsh)\"\n");
        assert!(hook_present(&path));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_commented_out_hook_does_not_count() {
        let path = temp_file("commented", "# eval \"$(/bin/recall init zsh)\"\n");
        assert!(!hook_present(&path));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_zshrc_reports_no_hook() {
        assert!(!hook_present(Path::new("/nonexistent/.zshrc")));
    }

    #[test]
    fn appending_makes_the_hook_detectable() {
        let path = temp_file("append", "export FOO=1\n");
        assert!(!hook_present(&path));
        append_hook(&path, "eval \"$(/bin/recall init zsh)\"").unwrap();
        assert!(hook_present(&path));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn hook_line_embeds_an_absolute_path() {
        let line = hook_line().unwrap();
        assert!(line.starts_with("eval \"$(/"));
        assert!(line.ends_with(" init zsh)\""));
    }

    #[test]
    fn ago_scales_from_minutes_to_days() {
        let now = chrono::Utc::now().timestamp_millis();
        assert_eq!(ago(now), "1m ago");
        assert_eq!(ago(now - 90 * 60_000), "1h ago");
        assert_eq!(ago(now - 3 * 24 * 60 * 60_000), "3d ago");
    }
}
