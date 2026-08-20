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
/// An alias pointing at a specific binary shadows whatever is on PATH.
const ALIAS_MARKER: &str = "alias recall=";
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

/// Rewrite stale lines to run `current`, leaving every other line untouched.
/// A backup is written first: this edits a file recall does not own.
fn repoint_lines(zshrc: &Path, stale: &[WiredLine], current: &Path) -> Result<usize> {
    let contents = std::fs::read_to_string(zshrc)
        .with_context(|| format!("Failed to read {}", zshrc.display()))?;
    let backup = zshrc.with_extension("recall-backup");
    std::fs::write(&backup, &contents)
        .with_context(|| format!("Failed to write {}", backup.display()))?;

    let display = current.display().to_string();
    let mut updated = 0;
    let rewritten: Vec<String> = contents
        .lines()
        .enumerate()
        .map(|(index, line)| {
            match stale.iter().find(|entry| entry.number == index + 1) {
                Some(entry) if entry.is_alias => {
                    updated += 1;
                    format!("alias recall=\"{}\"", display)
                }
                Some(_) => {
                    updated += 1;
                    format!("eval \"$({} init zsh)\"", display)
                }
                None => line.to_string(),
            }
        })
        .collect();

    let mut out = rewritten.join("\n");
    if contents.ends_with('\n') {
        out.push('\n');
    }
    std::fs::write(zshrc, out)
        .with_context(|| format!("Failed to write {}", zshrc.display()))?;

    Ok(updated)
}

/// Point any older install still wired up in `~/.zshrc` at the binary running
/// now, so an upgrade cannot leave a previous version shadowing it.
fn clear_stale_install(assume_yes: bool) -> Result<()> {
    let current = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => return Ok(()),
    };

    let zshrc = zshrc_path();
    let stale: Vec<WiredLine> = wired_lines(&zshrc)
        .into_iter()
        .filter(|entry| !same_binary(&entry.binary, &current))
        .collect();

    if stale.is_empty() {
        println!(
            "  {} {} {}",
            "└".dimmed(),
            "✓".green(),
            "no older install left behind".dimmed()
        );
        return Ok(());
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
        "  {}   {}",
        "│".dimmed(),
        format!("they run a different binary than this one ({})", current.display()).dimmed()
    );


    if (!assume_yes && !std::io::stdin().is_terminal())
        || (!assume_yes && !confirm("  │   Point them at this binary?")?)
    {
        println!("  {} {}", "└".dimmed(), "left unchanged".dimmed());
        print_manual_steps(&current, &stale);
        return Ok(());
    }

    let updated = repoint_lines(&zshrc, &stale, &current)?;
    println!(
        "  {} {} {}",
        "└".dimmed(),
        "✓".green(),
        format!(
            "repointed {} line{} (previous ~/.zshrc saved as ~/.zshrc.recall-backup)",
            updated,
            if updated == 1 { "" } else { "s" }
        )
        .dimmed()
    );
    Ok(())
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

/// What to do by hand when recall does not edit `~/.zshrc` — and how to run it
/// in the meantime.
fn print_manual_steps(current: &Path, stale: &[WiredLine]) {
    let line = format!("eval \"$({} init zsh)\"", current.display());
    println!();
    println!("    {}", "To wire it up yourself:".bold());

    let mut step = 1;
    if stale.is_empty() {
        println!("      {}. Add this line to {}:", step, "~/.zshrc".cyan());
        println!("           {}", line.cyan());
    } else {
        println!(
            "      {}. Replace {} in {}:",
            step,
            format!(
                "line{} {}",
                if stale.len() == 1 { "" } else { "s" },
                stale
                    .iter()
                    .map(|entry| entry.number.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "~/.zshrc".cyan()
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
        println!(
            "      {}. Put recall on your PATH by adding to {}:",
            step, "~/.zshrc".cyan()
        );
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

    println!("      {}. Reload your shell:  {}", step, "source ~/.zshrc".cyan());
    println!();
    println!("    {}", "Until then, recall still works — start it with:".bold());
    println!("      {}", current.display().to_string().cyan());
    println!(
        "    {}",
        "Only shell recording needs the hook; agent session search works without it.".dimmed()
    );
    println!();
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
            "  {}   {} {}",
            "│".dimmed(),
            "✓".green(),
            "hook installed in ~/.zshrc".dimmed()
        );
        clear_stale_install(assume_yes)?;
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

    let current = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("recall"));

    // Never prompt when nobody is watching: `recall setup` stays safe in scripts.
    if !assume_yes && !std::io::stdin().is_terminal() {
        println!("  {} {}", "└".dimmed(), "not installed".yellow());
        print_manual_steps(&current, &[]);
        return Ok(HookStatus::Declined);
    }

    if !assume_yes && !confirm("  │   Append it to ~/.zshrc?")? {
        println!("  {} {}", "└".dimmed(), "left unchanged".dimmed());
        print_manual_steps(&current, &[]);
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
    fn repointing_rewrites_only_the_stale_lines() {
        let path = temp_file(
            "repoint",
            "export FOO=1\n\
             alias recall=\"/old/recall\"\n\
             export BAR=2\n\
             eval \"$(/old/recall init zsh)\"\n",
        );
        let stale = wired_lines(&path);
        let updated = repoint_lines(&path, &stale, Path::new("/new/recall")).unwrap();
        assert_eq!(updated, 2);

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains(r#"alias recall="/new/recall""#));
        assert!(after.contains(r#"eval "$(/new/recall init zsh)""#));
        assert!(!after.contains("/old/recall"), "no old path survives");
        assert!(after.contains("export FOO=1") && after.contains("export BAR=2"));

        let backup = path.with_extension("recall-backup");
        assert!(
            std::fs::read_to_string(&backup).unwrap().contains("/old/recall"),
            "the previous file is kept"
        );
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&backup).ok();
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
