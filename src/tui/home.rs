//! The unified home screen: grouped list on the left, details and content on
//! the right, with a help overlay and a resume dialog on top.

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::db::models::Command;

use super::app::{App, Entry, Focus, Kind, Preview, Row};

// Everything here is a named ANSI colour or the terminal's own default, so the
// UI inherits whatever theme the user already reads comfortably. No hand-picked
// RGB text colours: those assume a background we do not control.
//
/// Body text: the terminal's default foreground.
const TEXT: Color = Color::Reset;
/// Labels, chrome and metadata.
const DIM: Color = Color::DarkGray;
/// The single interactive accent.
const ACCENT: Color = Color::Cyan;
/// Search matches. The terminal's own yellow, never a brightened one.
const HIGHLIGHT: Color = Color::Yellow;

/// The selected row is outlined, not painted. Any fill — reverse video or a
/// chosen background — turns the row into a block of the terminal's foreground
/// and buries the text on whichever theme we guessed wrong about, so the
/// selection is carried entirely by shape and weight.
fn row_style(selected: bool, pane_focused: bool) -> Style {
    if selected && pane_focused {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

/// Thin box edges drawn in the terminal's own foreground. They sit inside the
/// pane border and read as an outline around the current row without borrowing
/// a single colour.
const EDGE_LEFT: &str = "▏";
const EDGE_RIGHT: &str = "▕";

fn edge(selected: bool, glyph: &'static str) -> Span<'static> {
    Span::styled(if selected { glyph } else { " " }, Style::default())
}

pub fn render(frame: &mut Frame, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_header(frame, rows[0], app);
    render_tabs(frame, rows[1], app);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[2]);

    render_list(frame, panes[0], app);
    render_details(frame, panes[1], app);
    render_search(frame, rows[3], app);
    render_status(frame, rows[4], app);

    if app.show_help {
        render_help_overlay(frame, frame.area(), app);
    }
    if app.resume_dialog.is_some() {
        render_resume_dialog(frame, frame.area(), app);
    }
}

fn pane_block(title: &'static str, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { DIM }))
        .title(format!(" {} ", title))
        .title_style(Style::default().fg(if focused { ACCENT } else { DIM }))
}

// ─── Header ─────────────────────────────────────────────────

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  recall", Style::default().fg(ACCENT).bold()),
            Span::styled(
                format!(
                    "   {} agent sessions · {} commands indexed",
                    app.total_agent_sessions, app.total_commands
                ),
                Style::default().fg(DIM),
            ),
        ])),
        area,
    );
}

/// The source switcher. Every tab shows its own hit count for the current
/// query, so you can see where the matches are before switching.
fn render_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let active = app.tab_index();
    let total: usize = app.counts.iter().map(|(_, count)| count).sum();

    let mut tabs: Vec<(String, usize, Option<Kind>)> = vec![("All".into(), total, None)];
    for (kind, count) in &app.counts {
        tabs.push((kind.label().to_string(), *count, Some(*kind)));
    }

    let mut spans = vec![Span::raw("  ")];
    for (i, (label, count, kind)) in tabs.iter().enumerate() {
        let selected = i == active;
        spans.push(Span::raw("  "));

        // The active tab gets the same colourless outline as the selected row.
        let accent = kind.map(|k| k.color()).unwrap_or(ACCENT);
        spans.push(edge(selected, EDGE_LEFT));
        spans.push(Span::styled(
            format!(" {} ", i + 1),
            Style::default().fg(DIM),
        ));
        if kind.is_some() {
            spans.push(Span::styled("● ", Style::default().fg(accent)));
        }
        spans.push(Span::styled(
            label.clone(),
            if selected {
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(DIM)
            },
        ));
        spans.push(Span::styled(
            format!(" {} ", count),
            Style::default().fg(if selected { TEXT } else { DIM }),
        ));
        spans.push(edge(selected, EDGE_RIGHT));
    }

    spans.push(Span::styled("     ", Style::default()));
    spans.push(Span::styled("⇧←/⇧→", Style::default().fg(ACCENT)));
    spans.push(Span::styled(" switch source", Style::default().fg(DIM)));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ─── List ───────────────────────────────────────────────────

fn render_list(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::List;
    let visible = area.height.saturating_sub(2) as usize;
    let width = area.width as usize;
    let terms = app.query_terms();

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .skip(app.scroll_offset)
        .take(visible)
        .map(|(i, row)| match row {
            Row::Header { kind, count } => header_row(
                *kind,
                *count,
                width,
                i == app.selected_row,
                focused,
                app.is_collapsed(*kind),
            ),
            Row::Item(index) => {
                let entry = &app.entries[*index];
                item_row(entry, i == app.selected_row, focused, width, &terms)
            }
        })
        .collect();

    let total = app.entries.len();
    let footer = if total == 0 {
        String::new()
    } else {
        format!(" {}/{} ", app.selected_position(), total)
    };

    frame.render_widget(
        List::new(items).block(
            pane_block("Sessions", focused)
                .title_bottom(Line::from(footer).right_aligned()),
        ),
        area,
    );

    render_scrollbar(frame, area, app.rows.len(), visible, app.scroll_offset);
}

/// A selectable group divider: `▾ CLAUDE CODE ──────────── 231`
fn header_row(
    kind: Kind,
    count: usize,
    width: usize,
    selected: bool,
    pane_focused: bool,
    collapsed: bool,
) -> ListItem<'static> {
    let label = kind.label().to_uppercase();
    let count_text = format!("{}", count);
    let marker = if collapsed { "▸" } else { "▾" };
    let used = label.chars().count() + count_text.chars().count() + 10;
    let fill = width.saturating_sub(used).max(1);

    let row = row_style(selected, pane_focused);
    let label_style = Style::default()
        .fg(kind.color())
        .add_modifier(Modifier::BOLD);

    ListItem::new(Line::from(vec![
        edge(selected, EDGE_LEFT),
        Span::styled(format!(" {} ", marker), label_style),
        Span::styled(format!("{} ", label), label_style),
        Span::styled("─".repeat(fill), Style::default().fg(DIM)),
        Span::styled(format!(" {} ", count_text), Style::default().fg(DIM)),
        edge(selected, EDGE_RIGHT),
    ]))
    .style(row)
}

fn item_row(
    entry: &Entry,
    selected: bool,
    pane_focused: bool,
    width: usize,
    terms: &[String],
) -> ListItem<'static> {
    let kind = entry.kind();
    // edges(2) + tag(8) + failure(1) + time(8) + pane borders(2)
    let title_width = width.saturating_sub(21).max(8);

    let row = row_style(selected, pane_focused);
    let title = truncate(&entry.title(), title_width);

    let title_style = if selected {
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };

    let mut spans = vec![
        edge(selected, EDGE_LEFT),
        Span::styled(
            format!(" {:<7}", kind.tag()),
            Style::default().fg(kind.color()),
        ),
    ];
    spans.extend(highlight(&title, terms, title_style));
    spans.push(Span::raw(
        " ".repeat(title_width.saturating_sub(title.chars().count())),
    ));

    let failed = matches!(entry, Entry::Shell { failures, .. } if *failures > 0);
    spans.push(Span::styled(
        if failed { "!" } else { " " },
        Style::default().fg(Color::Red),
    ));
    spans.push(Span::styled(
        format!("{:>7} ", ago(entry.last_activity())),
        Style::default().fg(DIM),
    ));
    spans.push(edge(selected, EDGE_RIGHT));

    ListItem::new(Line::from(spans)).style(row)
}

fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    total: usize,
    visible: usize,
    offset: usize,
) {
    if total <= visible {
        return;
    }
    let mut state = ScrollbarState::new(total.saturating_sub(visible)).position(offset);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(Style::default().fg(DIM))
            .thumb_style(Style::default().fg(ACCENT)),
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

// ─── Details ────────────────────────────────────────────────

fn render_details(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content;
    let block = pane_block("Details", focused);

    if let Some((kind, _)) = app.selected_group() {
        render_group_summary(frame, area, app, kind, block);
        return;
    }

    let entry = match app.selected_entry() {
        Some(entry) => entry,
        None => {
            let hint = if app.input.trim().is_empty() {
                "Nothing indexed yet — run `recall setup`."
            } else {
                "Nothing matches that filter.  Esc clears it."
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(""),
                    Line::from(Span::styled(
                        format!("  {}", hint),
                        Style::default().fg(DIM),
                    )),
                ])
                .block(block),
                area,
            );
            return;
        }
    };

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let searching = !app.input.trim().is_empty();
    let header_height = if searching { 8 } else { 6 };

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    let mut fields = detail_fields(app, entry);
    if searching {
        fields.push(Line::raw(""));
        let mut spans = vec![Span::styled(
            " Match:     ",
            Style::default().fg(DIM),
        )];
        spans.extend(highlight(
            &truncate(entry.snippet(), 88),
            &app.query_terms(),
            Style::default().fg(TEXT),
        ));
        fields.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(fields), sections[0]);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ─── ", Style::default().fg(DIM)),
            Span::styled("Content", Style::default().fg(DIM)),
            Span::styled(" ───", Style::default().fg(DIM)),
        ])),
        sections[1],
    );
    render_content(frame, sections[2], app);
}

/// What a selected group header shows on the right: what's in the group and
/// what you can do with it.
fn render_group_summary(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    kind: Kind,
    block: Block<'static>,
) {
    let summary = app.group_summary(kind);
    let collapsed = app.is_collapsed(kind);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  ● ", Style::default().fg(kind.color())),
            Span::styled(
                kind.label().to_string(),
                Style::default().fg(kind.color()).bold(),
            ),
        ]),
        Line::raw(""),
        field(
            "Showing",
            format!("{} of these in the current view", summary.count),
            TEXT,
        ),
        field("Projects", format!("{}", summary.projects), TEXT),
    ];

    if let (Some(newest), Some(oldest)) = (summary.newest, summary.oldest) {
        lines.push(field(
            "Span",
            format!("{} ago  →  {} ago", ago(oldest), ago(newest)),
            TEXT,
        ));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(ACCENT)),
        Span::styled(
            format!("  show only {}", kind.label()),
            Style::default().fg(DIM),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" Space", Style::default().fg(ACCENT)),
        Span::styled(
            format!("  {} this group", if collapsed { "expand" } else { "fold" }),
            Style::default().fg(DIM),
        ),
    ]));

    if !summary.recent_titles.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(" ─── ", Style::default().fg(DIM)),
            Span::styled("Most recent", Style::default().fg(DIM)),
            Span::styled(" ───", Style::default().fg(DIM)),
        ]));
        for title in &summary.recent_titles {
            lines.push(Line::from(vec![
                Span::styled(" · ", Style::default().fg(DIM)),
                Span::styled(
                    truncate(&title.replace('\n', " "), 76),
                    Style::default().fg(TEXT),
                ),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

fn field(label: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {:<10}", format!("{}:", label)),
            Style::default().fg(DIM),
        ),
        Span::styled(value, Style::default().fg(color)),
    ])
}

fn detail_fields(app: &App, entry: &Entry) -> Vec<Line<'static>> {
    let kind = entry.kind();

    match entry {
        Entry::Agent { session, .. } => vec![
            field("Source", kind.label().to_string(), kind.color()),
            field("Session", session.session_id.clone(), TEXT),
            field("Project", truncate(&session.project, 70), TEXT),
            field(
                "Date",
                format!("{}   {}", full_date(session.started_at), ago(session.last_activity)),
                TEXT,
            ),
            field(
                "Model",
                session.model.clone().unwrap_or_else(|| "—".to_string()),
                TEXT,
            ),
            Line::from(vec![
                Span::styled(" Search:    ", Style::default().fg(DIM)),
                Span::styled(app.mode_label(), Style::default().fg(TEXT)),
                Span::styled(
                    format!("   {} messages   ", session.message_count),
                    Style::default().fg(DIM),
                ),
                Span::styled("Enter", Style::default().fg(ACCENT)),
                Span::styled(" to resume", Style::default().fg(DIM)),
            ]),
        ],
        Entry::Shell {
            session,
            command_count,
            failures,
            repos,
            ..
        } => vec![
            field("Source", "Shell".to_string(), kind.color()),
            field("Session", short_id(&session.id), TEXT),
            field(
                "Directory",
                truncate(session.initial_dir.as_deref().unwrap_or("—"), 70),
                TEXT,
            ),
            field("Started", full_date(session.start_time), TEXT),
            field(
                "Terminal",
                session.terminal_app.clone().unwrap_or_else(|| "—".into()),
                TEXT,
            ),
            Line::from(vec![
                Span::styled(" Repos:     ", Style::default().fg(DIM)),
                Span::styled(
                    if repos.is_empty() {
                        "—".to_string()
                    } else {
                        truncate(&repos.join(", "), 40)
                    },
                    Style::default().fg(TEXT),
                ),
                Span::styled(
                    format!("   {} commands", command_count),
                    Style::default().fg(DIM),
                ),
                Span::styled(
                    if *failures > 0 {
                        format!("   {} failed", failures)
                    } else {
                        String::new()
                    },
                    Style::default().fg(Color::Red),
                ),
            ]),
        ],
    }
}

fn render_content(frame: &mut Frame, area: Rect, app: &App) {
    let height = area.height as usize;
    let terms = app.query_terms();

    let (lines, total) = match &app.preview {
        Preview::Transcript(transcript) => (
            transcript
                .iter()
                .skip(app.preview_scroll)
                .take(height)
                .map(|line| transcript_line(line, &terms))
                .collect::<Vec<_>>(),
            transcript.len(),
        ),
        Preview::Commands(commands) => (
            commands
                .iter()
                .skip(app.preview_scroll)
                .take(height)
                .map(|cmd| command_line(cmd, &terms))
                .collect::<Vec<_>>(),
            commands.len(),
        ),
        Preview::Empty => (Vec::new(), 0),
    };

    let footer = if total > height {
        format!(
            " {}/{} ",
            (app.preview_scroll + height).min(total),
            total
        )
    } else {
        String::new()
    };

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().title_bottom(Line::from(footer).right_aligned())),
        area,
    );
}

fn transcript_line(line: &str, terms: &[String]) -> Line<'static> {
    if let Some(rest) = line.strip_prefix("USER: ") {
        let mut spans = vec![Span::styled(" ▸ ", Style::default().fg(ACCENT).bold())];
        spans.extend(highlight(rest, terms, Style::default().fg(TEXT)));
        Line::from(spans)
    } else if let Some(rest) = line.strip_prefix("ASSISTANT: ") {
        let mut spans = vec![Span::styled(" ▪ ", Style::default().fg(DIM))];
        spans.extend(highlight(rest, terms, Style::default().fg(TEXT)));
        Line::from(spans)
    } else if line.starts_with("[tools:") {
        Line::from(Span::styled(
            format!("   {}", line),
            Style::default().fg(Color::Yellow),
        ))
    } else {
        let mut spans = vec![Span::raw("   ")];
        spans.extend(highlight(line, terms, Style::default().fg(DIM)));
        Line::from(spans)
    }
}

fn command_line(cmd: &Command, terms: &[String]) -> Line<'static> {
    let failed = cmd.exit_code.is_some_and(|code| code != 0);
    let time = chrono::DateTime::from_timestamp_millis(cmd.timestamp)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "?".to_string());

    let mut spans = vec![
        Span::styled(
            format!(" {} ", if failed { "✗" } else { "✓" }),
            Style::default().fg(if failed { Color::Red } else { Color::Green }),
        ),
        Span::styled(format!("{} ", time), Style::default().fg(DIM)),
        Span::styled(
            format!("{:>7} ", duration(cmd.duration_ms)),
            Style::default().fg(DIM),
        ),
    ];
    spans.extend(highlight(
        &cmd.command_text,
        terms,
        Style::default().fg(if failed { Color::Red } else { TEXT }),
    ));
    Line::from(spans)
}

// ─── Search bar and status ──────────────────────────────────

fn render_search(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Search;
    let right = format!("[{}]  [{}]", app.mode_label(), app.sort.label());
    let right_width = right.chars().count() as u16 + 2;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { DIM }));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(right_width)])
        .split(inner);

    // Draw the caret at the cursor position so editing mid-query is visible.
    let (before, after) = app.input.split_at(app.cursor.min(app.input.len()));
    let mut spans = vec![
        Span::styled(" search> ", Style::default().fg(DIM)),
        Span::styled(before.to_string(), Style::default().fg(TEXT)),
    ];
    if focused {
        spans.push(Span::styled(
            "│",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(after.to_string(), Style::default().fg(TEXT)));
    if app.input.is_empty() && focused {
        spans.push(Span::styled(
            "  type to filter conversations and commands",
            Style::default().fg(DIM),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), columns[0]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            right,
            Style::default().fg(if focused { ACCENT } else { DIM }),
        )))
        .alignment(Alignment::Right),
        columns[1],
    );
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(ACCENT));
    let text = |t: &'static str| Span::styled(t, Style::default().fg(DIM));

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(30)])
        .split(area);

    let mut spans = vec![
        key(" Tab"),
        text(" focus  "),
        key("↑/↓"),
        text(" move  "),
        key("Enter"),
        text(" resume  "),
        key("⇧←→"),
        text(" source  "),
        key("Space"),
        text(" fold  "),
        key("^G"),
        text(" group  "),
        key("^O"),
        text(" sort  "),
        key("^R"),
        text(" refresh  "),
        key("F1"),
        text(" help"),
    ];
    if app.focus != Focus::Search {
        spans.push(text("  ·  "));
        spans.push(key("/"));
        spans.push(text(" search"));
    }

    // A refresh note replaces the key hints until the next keystroke.
    let left = match &app.status {
        Some(note) => Line::from(vec![
            Span::styled(" ● ", Style::default().fg(ACCENT)),
            Span::styled(note.clone(), Style::default().fg(TEXT)),
        ]),
        None => Line::from(spans),
    };
    frame.render_widget(Paragraph::new(left), columns[0]);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(app.filter_label(), Style::default().fg(TEXT)),
            Span::styled(
                format!("  ·  {} focus ", app.focus.label()),
                Style::default().fg(DIM),
            ),
        ]))
        .alignment(Alignment::Right),
        columns[1],
    );
}

// ─── Overlays ───────────────────────────────────────────────

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        (area.width.saturating_sub(width)) / 2,
        (area.height.saturating_sub(height)) / 2,
        width,
        height,
    )
}

fn render_help_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let entries: Vec<(&str, &str)> = vec![
        ("", "MOVING AROUND"),
        ("Tab / Shift+Tab", "cycle focus: search → list → content"),
        ("↑ / ↓", "move the selection, or scroll the content pane"),
        ("j / k", "same, when the list or content has focus"),
        ("g / G", "jump to the first or last row"),
        ("PageUp / PageDown", "scroll the content pane a screen at a time"),
        ("/", "jump back to the search box"),
        ("", ""),
        ("", "FINDING"),
        ("(type)", "filter agent conversations and shell commands together"),
        ("Ctrl+U", "clear the query"),
        ("Shift+← / Shift+→", "switch source tab: All / Claude Code / Codex / Shell"),
        ("1 2 3 4", "jump straight to a source tab (outside the search box)"),
        ("Ctrl+S", "next source tab"),
        ("Ctrl+G", "group by source, or show one flat newest-first list"),
        ("Ctrl+O", "sort by newest or by best match"),
        ("Ctrl+R", "rescan transcripts for sessions written since recall opened"),
        ("Esc", "clear the query, then quit"),
        ("", ""),
        ("", "ACTING"),
        ("Enter", "on a session: resume it — on a group header: show only that source"),
        ("Space", "fold or unfold the selected group"),
        ("r", "resume the selected agent session"),
        ("F1 / ?", "this help"),
        ("Ctrl+C", "quit"),
        ("", ""),
        ("", "NOTES"),
        ("", "Full-text search falls back to substring matching automatically."),
        ("", "Resuming hands the terminal to claude or codex, then exits."),
    ];

    let lines: Vec<Line> = entries
        .iter()
        .skip(app.help_scroll)
        .map(|(key, description)| {
            if key.is_empty() && description.is_empty() {
                Line::raw("")
            } else if key.is_empty() {
                Line::from(Span::styled(
                    format!(" {}", description),
                    Style::default().fg(ACCENT).bold(),
                ))
            } else {
                Line::from(vec![
                    Span::styled(
                        format!("  {:<20}", key),
                        Style::default().fg(TEXT),
                    ),
                    Span::styled(description.to_string(), Style::default().fg(DIM)),
                ])
            }
        })
        .collect();

    let popup = centered(area, 84, entries.len() as u16 + 3);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(ACCENT))
                    .title(" Keys ")
                    .title_style(Style::default().fg(ACCENT).bold())
                    .title_bottom(Line::from(" Esc to close ").right_aligned()),
            ),
        popup,
    );
}

fn render_resume_dialog(frame: &mut Frame, area: Rect, app: &App) {
    let dialog = match &app.resume_dialog {
        Some(dialog) => dialog,
        None => return,
    };

    let session = &dialog.session;
    let kind = match session.source {
        crate::ai::models::Source::Claude => Kind::Claude,
        crate::ai::models::Source::Codex => Kind::Codex,
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" {} ", kind.label()),
                Style::default().fg(kind.color()).bold(),
            ),
            Span::styled("· ", Style::default().fg(DIM)),
            Span::styled(
                format!("{}  ({} ago)", full_date(session.started_at), ago(session.last_activity)),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(Span::styled(
            format!(
                " {}",
                truncate(
                    &session
                        .title
                        .as_deref()
                        .unwrap_or("(untitled)")
                        .replace('\n', " "),
                    68
                )
            ),
            Style::default().fg(TEXT),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled(" $ ", Style::default().fg(DIM)),
            Span::styled(dialog.command.display(), Style::default().fg(ACCENT)),
        ]),
        Line::from(vec![
            Span::styled("   in ", Style::default().fg(DIM)),
            Span::styled(truncate(&dialog.command.cwd, 62), Style::default().fg(TEXT)),
        ]),
    ];

    if dialog.dir_missing {
        lines.push(Line::from(Span::styled(
            "   that directory no longer exists",
            Style::default().fg(Color::Red),
        )));
    }

    lines.push(Line::raw(""));

    let option = |label: &'static str, selected: bool, danger: bool| {
        let color = match (selected, danger) {
            (true, true) => Color::Red,
            (true, false) => ACCENT,
            (false, _) => DIM,
        };
        let mut style = Style::default().fg(color);
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        vec![
            Span::styled(if selected { " ▸ " } else { "   " }, style),
            Span::styled(label, style),
        ]
    };

    let mut choice = vec![Span::styled(" Resume?   ", Style::default().fg(TEXT))];
    choice.extend(option("Yes", dialog.confirmed, false));
    choice.extend(option("No", !dialog.confirmed, true));
    lines.push(Line::from(choice));

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        " y / Enter confirm    n / Esc cancel",
        Style::default().fg(DIM),
    )));

    let popup = centered(area, 76, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(ACCENT))
                .title(" Resume session ")
                .title_style(Style::default().fg(ACCENT).bold()),
        ),
        popup,
    );
}

// ─── Text helpers ───────────────────────────────────────────

/// Split `text` so every occurrence of a query term is styled as a match.
fn highlight(text: &str, terms: &[String], base: Style) -> Vec<Span<'static>> {
    if terms.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }

    let lower = text.to_lowercase();
    let mut marks = vec![false; text.chars().count()];

    for term in terms {
        let mut from = 0;
        while let Some(found) = lower[from..].find(term.as_str()) {
            let start = lower[..from + found].chars().count();
            for mark in marks.iter_mut().skip(start).take(term.chars().count()) {
                *mark = true;
            }
            from += found + term.len();
        }
    }

    // Matches are underlined in the terminal's own yellow. Underline carries
    // the mark; the colour is a hint, so it stays readable even where yellow
    // has poor contrast against the user's background.
    let matched = base.fg(HIGHLIGHT).add_modifier(Modifier::UNDERLINED);
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut current = marks.first().copied().unwrap_or(false);

    for (ch, is_match) in text.chars().zip(marks.iter().copied()) {
        if is_match != current {
            spans.push(Span::styled(
                std::mem::take(&mut buffer),
                if current { matched } else { base },
            ));
            current = is_match;
        }
        buffer.push(ch);
    }
    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, if current { matched } else { base }));
    }

    spans
}

/// Rough relative time, e.g. "2h ago" — easier to place than a date.
fn ago(millis: i64) -> String {
    let minutes = (chrono::Utc::now().timestamp_millis() - millis).max(0) / 60_000;
    if minutes < 60 {
        format!("{}m", minutes.max(1))
    } else if minutes < 60 * 24 {
        format!("{}h", minutes / 60)
    } else if minutes < 60 * 24 * 365 {
        format!("{}d", minutes / (60 * 24))
    } else {
        format!("{}y", minutes / (60 * 24 * 365))
    }
}

fn duration(ms: Option<i64>) -> String {
    match ms {
        Some(d) if d >= 60_000 => format!("{}m{}s", d / 60_000, (d % 60_000) / 1000),
        Some(d) if d >= 1000 => format!("{}.{}s", d / 1000, (d % 1000) / 100),
        Some(d) => format!("{}ms", d),
        None => "-".to_string(),
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn full_date(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "?".to_string())
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
    fn highlight_marks_every_occurrence() {
        let spans = highlight("retry the retry logic", &["retry".to_string()], Style::default());
        let matched: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(matched, vec!["retry", "retry"]);
    }

    #[test]
    fn highlight_keeps_the_weight_of_the_row_it_sits_on() {
        let base = Style::default().add_modifier(Modifier::BOLD);
        let spans = highlight("retry", &["retry".to_string()], base);
        let matched = spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .expect("the match is underlined");
        assert!(matched.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn the_selection_outline_carries_no_colour() {
        let left = edge(true, EDGE_LEFT);
        assert_eq!(left.content.as_ref(), EDGE_LEFT);
        assert_eq!(left.style.fg, None, "the outline uses the terminal's own foreground");
        assert_eq!(left.style.bg, None);
        assert_eq!(edge(false, EDGE_LEFT).content.as_ref(), " ");
    }

    #[test]
    fn nothing_in_a_row_sets_a_background() {
        // A filled row buries text on whichever theme we guessed wrong about.
        for selected in [true, false] {
            for focused in [true, false] {
                assert_eq!(row_style(selected, focused).bg, None);
            }
        }
    }

    #[test]
    fn highlight_is_case_insensitive() {
        let spans = highlight("Docker Compose", &["docker".to_string()], Style::default());
        assert!(spans.iter().any(|s| s.content.as_ref() == "Docker"));
    }

    #[test]
    fn highlight_without_terms_returns_one_span() {
        let spans = highlight("plain text", &[], Style::default());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "plain text");
    }

    #[test]
    fn highlight_preserves_the_whole_string() {
        let text = "fix the flaky retry test";
        let spans = highlight(text, &["retry".to_string(), "flaky".to_string()], Style::default());
        let rebuilt: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn ago_reads_as_relative_time() {
        let now = chrono::Utc::now().timestamp_millis();
        assert_eq!(ago(now), "1m");
        assert_eq!(ago(now - 90 * 60_000), "1h");
        assert_eq!(ago(now - 3 * 24 * 60 * 60_000), "3d");
    }

    #[test]
    fn truncate_marks_elided_text() {
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
        assert_eq!(truncate("short", 10), "short");
    }
}
