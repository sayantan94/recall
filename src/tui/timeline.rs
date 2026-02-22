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

    let header = Paragraph::new(Line::from(vec![
        Span::styled("  recall", Style::default().fg(Color::Cyan).bold()),
        Span::styled("  your shell, remembered", Style::default().fg(Color::DarkGray)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(header, chunks[0]);

    let stats = Paragraph::new(Line::from(vec![
        Span::styled("  Sessions: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", app.session_infos.len()),
            Style::default().fg(Color::White).bold(),
        ),
        Span::styled("    Commands: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", app.total_commands),
            Style::default().fg(Color::White).bold(),
        ),
        Span::styled("    Selected: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(
                "{}/{}",
                if app.session_infos.is_empty() { 0 } else { app.selected_session + 1 },
                app.session_infos.len()
            ),
            Style::default().fg(Color::Yellow),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(stats, chunks[1]);

    let visible_height = chunks[2].height.saturating_sub(2) as usize;
    let end = (app.scroll_offset + visible_height).min(app.session_infos.len());

    let items: Vec<ListItem> = app
        .session_infos
        .iter()
        .enumerate()
        .skip(app.scroll_offset)
        .take(visible_height)
        .map(|(i, info)| {
            let ts = chrono::DateTime::from_timestamp_millis(info.session.start_time)
                .map(|dt| dt.format("%b %d  %H:%M").to_string())
                .unwrap_or_else(|| "?".to_string());

            let terminal = info
                .session
                .terminal_app
                .as_deref()
                .unwrap_or("?");

            let dir = info
                .session
                .initial_dir
                .as_deref()
                .and_then(|d| d.rsplit('/').next())
                .unwrap_or("~");

            let repos_str = if info.repos.is_empty() {
                String::new()
            } else {
                format!("  {}", info.repos.join(", "))
            };

            let cmd_count = format!("{}cmd", info.command_count);

            let is_selected = i == app.selected_session;

            let marker = if is_selected { ">" } else { " " };

            let fail_dot = if info.has_failures { "!" } else { " " };

            let mut spans = vec![
                Span::styled(
                    format!(" {}", marker),
                    if is_selected {
                        Style::default().fg(Color::Cyan).bold()
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(
                    format!(" {} ", ts),
                    if is_selected {
                        Style::default().fg(Color::White).bold()
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
                Span::styled(
                    format!("{:<10}", terminal),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:<14}", dir),
                    if is_selected {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::Blue)
                    },
                ),
                Span::styled(
                    format!("{:>5}", cmd_count),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!(" {}", fail_dot),
                    if info.has_failures {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default()
                    },
                ),
            ];

            if !repos_str.is_empty() {
                spans.push(Span::styled(
                    repos_str,
                    Style::default().fg(Color::Green),
                ));
            }

            let line = Line::from(spans);

            let style = if is_selected {
                Style::default().bg(Color::Rgb(30, 35, 50))
            } else {
                Style::default()
            };

            ListItem::new(line).style(style)
        })
        .collect();

    let scrollbar_info = if app.session_infos.len() > visible_height {
        format!(
            " {}-{} of {} ",
            app.scroll_offset + 1,
            end,
            app.session_infos.len()
        )
    } else {
        String::new()
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Sessions ")
            .title_style(Style::default().fg(Color::White).bold())
            .title_alignment(Alignment::Left)
            .title_bottom(Line::from(scrollbar_info).right_aligned()),
    );

    frame.render_widget(list, chunks[2]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled(" ↑/↓", Style::default().fg(Color::Cyan)),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::styled(" open  ", Style::default().fg(Color::DarkGray)),
        Span::styled("/", Style::default().fg(Color::Cyan)),
        Span::styled(" search  ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::styled(" quit", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(help, chunks[3]);
}
