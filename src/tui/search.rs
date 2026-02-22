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
        Span::styled("  search", Style::default().fg(Color::DarkGray)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(header, chunks[0]);

    let cursor_char = if app.search_results.is_empty() { "_" } else { "" };
    let input = Paragraph::new(Line::from(vec![
        Span::styled("  / ", Style::default().fg(Color::Cyan).bold()),
        Span::styled(
            format!("{}{}", app.search_input, cursor_char),
            Style::default().fg(Color::White),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Query ")
            .title_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(input, chunks[1]);

    let visible_height = chunks[2].height.saturating_sub(2) as usize;
    let end = (app.search_scroll_offset + visible_height).min(app.search_results.len());

    let items: Vec<ListItem> = app
        .search_results
        .iter()
        .enumerate()
        .skip(app.search_scroll_offset)
        .take(visible_height)
        .map(|(i, cmd)| {
            let ts = chrono::DateTime::from_timestamp_millis(cmd.timestamp)
                .map(|dt| dt.format("%b %d  %H:%M").to_string())
                .unwrap_or_else(|| "?".to_string());

            let is_selected = i == app.search_selected;
            let is_failure = cmd.exit_code.is_some_and(|code| code != 0);

            let marker = if is_selected { ">" } else { " " };

            let exit_indicator = if is_failure { "!" } else { " " };

            let dir = cmd
                .cwd
                .as_deref()
                .and_then(|d| d.rsplit('/').next())
                .unwrap_or("");

            let git_info = cmd
                .git_repo
                .as_ref()
                .map(|r| {
                    let name = r.rsplit('/').next().unwrap_or(r);
                    format!(" {}", name)
                })
                .unwrap_or_default();

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
                    format!("{:<12}", dir),
                    if is_selected {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::Blue)
                    },
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

    let result_count = app.search_results.len();
    let title = if result_count > 0 {
        format!(" Results ({}) ", result_count)
    } else if app.search_input.is_empty() {
        " Results ".to_string()
    } else {
        " No results ".to_string()
    };

    let scrollbar_info = if result_count > visible_height {
        format!(
            " {}-{} of {} ",
            app.search_scroll_offset + 1,
            end,
            result_count
        )
    } else {
        String::new()
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(title)
            .title_style(Style::default().fg(Color::White).bold())
            .title_alignment(Alignment::Left)
            .title_bottom(Line::from(scrollbar_info).right_aligned()),
    );

    frame.render_widget(list, chunks[2]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled(" Type", Style::default().fg(Color::DarkGray)),
        Span::styled(" query  ", Style::default().fg(Color::Cyan)),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::styled(" search  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Up/Down", Style::default().fg(Color::Cyan)),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::styled(" back", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(help, chunks[3]);
}
