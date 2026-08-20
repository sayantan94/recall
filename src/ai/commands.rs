//! `recall sessions ...` — terminal output for the AI session index.

use anyhow::{anyhow, Result};
use colored::Colorize;
use rusqlite::Connection;

use crate::cli::{AgentFilters, AgentsCommand};

use super::indexer;
use super::models::{AiSearchResult, AiSession, Source};
use super::resume;
use super::search::{self, Mode};
use super::store::{self, Filter};

pub fn handle(command: Option<AgentsCommand>) -> Result<()> {
    let conn = crate::db::schema::open_db()?;

    match command.unwrap_or(AgentsCommand::List {
        filters: AgentFilters {
            source: None,
            project: None,
            limit: 20,
            no_index: false,
        },
    }) {
        AgentsCommand::Index { force, source } => handle_index(&conn, force, source.as_deref()),
        AgentsCommand::Search {
            query,
            filters,
            fuzzy,
        } => handle_search(&conn, &query, &filters, fuzzy),
        AgentsCommand::List { filters } => handle_list(&conn, &filters),
        AgentsCommand::Show { session } => handle_show(&conn, &session),
        AgentsCommand::Resume { session, dir, print } => {
            handle_resume(&conn, session.as_deref(), dir.as_deref(), print)
        }
        AgentsCommand::Stats => handle_stats(&conn),
    }
}

fn parse_source(raw: &str) -> Result<Source> {
    Source::parse(raw).ok_or_else(|| anyhow!("Unknown source `{}`. Use claude or codex.", raw))
}

/// Keep the index honest before answering. A warm reconcile is well under a
/// second; narrate it on stderr so scripted and piped output stays clean.
fn refresh_index(conn: &Connection, skip: bool) -> Result<()> {
    if skip {
        return Ok(());
    }

    let cold = store::stats(conn)?.sessions == 0;
    if cold {
        eprintln!(
            "  {} {}",
            "●".dimmed(),
            "First run — indexing Claude Code and Codex transcripts...".dimmed()
        );
    }

    let report = indexer::index_all(conn, false)?;
    let changed = report.added + report.updated + report.removed;

    if cold {
        eprintln!(
            "  {} {}",
            "✓".green(),
            format!("{} sessions indexed", store::stats(conn)?.sessions).dimmed()
        );
    } else if changed > 0 {
        eprintln!(
            "  {} {}",
            "●".dimmed(),
            format!("index refreshed — {} sessions changed", changed).dimmed()
        );
    }

    Ok(())
}

fn build_filter(filters: &AgentFilters) -> Result<Filter> {
    Ok(Filter {
        source: filters.source.as_deref().map(parse_source).transpose()?,
        project: filters.project.clone(),
        limit: filters.limit.max(1),
    })
}

fn handle_index(conn: &Connection, force: bool, source: Option<&str>) -> Result<()> {
    println!();
    println!(
        "  {} {}",
        "◉".cyan(),
        "Indexing agent sessions...".bold()
    );
    println!("  {}", "─".repeat(60).dimmed());

    let report = match source {
        Some(raw) => indexer::index_source(conn, parse_source(raw)?, force)?,
        None => indexer::index_all(conn, force)?,
    };

    println!(
        "  {} {} added   {} updated   {} unchanged   {} removed",
        "│".dimmed(),
        report.added.to_string().green().bold(),
        report.updated.to_string().yellow().bold(),
        report.unchanged.to_string().dimmed(),
        report.removed.to_string().red().bold(),
    );
    println!(
        "  {} {} chunks indexed",
        "│".dimmed(),
        report.chunks.to_string().cyan()
    );

    for (path, error) in &report.failed {
        println!("  {} {} {}", "│".dimmed(), "skipped".red(), path.dimmed());
        println!("  {}   {}", "│".dimmed(), error.dimmed());
    }

    println!("  {}", "─".repeat(60).dimmed());
    println!();
    Ok(())
}

fn handle_search(
    conn: &Connection,
    query: &str,
    filters: &AgentFilters,
    fuzzy: bool,
) -> Result<()> {
    refresh_index(conn, filters.no_index)?;
    let filter = build_filter(filters)?;
    let mode = if fuzzy { Mode::Fuzzy } else { Mode::Fts };
    let (results, used) = search::search(conn, query, &filter, mode)?;

    if results.is_empty() {
        empty_note(conn, &format!("No sessions match \"{}\".", query))?;
        return Ok(());
    }

    println!();
    println!(
        "  {} {}  {}",
        "◉".cyan(),
        format!("Sessions: \"{}\"", query).bold(),
        format!("{} results · {}", results.len(), used.label()).dimmed()
    );
    println!("  {}", "─".repeat(60).dimmed());

    for result in &results {
        print_result(result);
    }

    println!();
    println!(
        "  {}",
        "recall agents resume <id>  to reopen one".dimmed()
    );
    println!();
    Ok(())
}

fn handle_list(conn: &Connection, filters: &AgentFilters) -> Result<()> {
    refresh_index(conn, filters.no_index)?;
    let filter = build_filter(filters)?;
    let sessions = store::list_sessions(conn, &filter)?;

    if sessions.is_empty() {
        empty_note(conn, "No agent sessions indexed yet.")?;
        return Ok(());
    }

    println!();
    println!(
        "  {} {}  {}",
        "◉".cyan(),
        "Agent sessions".bold(),
        format!("{} shown", sessions.len()).dimmed()
    );
    println!("  {}", "─".repeat(60).dimmed());

    for session in &sessions {
        print_session_line(session);
    }
    println!();
    Ok(())
}

fn handle_show(conn: &Connection, reference: &str) -> Result<()> {
    let session = resolve_one(conn, reference)?;
    let chunks = store::session_chunks(conn, &session.uid)?;

    println!();
    println!(
        "  {} {}",
        "◉".cyan(),
        session.title.as_deref().unwrap_or("(untitled session)").bold()
    );
    if let Some(name) = &session.custom_name {
        println!("  {} {}", "★".cyan(), format!("saved as \"{}\"", name).cyan());
    }
    println!(
        "  {}  {}  {}",
        session.source.label().magenta(),
        session.project.blue(),
        session.uid.dimmed()
    );
    if let Some(model) = &session.model {
        println!("  {}  {}", "model".dimmed(), model.dimmed());
    }
    println!(
        "  {}  {} messages · {} chunks",
        format_time(session.started_at).dimmed(),
        session.message_count,
        chunks.len()
    );
    println!("  {}", "─".repeat(60).dimmed());

    for chunk in &chunks {
        for line in chunk.text.lines() {
            if let Some(rest) = line.strip_prefix("USER: ") {
                println!("  {} {}", "▸".cyan().bold(), rest.white().bold());
            } else if let Some(rest) = line.strip_prefix("ASSISTANT: ") {
                println!("  {} {}", "▸".green(), rest);
            } else {
                println!("    {}", line);
            }
        }
    }

    println!("  {}", "─".repeat(60).dimmed());
    println!();
    Ok(())
}

fn handle_resume(
    conn: &Connection,
    reference: Option<&str>,
    dir: Option<&str>,
    print_only: bool,
) -> Result<()> {
    let session = match reference {
        Some(reference) => resolve_one(conn, reference)?,
        // No argument means "drop me back into what I was just doing".
        None => store::list_sessions(conn, &Filter::with_limit(1))?
            .into_iter()
            .next()
            .ok_or_else(|| {
                anyhow!("No agent sessions indexed yet. Run `recall agents index` first.")
            })?,
    };
    let spec = resume::resume_command(&session, dir);

    if print_only {
        println!("cd {} && {}", spec.cwd, spec.display());
        return Ok(());
    }

    println!();
    println!(
        "  {} {}  {}",
        "▶".green(),
        spec.display().bold(),
        format!("in {}", spec.cwd).dimmed()
    );
    println!();

    resume::exec(&spec)
}

fn handle_stats(conn: &Connection) -> Result<()> {
    let stats = store::stats(conn)?;

    println!();
    println!("  {} {}", "◉".cyan(), "Agent session index".bold());
    println!("  {}", "─".repeat(60).dimmed());
    println!(
        "  {} {} sessions across {} projects, {} chunks",
        "│".dimmed(),
        stats.sessions.to_string().white().bold(),
        stats.projects.to_string().white().bold(),
        stats.chunks.to_string().white().bold()
    );
    for (source, count) in &stats.per_source {
        println!(
            "  {}   {:<14} {}",
            "│".dimmed(),
            source.label().magenta(),
            count
        );
    }
    println!("  {}", "─".repeat(60).dimmed());
    println!();
    Ok(())
}

/// Resolve a session reference, reporting ambiguity rather than guessing.
fn resolve_one(conn: &Connection, reference: &str) -> Result<AiSession> {
    let matches = store::resolve_session(conn, reference)?;

    match matches.len() {
        0 => Err(anyhow!(
            "No indexed session matches `{}`. Run `recall agents index` first.",
            reference
        )),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => {
            let listed = matches
                .iter()
                .take(5)
                .map(|s| format!("  {}  {}", s.uid, s.project))
                .collect::<Vec<_>>()
                .join("\n");
            Err(anyhow!(
                "`{}` matches {} sessions:\n{}\nUse a longer prefix.",
                reference,
                matches.len(),
                listed
            ))
        }
    }
}

fn empty_note(conn: &Connection, message: &str) -> Result<()> {
    println!("\n  {} {}", "●".dimmed(), message.dimmed());
    if store::stats(conn)?.sessions == 0 {
        println!(
            "  {} {}",
            "●".dimmed(),
            "Run `recall agents index` to build the index.".dimmed()
        );
    }
    println!();
    Ok(())
}

fn print_result(result: &AiSearchResult) {
    print_session_line(&result.session);
    let snippet = result.snippet.replace('\n', " ");
    println!("      {}", truncate(&snippet, 100).dimmed());
}

fn print_session_line(session: &AiSession) {
    let title = session
        .title
        .as_deref()
        .map(|t| truncate(&t.replace('\n', " "), 58))
        .unwrap_or_else(|| "(untitled)".to_string());

    let project = session
        .project
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(&session.project);

    println!(
        "\n  {} {}  {:<7} {}{}",
        "┌".dimmed(),
        format_time(session.last_activity).white().bold(),
        session.source.as_str().magenta(),
        if session.custom_name.is_some() {
            "★ ".cyan().to_string()
        } else {
            String::new()
        },
        title.white()
    );
    println!(
        "  {} {}  {}  {}",
        "└".dimmed(),
        project.blue(),
        format!("{} msg", session.message_count).dimmed(),
        short_id(&session.session_id).dimmed()
    );
}

fn format_time(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%b %d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "?".to_string())
}

fn short_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
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

    #[test]
    fn truncate_leaves_short_text_alone() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_marks_elided_text() {
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn short_id_takes_a_readable_prefix() {
        assert_eq!(short_id("019d3b6a-8a99-72b0"), "019d3b6a");
        assert_eq!(short_id("abc"), "abc");
    }

    #[test]
    fn parse_source_rejects_unknown_tools() {
        assert!(parse_source("claude").is_ok());
        assert!(parse_source("codex").is_ok());
        assert!(parse_source("cursor").is_err());
    }
}
