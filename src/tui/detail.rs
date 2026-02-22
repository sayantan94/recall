use ratatui::prelude::*;
use ratatui::widgets::*;

use super::app::App;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let info = app.session_infos.get(app.selected_session);

    let session_label = info
        .map(|i| i.session.id.chars().take(8).collect::<String>())
        .unwrap_or_default();

    let session_time = info
        .and_then(|i| chrono::DateTime::from_timestamp_millis(i.session.start_time))
        .map(|dt| dt.format("%b %d, %Y  %H:%M").to_string())
        .unwrap_or_else(|| "?".to_string());

    let header = Paragraph::new(Line::from(vec![
        Span::styled("  session ", Style::default().fg(Color::DarkGray)),
        Span::styled(&session_label, Style::default().fg(Color::Cyan).bold()),
        Span::styled("  ", Style::default()),
        Span::styled(session_time, Style::default().fg(Color::White)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(header, chunks[0]);

    let terminal = info
        .and_then(|i| i.session.terminal_app.as_deref())
        .unwrap_or("?");

    let dir = info
        .and_then(|i| i.session.initial_dir.as_deref())
        .unwrap_or("~");

    let repos_str = info
        .map(|i| {
            if i.repos.is_empty() {
                String::from("none")
            } else {
                i.repos.join(", ")
            }
        })
        .unwrap_or_else(|| "none".to_string());

    let cmd_count = app.session_commands.len();
    let fail_count = app
        .session_commands
        .iter()
        .filter(|c| c.exit_code.is_some_and(|code| code != 0))
        .count();

    let meta = Paragraph::new(Line::from(vec![
        Span::styled("  ", Style::default().fg(Color::DarkGray)),
        Span::styled(terminal, Style::default().fg(Color::Gray)),
        Span::styled("  dir: ", Style::default().fg(Color::DarkGray)),
        Span::styled(dir, Style::default().fg(Color::Blue)),
        Span::styled("  repos: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&repos_str, Style::default().fg(Color::Green)),
        Span::styled(
            format!("  {} cmds", cmd_count),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            if fail_count > 0 {
                format!("  {} failed", fail_count)
            } else {
                String::new()
            },
            Style::default().fg(Color::Red),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(meta, chunks[1]);

    let visible_height = chunks[2].height.saturating_sub(2) as usize;
    let end = (app.cmd_scroll_offset + visible_height).min(app.session_commands.len());

    let items: Vec<ListItem> = app
        .session_commands
        .iter()
        .enumerate()
        .skip(app.cmd_scroll_offset)
        .take(visible_height)
        .map(|(i, cmd)| {
            let ts = chrono::DateTime::from_timestamp_millis(cmd.timestamp)
                .map(|dt| dt.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "?".to_string());

            let exit_indicator = match cmd.exit_code {
                Some(0) => " ",
                Some(code) => {
                    // We'll handle the styling per-span below
                    if code != 0 { "!" } else { " " }
                }
                None => "?",
            };

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

            let is_selected = i == app.selected_command;

            let marker = if is_selected { ">" } else { " " };

            let git_info = match (&cmd.git_repo, &cmd.git_branch) {
                (Some(repo), Some(branch)) => {
                    let repo_name = repo.rsplit('/').next().unwrap_or(repo);
                    format!(" {}:{}", repo_name, branch)
                }
                (Some(repo), None) => {
                    let repo_name = repo.rsplit('/').next().unwrap_or(repo);
                    format!(" {}", repo_name)
                }
                _ => String::new(),
            };

            let is_failure = cmd.exit_code.is_some_and(|code| code != 0);

            let spans = vec![
                Span::styled(
                    format!(" {}", marker),
                    if is_selected {
                        Style::default().fg(Color::Cyan).bold()
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(
                    format!(" {} ", exit_indicator),
                    if is_failure {
                        Style::default().fg(Color::Red).bold()
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(
                    format!("{} ", ts),
                    if is_selected {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
                Span::styled(
                    format!("{:>7} ", dur),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    cmd.command_text.clone(),
                    if is_selected {
                        Style::default().fg(Color::White).bold()
                    } else if is_failure {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
                Span::styled(git_info, Style::default().fg(Color::Green)),
            ];

            let line = Line::from(spans);

            let style = if is_selected {
                Style::default().bg(Color::Rgb(30, 35, 50))
            } else {
                Style::default()
            };

            ListItem::new(line).style(style)
        })
        .collect();

    let scrollbar_info = if app.session_commands.len() > visible_height {
        format!(
            " {}-{} of {} ",
            app.cmd_scroll_offset + 1,
            end,
            app.session_commands.len()
        )
    } else {
        String::new()
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Commands ")
            .title_style(Style::default().fg(Color::White).bold())
            .title_alignment(Alignment::Left)
            .title_bottom(Line::from(scrollbar_info).right_aligned()),
    );

    frame.render_widget(list, chunks[2]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled(" ↑/↓", Style::default().fg(Color::Cyan)),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::styled(" back  ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::styled(" back", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(help, chunks[3]);
}
