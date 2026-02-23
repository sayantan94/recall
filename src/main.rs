mod capture;
mod cli;
mod config;
mod db;
mod llm;
mod privacy;
mod search;
mod shell;
mod tui;
mod web;

use anyhow::Result;
use chrono::Utc;
use clap::Parser;
use colored::Colorize;

use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { shell }) => handle_init(&shell),
        Some(Commands::Log {
            command,
            exit_code,
            start,
            cwd,
            session,
            terminal,
            output_file,
        }) => {
            capture::log::handle_log(
                &command,
                exit_code,
                start,
                cwd.as_deref(),
                &session,
                terminal.as_deref(),
                output_file.as_deref(),
            )?;
        }
        Some(Commands::SessionId) => handle_session_id(),
        Some(Commands::Search {
            query,
            repo,
            dir,
            failed,
            limit,
        }) => handle_search(&query, repo, dir, failed, limit)?,
        Some(Commands::Today) => handle_today()?,
        Some(Commands::On { date }) => handle_on(&date)?,
        Some(Commands::Pause) => handle_pause()?,
        Some(Commands::Resume) => handle_resume()?,
        Some(Commands::Summarize) => handle_summarize().await?,
        Some(Commands::Ui) => tui::app::run_tui()?,
        Some(Commands::Web { port }) => web::server::start_server(port).await?,
        None => {
            if !cli.question.is_empty() {
                let question = cli.question.join(" ");
                handle_ask(&question).await?;
            } else {
                handle_today()?;
            }
        }
    }

    Ok(())
}

fn handle_init(shell: &str) {
    match shell {
        "zsh" => shell::zsh::print_hook(),
        other => {
            eprintln!("Unsupported shell: {}. Currently supported: zsh", other);
            std::process::exit(1);
        }
    }
}

fn handle_session_id() {
    println!("{}", uuid::Uuid::new_v4());
}

fn handle_search(
    query: &str,
    repo: Option<String>,
    dir: Option<String>,
    failed: bool,
    limit: usize,
) -> Result<()> {
    let conn = db::schema::open_db()?;
    let opts = search::engine::SearchOptions {
        query: query.to_string(),
        repo,
        dir,
        failed_only: failed,
        limit,
    };
    let results = search::engine::search(&conn, &opts)?;

    if results.is_empty() {
        println!("\n  {} {}\n", "●".dimmed(), "No matching commands found.".dimmed());
        return Ok(());
    }

    print_header(&format!("Search: \"{}\"", query), results.len());

    let cmds: Vec<&db::models::Command> = results.iter().map(|r| &r.command).collect();
    print_commands_grouped(&cmds);

    Ok(())
}

fn handle_today() -> Result<()> {
    let conn = db::schema::open_db()?;
    let commands = db::queries::get_commands_today(&conn)?;

    if commands.is_empty() {
        println!("\n  {} {}\n", "●".dimmed(), "No commands recorded today.".dimmed());
        return Ok(());
    }

    let today = chrono::Local::now().format("%b %d, %Y").to_string();
    print_header(&format!("Today — {}", today), commands.len());

    let refs: Vec<&db::models::Command> = commands.iter().collect();
    print_commands_grouped(&refs);

    Ok(())
}

fn handle_on(date: &str) -> Result<()> {
    let conn = db::schema::open_db()?;
    let commands = db::queries::get_commands_on_date(&conn, date)?;

    if commands.is_empty() {
        println!("\n  {} {}\n", "●".dimmed(), format!("No commands recorded on {}.", date).dimmed());
        return Ok(());
    }

    print_header(date, commands.len());

    let refs: Vec<&db::models::Command> = commands.iter().collect();
    print_commands_grouped(&refs);

    Ok(())
}

fn handle_pause() -> Result<()> {
    config::settings::ensure_recall_dir()?;
    std::fs::write(config::settings::pause_file(), "")?;
    println!("\n  {} {}\n", "⏸".yellow(), "Recording paused. Run `recall resume` to continue.".yellow());
    Ok(())
}

fn handle_resume() -> Result<()> {
    let pause = config::settings::pause_file();
    if pause.exists() {
        std::fs::remove_file(pause)?;
        println!("\n  {} {}\n", "▶".green(), "Recording resumed.".green());
    } else {
        println!("\n  {} {}\n", "●".dimmed(), "Recording is not paused.".dimmed());
    }
    Ok(())
}

async fn handle_summarize() -> Result<()> {
    let cfg = config::settings::load_config()?;
    let conn = db::schema::open_db()?;

    let session_ids = db::queries::get_unsummarized_sessions(&conn, 3)?;
    if session_ids.is_empty() {
        println!("\n  {} {}\n", "●".dimmed(), "No sessions to summarize.".dimmed());
        return Ok(());
    }

    println!();
    println!(
        "  {} Summarizing {} sessions...",
        "◉".cyan(),
        session_ids.len().to_string().bold()
    );
    println!("  {}", "─".repeat(50).dimmed());

    for session_id in &session_ids {
        let commands = db::queries::get_session_commands(&conn, session_id)?;
        print!(
            "  {} Session {} ",
            "│".dimmed(),
            session_id[..8].cyan()
        );

        match llm::summarizer::summarize_session(&cfg.llm, &commands).await {
            Ok((summary_text, tags, intent)) => {
                let summary = db::models::Summary {
                    id: None,
                    session_id: session_id.clone(),
                    summary_text: summary_text.clone(),
                    tags: Some(tags),
                    intent: Some(intent.clone()),
                    created_at: Utc::now().timestamp_millis(),
                };
                db::queries::insert_summary(&conn, &summary)?;
                println!("{}", "✓".green());
                println!("  {}   {}", "│".dimmed(), summary_text);
                println!("  {}   {}", "│".dimmed(), intent.dimmed());
            }
            Err(e) => {
                println!("{}", "✗".red());
                println!("  {}   {}", "│".dimmed(), format!("{}", e).red());
            }
        }
    }

    println!("  {}", "─".repeat(50).dimmed());
    println!();

    Ok(())
}

async fn handle_ask(question: &str) -> Result<()> {
    let cfg = config::settings::load_config()?;
    let conn = db::schema::open_db()?;

    let mut candidates = Vec::new();

    let words: Vec<&str> = question.split_whitespace().collect();
    for word in &words {
        if word.len() > 2 {
            if let Ok(results) = search::engine::search(
                &conn,
                &search::engine::SearchOptions {
                    query: word.to_string(),
                    limit: 10,
                    ..Default::default()
                },
            ) {
                for r in results {
                    candidates.push(r.command);
                }
            }
        }
    }

    if let Ok(recent) = search::engine::get_recent_commands(&conn, 50) {
        candidates.extend(recent);
    }

    candidates.sort_by_key(|c| c.id);
    candidates.dedup_by_key(|c| c.id);
    candidates.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    candidates.truncate(100);

    println!();
    println!("  {} {}", "◉".cyan(), "Thinking...".dimmed());

    let answer = llm::answerer::answer_question(&cfg.llm, question, &candidates).await?;

    println!();
    println!("  {}", "─".repeat(60).dimmed());
    for line in answer.lines() {
        println!("  {}", line);
    }
    println!("  {}", "─".repeat(60).dimmed());
    println!();

    Ok(())
}

// ─── Rich output helpers ────────────────────────────────────

fn print_header(title: &str, count: usize) {
    println!();
    println!(
        "  {} {}  {}",
        "◉".cyan(),
        title.bold(),
        format!("{} commands", count).dimmed()
    );
    println!("  {}", "─".repeat(60).dimmed());
}

fn print_commands_grouped(commands: &[&db::models::Command]) {
    // Group by session
    let mut groups: Vec<(&str, Vec<&db::models::Command>)> = Vec::new();
    for cmd in commands {
        if let Some(last) = groups.last_mut() {
            if last.0 == cmd.session_id.as_str() {
                last.1.push(cmd);
                continue;
            }
        }
        groups.push((&cmd.session_id, vec![cmd]));
    }

    for (session_id, cmds) in &groups {
        // Session header
        let first_ts = cmds
            .first()
            .and_then(|c| chrono::DateTime::from_timestamp_millis(c.timestamp))
            .map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "?".to_string());

        let dir = cmds
            .first()
            .and_then(|c| c.cwd.as_deref())
            .and_then(|d| d.rsplit('/').next())
            .unwrap_or("~");

        let repo = cmds
            .iter()
            .find_map(|c| c.git_repo.as_ref())
            .map(|r| r.rsplit('/').next().unwrap_or(r))
            .unwrap_or("");

        let branch = cmds
            .iter()
            .find_map(|c| c.git_branch.as_ref())
            .map(|b| b.as_str())
            .unwrap_or("");

        let fail_count = cmds
            .iter()
            .filter(|c| c.exit_code.is_some_and(|code| code != 0))
            .count();

        // Session line
        print!(
            "\n  {} {}  {}",
            "┌".dimmed(),
            first_ts.white().bold(),
            session_id[..8].dimmed()
        );

        if !dir.is_empty() {
            print!("  {}", dir.blue());
        }
        if !repo.is_empty() {
            print!("  {}", repo.green());
            if !branch.is_empty() {
                print!("{}{}", ":".dimmed(), branch.magenta());
            }
        }
        if fail_count > 0 {
            print!("  {}", format!("{} failed", fail_count).red());
        }
        println!();

        // Commands
        for (i, cmd) in cmds.iter().enumerate() {
            let is_last = i == cmds.len() - 1;
            let connector = if is_last { "└" } else { "│" };

            let ts = chrono::DateTime::from_timestamp_millis(cmd.timestamp)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|| "?".to_string());

            let dur = cmd
                .duration_ms
                .map(|d| {
                    if d >= 60_000 {
                        format!("{}m{}s", d / 60_000, (d % 60_000) / 1000)
                    } else if d >= 1000 {
                        format!("{}.{}s", d / 1000, (d % 1000) / 100)
                    } else {
                        format!("{}ms", d)
                    }
                })
                .unwrap_or_else(|| "-".to_string());

            let exit_icon = match cmd.exit_code {
                Some(0) => "✓".green().to_string(),
                Some(_) => "✗".red().to_string(),
                None => "·".dimmed().to_string(),
            };

            let is_fail = cmd.exit_code.is_some_and(|code| code != 0);

            let cmd_text = if is_fail {
                cmd.command_text.red().to_string()
            } else {
                cmd.command_text.to_string()
            };

            println!(
                "  {} {} {} {:>7}  {}",
                connector.dimmed(),
                exit_icon,
                ts.dimmed(),
                dur.dimmed(),
                cmd_text
            );
        }
    }

    println!();
}
