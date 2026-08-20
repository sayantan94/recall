//! `recall setup` — everything a new user needs, in one command.
//!
//! Setup indexes agent sessions and then reports what else is worth doing. It
//! never edits `~/.zshrc`: that file is the user's, often generated or version
//! controlled, and a tool writing to it behind their back is a surprise nobody
//! asked for. Anything that needs changing there is printed to copy instead.

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
/// An alias pointing at a specific binary shadows whatever is on PATH.
const ALIAS_MARKER: &str = "alias recall=";
const RULE: usize = 60;

pub fn run() -> Result<()> {
    let conn = crate::db::schema::open_db()?;

    println!();
    println!("  {} {}", "◉".cyan(), "Setting up recall".bold());
    println!("  {}", "─".repeat(RULE).dimmed());

    let latest = index_agent_sessions(&conn)?;
    println!();
    let hook = report_shell_hook();
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
    /// Installed and pointing at the binary running now.
    Current,
    /// Installed, but running some other recall binary.
    Stale,
    Missing,
    NotZsh,
}

/// A line in `~/.zshrc` that runs a specific recall binary.
#[derive(Debug, Clone)]
struct WiredLine {
    number: usize,
    text: String,
    binary: PathBuf,
    is_alias: bool,
}

/// Every uncommented line that wires up a recall binary, with the path it runs.
fn wired_lines(zshrc: &Path) -> Vec<WiredLine> {
    let contents = match std::fs::read_to_string(zshrc) {
        Ok(contents) => contents,
        Err(_) => return Vec::new(),
    };

    contents
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with('#'))
        .filter_map(|(index, line)| {
            let (binary, is_alias) = if line.contains(HOOK_MARKER) {
                (hook_binary(line)?, false)
            } else if line.contains(ALIAS_MARKER) {
                (alias_binary(line)?, true)
            } else {
                return None;
            };

            Some(WiredLine {
                number: index + 1,
                text: line.to_string(),
                binary,
                is_alias,
            })
        })
        .collect()
}

/// `eval "$(/path/to/recall init zsh)"` -> `/path/to/recall`
///
/// Split on the arguments only: `HOOK_MARKER` starts with the binary name, so
/// splitting on it would swallow the last path segment.
fn hook_binary(line: &str) -> Option<PathBuf> {
    let before = line.split(" init zsh").next()?;
    let start = before.rfind("$(").map(|i| i + 2).unwrap_or(0);
    Some(expand_home(before[start..].trim().trim_matches('"').trim_matches('\'')))
}

/// `alias recall="/path/to/recall"` -> `/path/to/recall`
fn alias_binary(line: &str) -> Option<PathBuf> {
    let value = line.split_once('=')?.1.trim();
    Some(expand_home(value.trim_matches('"').trim_matches('\'')))
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest),
        None => PathBuf::from(path),
    }
}

/// Two paths run the same program, following symlinks where possible.
fn same_binary(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Whether the directory holding the running binary is on PATH. If it is not,
/// `recall` only works by full path or through an alias.
fn on_path(binary: &Path) -> bool {
    let dir = match binary.parent() {
        Some(dir) => dir,
        None => return false,
    };
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|entry| entry == dir))
        .unwrap_or(false)
}

/// The lines to add or change in `~/.zshrc`, and how to run recall without
/// touching that file at all.
fn print_manual_steps(current: &Path, stale: &[WiredLine]) {
    let line = format!("eval \"$({} init zsh)\"", current.display());
    println!();
    println!(
        "    {}  {}",
        "To record shell commands, edit ~/.zshrc yourself:".bold(),
        "(recall never writes to it)".dimmed()
    );

    let mut step = 1;
    if stale.is_empty() {
        println!("      {}. Add this line at the end:", step);
        println!("           {}", line.cyan());
    } else {
        println!(
            "      {}. Replace line{} {}:",
            step,
            if stale.len() == 1 { "" } else { "s" },
            stale
                .iter()
                .map(|entry| entry.number.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for entry in stale {
            let replacement = if entry.is_alias {
                format!("alias recall=\"{}\"", current.display())
            } else {
                line.clone()
            };
            println!("           {}", replacement.cyan());
        }
    }
    step += 1;

    if !on_path(current) {
        println!("      {}. Put recall on your PATH, also in ~/.zshrc:", step);
        println!(
            "           {}",
            format!(
                "export PATH=\"{}:$PATH\"",
                current.parent().unwrap_or(Path::new("")).display()
            )
            .cyan()
        );
        step += 1;
    }

    println!("      {}. Reload:  {}", step, "source ~/.zshrc".cyan());
    println!();
    println!(
        "    {}",
        "Nothing above is required — everything else already works:".bold()
    );
    println!("      {}", current.display().to_string().cyan());
    println!(
        "    {}",
        "Only shell recording needs the hook; agent session search does not.".dimmed()
    );
    println!();
}

/// Report how `~/.zshrc` is wired up, and print anything worth changing.
/// Nothing here writes to the file.
fn report_shell_hook() -> HookStatus {
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
        return HookStatus::NotZsh;
    }

    let current = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("recall"));
    let zshrc = zshrc_path();
    let wired = wired_lines(&zshrc);
    let stale: Vec<WiredLine> = wired
        .iter()
        .filter(|entry| !same_binary(&entry.binary, &current))
        .cloned()
        .collect();

    if wired.is_empty() {
        println!("  {} {}", "└".dimmed(), "not set up yet".yellow());
        print_manual_steps(&current, &[]);
        return HookStatus::Missing;
    }

    if stale.is_empty() {
        println!(
            "  {} {} {}",
            "└".dimmed(),
            "✓".green(),
            "installed in ~/.zshrc and pointing here".dimmed()
        );
        return HookStatus::Current;
    }

    println!(
        "  {}   {}",
        "│".dimmed(),
        "an older install is still wired up:".yellow()
    );
    for entry in &stale {
        println!(
            "  {}     {} {}",
            "│".dimmed(),
            format!("line {}", entry.number).dimmed(),
            truncate(entry.text.trim(), 72).dimmed()
        );
    }
    println!(
        "  {} {}",
        "└".dimmed(),
        format!("they run a different binary than this one ({})", current.display()).dimmed()
    );
    print_manual_steps(&current, &stale);
    HookStatus::Stale
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

    if hook == HookStatus::Current {
        println!();
        println!(
            "  {} {}",
            "●".dimmed(),
            "Open a new terminal (or run `source ~/.zshrc`) if commands aren't being recorded."
                .dimmed()
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
    fn hook_binary_is_read_out_of_an_eval_line() {
        assert_eq!(
            hook_binary(r#"eval "$(/usr/local/bin/recall init zsh)""#),
            Some(PathBuf::from("/usr/local/bin/recall"))
        );
    }

    #[test]
    fn hook_binary_expands_a_home_relative_path() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            hook_binary(r#"eval "$(~/repos/recall/target/release/recall init zsh)""#),
            Some(home.join("repos/recall/target/release/recall"))
        );
    }

    #[test]
    fn alias_binary_is_read_out_of_an_alias_line() {
        assert_eq!(
            alias_binary(r#"alias recall="/opt/recall""#),
            Some(PathBuf::from("/opt/recall"))
        );
        assert_eq!(
            alias_binary("alias recall='/opt/recall'"),
            Some(PathBuf::from("/opt/recall"))
        );
    }

    #[test]
    fn wired_lines_finds_both_forms_and_skips_comments() {
        let path = temp_file(
            "wired",
            "export FOO=1\n\
             alias recall=\"/old/recall\"\n\
             # eval \"$(/commented/recall init zsh)\"\n\
             eval \"$(/old/recall init zsh)\"\n",
        );
        let found = wired_lines(&path);
        assert_eq!(found.len(), 2, "the commented line does not count");
        assert_eq!(found[0].number, 2);
        assert!(found[0].is_alias);
        assert_eq!(found[1].number, 4);
        assert!(!found[1].is_alias);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn setup_never_writes_to_zshrc() {
        // Setup is read-only with respect to the user's shell config. Scan the
        // module's own code — not this test block, whose literals would match
        // themselves — so a writer sneaking back in is caught.
        let source = include_str!("setup.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        for writer in ["OpenOptions", "fs::write", "append(true)"] {
            assert!(
                !code.contains(writer),
                "setup must not write files: found `{}`",
                writer
            );
        }
    }

    #[test]
    fn same_binary_ignores_path_spelling() {
        assert!(same_binary(Path::new("/bin/sh"), Path::new("/bin/sh")));
        assert!(!same_binary(Path::new("/a/recall"), Path::new("/b/recall")));
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
