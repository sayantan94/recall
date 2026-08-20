//! recall's TUI: one unified split-pane home.
//!
//! Agent sessions and shell sessions share a single list on the left, grouped
//! by source, with the selected entry's detail and content rendered live on the
//! right. There is no drill-in screen — scanning and reading happen together.

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use rusqlite::Connection;
use std::io::stdout;
use std::time::Duration;

use crate::ai::indexer;
use crate::ai::models::{AiSession, Source};
use crate::ai::resume::{self, CommandSpec};
use crate::ai::search as ai_search;
use crate::ai::store::{self as ai_store, Filter};
use crate::db::models::{Command, Session};
use crate::db::queries;

use super::home;

/// How many entries the list holds before older ones are dropped.
const LIST_LIMIT: usize = 300;
/// Shell sessions scanned when browsing.
const SHELL_SESSION_LIMIT: usize = 200;
/// Per-source budget, so no one tool can crowd the others out of the list.
const SOURCE_LIMIT: usize = 120;

// ─── Data ───────────────────────────────────────────────────

/// What a list row can be. Agent sessions and shell sessions live side by side.
#[derive(Debug, Clone)]
pub enum Entry {
    Agent {
        session: AiSession,
        snippet: String,
        rank: f64,
    },
    Shell {
        session: Session,
        command_count: usize,
        failures: usize,
        repos: Vec<String>,
        snippet: String,
    },
}

impl Entry {
    pub fn kind(&self) -> Kind {
        match self {
            Entry::Agent { session, .. } => match session.source {
                Source::Claude => Kind::Claude,
                Source::Codex => Kind::Codex,
            },
            Entry::Shell { .. } => Kind::Shell,
        }
    }

    pub fn last_activity(&self) -> i64 {
        match self {
            Entry::Agent { session, .. } => session.last_activity,
            Entry::Shell { session, .. } => session.end_time.unwrap_or(session.start_time),
        }
    }

    /// Lower is a better match. Shell hits have no BM25 score of their own, so
    /// they sort after agent hits when ranking by relevance.
    pub fn rank(&self) -> f64 {
        match self {
            Entry::Agent { rank, .. } => *rank,
            Entry::Shell { .. } => 0.0,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Entry::Agent { session, .. } => session
                .title
                .as_deref()
                .map(|t| t.replace('\n', " "))
                .unwrap_or_else(|| "(untitled)".to_string()),
            Entry::Shell {
                session,
                command_count,
                repos,
                ..
            } => {
                let dir = session
                    .initial_dir
                    .as_deref()
                    .and_then(|d| d.rsplit('/').next())
                    .unwrap_or("~");
                if repos.is_empty() {
                    format!("{}  ({} commands)", dir, command_count)
                } else {
                    format!("{}  {}", dir, repos.join(", "))
                }
            }
        }
    }

    pub fn snippet(&self) -> &str {
        match self {
            Entry::Agent { snippet, .. } | Entry::Shell { snippet, .. } => snippet,
        }
    }
}

/// The kinds a row can have, which is also the filter and grouping order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Claude,
    Codex,
    Shell,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Claude, Kind::Codex, Kind::Shell];

    pub fn tag(&self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
            Kind::Shell => "shell",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Kind::Claude => "Claude Code",
            Kind::Codex => "Codex",
            Kind::Shell => "Shell",
        }
    }

    /// Named ANSI colours, so each source stays distinguishable under whatever
    /// palette the user's terminal already uses.
    pub fn color(&self) -> Color {
        match self {
            Kind::Claude => Color::Magenta,
            Kind::Codex => Color::Blue,
            Kind::Shell => Color::Green,
        }
    }
}

/// A rendered list row. Headers are labels only — navigation skips them.
#[derive(Debug, Clone)]
pub enum Row {
    Header { kind: Kind, count: usize },
    Item(usize),
}

/// What fills the content half of the right pane.
#[derive(Debug, Clone)]
pub enum Preview {
    Transcript(Vec<String>),
    Commands(Vec<Command>),
    Empty,
}

// ─── Modes ──────────────────────────────────────────────────

/// Which region takes keystrokes. Search has focus on open, so typing filters
/// immediately; moving focus frees single letters to act as shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Search,
    List,
    Content,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Focus::Search => Focus::List,
            Focus::List => Focus::Content,
            Focus::Content => Focus::Search,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Focus::Search => "search",
            Focus::List => "list",
            Focus::Content => "content",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Newest,
    BestMatch,
}

impl Sort {
    fn toggled(self) -> Self {
        match self {
            Sort::Newest => Sort::BestMatch,
            Sort::BestMatch => Sort::Newest,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Sort::Newest => "Newest",
            Sort::BestMatch => "Best match",
        }
    }
}

/// State of the resume confirmation. A session is always reopened in its own
/// directory — that is where the work was — so the only question is whether to
/// go ahead.
#[derive(Debug, Clone)]
pub struct ResumeDialog {
    pub session: AiSession,
    pub command: CommandSpec,
    pub confirmed: bool,
    /// Surfaced as a warning: the project directory no longer exists.
    pub dir_missing: bool,
}

// ─── App ────────────────────────────────────────────────────

/// Aggregate facts about one source group.
#[derive(Debug, Clone)]
pub struct GroupSummary {
    pub count: usize,
    pub projects: usize,
    pub newest: Option<i64>,
    pub oldest: Option<i64>,
    pub recent_titles: Vec<String>,
}

pub struct App {
    /// Every match, regardless of the active tab. Kept so switching tabs is a
    /// re-filter rather than a re-query, and so tab counts stay honest.
    all_entries: Vec<Entry>,
    pub entries: Vec<Entry>,
    pub rows: Vec<Row>,
    pub selected_row: usize,
    pub scroll_offset: usize,
    pub input: String,
    pub cursor: usize,
    pub kind_filter: Option<Kind>,
    pub grouped: bool,
    /// Groups the user has folded shut.
    pub collapsed: std::collections::HashSet<Kind>,
    pub sort: Sort,
    pub mode: ai_search::Mode,
    pub focus: Focus,
    pub preview: Preview,
    pub preview_scroll: usize,
    pub counts: Vec<(Kind, usize)>,
    pub total_commands: usize,
    pub total_agent_sessions: usize,
    /// Transient note shown in the status bar, e.g. after a manual refresh.
    pub status: Option<String>,
    pub show_help: bool,
    pub help_scroll: usize,
    pub resume_dialog: Option<ResumeDialog>,
    pub should_quit: bool,
    /// Set when the user confirms a resume: the TUI exits and hands over the terminal.
    pub pending_resume: Option<CommandSpec>,
}

impl App {
    pub fn new(conn: &Connection) -> Result<Self> {
        let mut app = Self {
            all_entries: Vec::new(),
            entries: Vec::new(),
            rows: Vec::new(),
            selected_row: 0,
            scroll_offset: 0,
            input: String::new(),
            cursor: 0,
            kind_filter: None,
            grouped: true,
            collapsed: std::collections::HashSet::new(),
            sort: Sort::Newest,
            mode: ai_search::Mode::Fts,
            focus: Focus::Search,
            preview: Preview::Empty,
            preview_scroll: 0,
            counts: Vec::new(),
            total_commands: queries::get_all_commands(conn, 1_000_000)?.len(),
            total_agent_sessions: ai_store::stats(conn)?.sessions,
            status: None,
            show_help: false,
            help_scroll: 0,
            resume_dialog: None,
            should_quit: false,
            pending_resume: None,
        };
        app.refresh(conn)?;
        Ok(app)
    }

    pub fn visible_height(&self, frame_height: u16) -> usize {
        frame_height.saturating_sub(8) as usize
    }

    // ─── Key handling ───────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent, conn: &Connection, frame_height: u16) -> Result<()> {
        let visible = self.visible_height(frame_height).max(1);
        self.status = None;

        // Overlays swallow every key while they are up.
        if self.resume_dialog.is_some() {
            return self.handle_resume_dialog_key(key);
        }
        if self.show_help {
            match key.code {
                KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?') | KeyCode::Char('q') => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                _ => {}
            }
            return Ok(());
        }

        // Bindings that work from any focus.
        match key.code {
            KeyCode::F(1) => {
                self.show_help = true;
                return Ok(());
            }
            KeyCode::Tab => {
                self.focus = self.focus.next();
                return Ok(());
            }
            KeyCode::BackTab => {
                self.focus = self.focus.next().next();
                return Ok(());
            }
            KeyCode::Char('c') | KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.should_quit = true;
                return Ok(());
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return self.cycle_tab(conn, 1);
            }
            // Shift+arrows switch tabs from any focus, including mid-query.
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                return self.cycle_tab(conn, 1);
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                return self.cycle_tab(conn, -1);
            }
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.grouped = !self.grouped;
                return self.rebuild_rows(conn);
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // reindex() sets the status note, so it must run after the clear.
                return self.reindex(conn);
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.sort = self.sort.toggled();
                return self.refresh(conn);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
                self.cursor = 0;
                return self.refresh(conn);
            }
            KeyCode::Enter => {
                // On a group header, Enter drills into that source instead.
                if let Some((kind, _)) = self.selected_group() {
                    let index = Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0) + 1;
                    return self.select_tab(conn, index);
                }
                self.open_resume_dialog();
                return Ok(());
            }
            KeyCode::Char(' ') if self.focus != Focus::Search => {
                if let Some((kind, _)) = self.selected_group() {
                    if !self.collapsed.remove(&kind) {
                        self.collapsed.insert(kind);
                    }
                    return self.rebuild_rows(conn);
                }
                return Ok(());
            }
            KeyCode::PageUp => {
                self.preview_scroll = self.preview_scroll.saturating_sub(visible);
                return Ok(());
            }
            KeyCode::PageDown => {
                self.preview_scroll = (self.preview_scroll + visible).min(self.preview_len());
                return Ok(());
            }
            KeyCode::Esc => {
                // Esc unwinds: clear the filter, then leave.
                if self.input.is_empty() {
                    self.should_quit = true;
                } else {
                    self.input.clear();
                    self.cursor = 0;
                    return self.refresh(conn);
                }
                return Ok(());
            }
            _ => {}
        }

        match self.focus {
            Focus::Search => self.handle_search_key(key, conn, visible),
            Focus::List => self.handle_list_key(key, conn, visible),
            Focus::Content => self.handle_content_key(key, conn, visible),
        }
    }

    fn handle_search_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        visible: usize,
    ) -> Result<()> {
        match key.code {
            // Anything still carrying Ctrl/Alt is not text — usually the tail of
            // a terminal escape sequence, never something to type.
            KeyCode::Char(_)
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {}
            KeyCode::Char(c) if !c.is_control() => {
                self.input.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                return self.refresh(conn);
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let previous = self.input[..self.cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.input.remove(previous);
                self.cursor = previous;
                return self.refresh(conn);
            }
            KeyCode::Left => {
                self.cursor = self.input[..self.cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
            }
            KeyCode::Right => {
                self.cursor = self.input[self.cursor..]
                    .char_indices()
                    .nth(1)
                    .map(|(i, _)| self.cursor + i)
                    .unwrap_or(self.input.len());
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            // Arrows still drive the list, so you can filter and pick without
            // ever leaving the search box.
            KeyCode::Up => self.select(conn, -1, visible)?,
            KeyCode::Down => self.select(conn, 1, visible)?,
            _ => {}
        }
        Ok(())
    }

    fn handle_list_key(&mut self, key: KeyEvent, conn: &Connection, visible: usize) -> Result<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.select(conn, -1, visible)?,
            KeyCode::Down | KeyCode::Char('j') => self.select(conn, 1, visible)?,
            KeyCode::Char('g') | KeyCode::Home => self.select_to(conn, 0, visible)?,
            KeyCode::Char('G') | KeyCode::End => {
                self.select_to(conn, self.rows.len().saturating_sub(1), visible)?
            }
            KeyCode::Char(c @ '1'..='4') => {
                return self.select_tab(conn, c as usize - '1' as usize);
            }
            KeyCode::Char('/') => self.focus = Focus::Search,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('r') => self.open_resume_dialog(),
            _ => {}
        }
        Ok(())
    }

    fn handle_content_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        visible: usize,
    ) -> Result<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.preview_scroll = (self.preview_scroll + 1).min(self.preview_len())
            }
            KeyCode::Char('g') | KeyCode::Home => self.preview_scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.preview_scroll = self.preview_len(),
            KeyCode::Char('u') => self.preview_scroll = self.preview_scroll.saturating_sub(visible),
            KeyCode::Char('d') => {
                self.preview_scroll = (self.preview_scroll + visible).min(self.preview_len())
            }
            KeyCode::Char(c @ '1'..='4') => {
                return self.select_tab(conn, c as usize - '1' as usize);
            }
            KeyCode::Char('/') => self.focus = Focus::Search,
            KeyCode::Char('?') => self.show_help = true,
            _ => {}
        }
        Ok(())
    }

    fn handle_resume_dialog_key(&mut self, key: KeyEvent) -> Result<()> {
        let dialog = match &mut self.resume_dialog {
            Some(dialog) => dialog,
            None => return Ok(()),
        };

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.resume_dialog = None
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => dialog.confirmed = !dialog.confirmed,
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let command = dialog.command.clone();
                self.resume_dialog = None;
                self.pending_resume = Some(command);
                self.should_quit = true;
            }
            KeyCode::Enter => {
                let confirmed = dialog.confirmed;
                let command = dialog.command.clone();
                self.resume_dialog = None;
                if confirmed {
                    self.pending_resume = Some(command);
                    self.should_quit = true;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Only agent sessions can be resumed; shell sessions have nothing to reopen.
    fn open_resume_dialog(&mut self) {
        if let Some(Entry::Agent { session, .. }) = self.selected_entry() {
            let session = session.clone();
            self.resume_dialog = Some(ResumeDialog {
                command: resume::resume_command(&session, None),
                dir_missing: !std::path::Path::new(&session.project).is_dir(),
                session,
                confirmed: true,
            });
        }
    }

    // ─── Data loading ───────────────────────────────────────

    fn preview_len(&self) -> usize {
        match &self.preview {
            Preview::Transcript(lines) => lines.len().saturating_sub(1),
            Preview::Commands(commands) => commands.len().saturating_sub(1),
            Preview::Empty => 0,
        }
    }

    /// Rescan the transcripts on disk and rebuild the list. Cheap when nothing
    /// changed, so it also runs once when the TUI opens.
    pub fn reindex(&mut self, conn: &Connection) -> Result<()> {
        let report = indexer::index_all(conn, false)?;
        let changed = report.added + report.updated + report.removed;

        self.total_agent_sessions = ai_store::stats(conn)?.sessions;
        self.total_commands = queries::get_all_commands(conn, 1_000_000)?.len();
        self.status = Some(if changed == 0 {
            "index already up to date".to_string()
        } else {
            format!(
                "indexed {} new · {} updated · {} removed",
                report.added, report.updated, report.removed
            )
        });

        self.refresh(conn)
    }

    /// Re-query every source for the current search, then apply the active tab.
    fn refresh(&mut self, conn: &Connection) -> Result<()> {
        let query = self.input.trim().to_string();

        let mut entries = self.agent_entries(conn, &query)?;
        entries.extend(self.shell_entries(conn, &query)?);

        match self.sort {
            Sort::Newest => entries.sort_by(|a, b| b.last_activity().cmp(&a.last_activity())),
            Sort::BestMatch => entries.sort_by(|a, b| {
                a.rank()
                    .partial_cmp(&b.rank())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.last_activity().cmp(&a.last_activity()))
            }),
        }

        self.counts = Kind::ALL
            .iter()
            .map(|kind| (*kind, entries.iter().filter(|e| e.kind() == *kind).count()))
            .collect();
        self.all_entries = entries;

        self.apply_tab(conn)
    }

    /// Narrow the cached results to the active tab. No database work, so
    /// switching tabs is instant.
    fn apply_tab(&mut self, conn: &Connection) -> Result<()> {
        let mut entries: Vec<Entry> = self
            .all_entries
            .iter()
            .filter(|entry| self.shows(entry.kind()))
            .cloned()
            .collect();

        entries = if self.kind_filter.is_some() {
            entries.into_iter().take(LIST_LIMIT).collect()
        } else {
            reserve_per_kind(entries, LIST_LIMIT)
        };

        self.entries = entries;
        // Land on the newest session, not on the group header above it.
        self.selected_row = 0;
        self.rebuild_rows(conn)?;
        if let Some(first) = self
            .rows
            .iter()
            .position(|row| matches!(row, Row::Item(_)))
        {
            self.selected_row = first;
            return self.load_preview(conn);
        }
        Ok(())
    }

    /// Move `delta` tabs through All → Claude → Codex → Shell → All.
    pub fn cycle_tab(&mut self, conn: &Connection, delta: isize) -> Result<()> {
        let current = self.tab_index() as isize;
        let count = Kind::ALL.len() as isize + 1;
        let next = (current + delta).rem_euclid(count);
        self.select_tab(conn, next as usize)
    }

    /// Tab 0 is "everything"; 1..=3 are the individual kinds.
    pub fn select_tab(&mut self, conn: &Connection, index: usize) -> Result<()> {
        self.kind_filter = if index == 0 {
            None
        } else {
            Kind::ALL.get(index - 1).copied()
        };
        self.apply_tab(conn)
    }

    pub fn tab_index(&self) -> usize {
        match self.kind_filter {
            None => 0,
            Some(kind) => Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0) + 1,
        }
    }

    /// Turn entries into display rows, inserting group headers when grouping is
    /// on. Groups keep the order the sort produced.
    fn rebuild_rows(&mut self, conn: &Connection) -> Result<()> {
        let mut rows = Vec::new();

        // Fixed order rather than "whichever kind has the newest row", so the
        // groups never reshuffle under you — and agent sessions, the reason to
        // open recall, always lead.
        let groups: Vec<(Kind, Vec<usize>)> = Kind::ALL
            .iter()
            .map(|kind| {
                (
                    *kind,
                    self.entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| e.kind() == *kind)
                        .map(|(i, _)| i)
                        .collect::<Vec<_>>(),
                )
            })
            .filter(|(_, members)| !members.is_empty())
            .collect();

        // A header over the only group on screen is noise — the tab already
        // says which source you are looking at.
        if self.grouped && groups.len() > 1 {
            for (kind, members) in groups {
                rows.push(Row::Header {
                    kind,
                    count: members.len(),
                });
                if !self.collapsed.contains(&kind) {
                    rows.extend(members.into_iter().map(Row::Item));
                }
            }
        } else {
            rows.extend((0..self.entries.len()).map(Row::Item));
        }

        self.rows = rows;
        self.selected_row = self.selected_row.min(self.rows.len().saturating_sub(1));
        self.scroll_offset = 0;
        self.load_preview(conn)
    }

    fn shows(&self, kind: Kind) -> bool {
        self.kind_filter.is_none_or(|filter| filter == kind)
    }

    fn agent_entries(&mut self, conn: &Connection, query: &str) -> Result<Vec<Entry>> {
        // Query each source on its own budget. A single combined query would let
        // whichever tool the user leans on bury the other one entirely.
        let mut entries = Vec::new();
        self.mode = ai_search::Mode::Fts;

        for source in [Source::Claude, Source::Codex] {
            let filter = Filter {
                source: Some(source),
                project: None,
                limit: SOURCE_LIMIT,
            };

            if query.is_empty() {
                entries.extend(ai_store::list_sessions(conn, &filter)?.into_iter().map(
                    |session| Entry::Agent {
                        snippet: format!(
                            "{} messages · {}",
                            session.message_count, session.project
                        ),
                        session,
                        rank: 0.0,
                    },
                ));
                continue;
            }

            let (results, mode) = ai_search::search(conn, query, &filter, ai_search::Mode::Fts)?;
            // Report fuzzy as soon as either source needed the fallback.
            if mode == ai_search::Mode::Fuzzy && !results.is_empty() {
                self.mode = mode;
            }
            entries.extend(results.into_iter().map(|result| Entry::Agent {
                snippet: result.snippet.replace('\n', " "),
                rank: result.rank,
                session: result.session,
            }));
        }

        Ok(entries)
    }

    fn shell_entries(&self, conn: &Connection, query: &str) -> Result<Vec<Entry>> {
        // Searching narrows to the sessions whose commands matched; browsing
        // shows the most recent sessions.
        let (sessions, matched) = if query.is_empty() {
            (queries::get_sessions(conn, SHELL_SESSION_LIMIT, 0)?, None)
        } else {
            let mut best: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for hit in queries::search_commands(conn, query, 300)? {
                best.entry(hit.command.session_id.clone())
                    .or_insert(hit.command.command_text);
            }
            let kept: Vec<Session> = queries::get_sessions(conn, 1000, 0)?
                .into_iter()
                .filter(|s| best.contains_key(&s.id))
                .collect();
            (kept, Some(best))
        };

        let mut entries = Vec::new();
        for session in sessions {
            let commands = queries::get_session_commands(conn, &session.id)?;
            if commands.is_empty() {
                continue;
            }

            let failures = commands
                .iter()
                .filter(|c| c.exit_code.is_some_and(|code| code != 0))
                .count();
            let mut repos: Vec<String> = commands
                .iter()
                .filter_map(|c| c.git_repo.clone())
                .map(|r| r.rsplit('/').next().unwrap_or(&r).to_string())
                .collect();
            repos.sort();
            repos.dedup();

            let last = commands.iter().map(|c| c.timestamp).max();
            let snippet = match &matched {
                Some(best) => best.get(&session.id).cloned().unwrap_or_default(),
                None => commands
                    .last()
                    .map(|c| c.command_text.clone())
                    .unwrap_or_default(),
            };

            entries.push(Entry::Shell {
                session: Session {
                    end_time: last.or(session.end_time),
                    ..session
                },
                command_count: commands.len(),
                failures,
                repos,
                snippet,
            });
        }

        Ok(entries)
    }

    // ─── Selection ──────────────────────────────────────────

    /// Move the selection by `delta` rows. Group headers are selectable, so a
    /// source can be folded shut or drilled into like any other row.
    fn select(&mut self, conn: &Connection, delta: isize, visible: usize) -> Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }

        let candidate = self.selected_row as isize + delta;
        if candidate < 0 || candidate >= self.rows.len() as isize {
            return Ok(());
        }

        self.selected_row = candidate as usize;
        ensure_visible(self.selected_row, &mut self.scroll_offset, visible);
        self.load_preview(conn)
    }

    fn select_to(&mut self, conn: &Connection, target: usize, visible: usize) -> Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        self.selected_row = target.min(self.rows.len() - 1);
        ensure_visible(self.selected_row, &mut self.scroll_offset, visible);
        self.load_preview(conn)
    }

    /// The group under the cursor, when a header row is selected.
    pub fn selected_group(&self) -> Option<(Kind, usize)> {
        match self.rows.get(self.selected_row) {
            Some(Row::Header { kind, count }) => Some((*kind, *count)),
            _ => None,
        }
    }

    pub fn is_collapsed(&self, kind: Kind) -> bool {
        self.collapsed.contains(&kind)
    }

    /// Aggregate facts about a group, shown when its header is selected.
    pub fn group_summary(&self, kind: Kind) -> GroupSummary {
        let members: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.kind() == kind)
            .collect();

        let mut projects: Vec<String> = members
            .iter()
            .map(|entry| match entry {
                Entry::Agent { session, .. } => session.project.clone(),
                Entry::Shell { session, .. } => {
                    session.initial_dir.clone().unwrap_or_default()
                }
            })
            .filter(|project| !project.is_empty())
            .collect();
        projects.sort();
        projects.dedup();

        GroupSummary {
            count: members.len(),
            projects: projects.len(),
            newest: members.iter().map(|entry| entry.last_activity()).max(),
            oldest: members.iter().map(|entry| entry.last_activity()).min(),
            recent_titles: members
                .iter()
                .take(4)
                .map(|entry| entry.title())
                .collect(),
        }
    }

    fn load_preview(&mut self, conn: &Connection) -> Result<()> {
        self.preview_scroll = 0;
        self.preview = match self.selected_entry() {
            Some(Entry::Agent { session, .. }) => Preview::Transcript(
                ai_store::session_chunks(conn, &session.uid)?
                    .iter()
                    .flat_map(|chunk| chunk.text.lines().map(str::to_string).collect::<Vec<_>>())
                    .collect(),
            ),
            Some(Entry::Shell { session, .. }) => {
                Preview::Commands(queries::get_session_commands(conn, &session.id)?)
            }
            None => Preview::Empty,
        };
        Ok(())
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        match self.rows.get(self.selected_row) {
            Some(Row::Item(index)) => self.entries.get(*index),
            _ => None,
        }
    }

    /// One-based position among item rows, for the list footer.
    pub fn selected_position(&self) -> usize {
        self.rows
            .iter()
            .take(self.selected_row + 1)
            .filter(|row| matches!(row, Row::Item(_)))
            .count()
    }

    // ─── Labels ─────────────────────────────────────────────

    pub fn filter_label(&self) -> &'static str {
        match self.kind_filter {
            None => "everything",
            Some(kind) => kind.label(),
        }
    }

    pub fn mode_label(&self) -> &'static str {
        if self.input.trim().is_empty() {
            "Browsing"
        } else {
            match self.mode {
                ai_search::Mode::Fts => "Full-text",
                ai_search::Mode::Fuzzy => "Fuzzy",
            }
        }
    }

    /// Whitespace-separated query terms, for highlighting matches.
    pub fn query_terms(&self) -> Vec<String> {
        self.input
            .split_whitespace()
            .filter(|term| term.len() > 1)
            .map(|term| term.to_lowercase())
            .collect()
    }
}

// ─── Helpers ────────────────────────────────────────────────

/// Trim to `limit` while keeping every kind represented. Agent sessions usually
/// outnumber shell sessions, and a plain newest-first cut would drop shell
/// results out of the list entirely.
fn reserve_per_kind(entries: Vec<Entry>, limit: usize) -> Vec<Entry> {
    if entries.len() <= limit {
        return entries;
    }

    let share = (limit / Kind::ALL.len()).max(1);
    let mut kept: Vec<(usize, Entry)> = Vec::new();
    let mut overflow: Vec<(usize, Entry)> = Vec::new();
    let mut taken = std::collections::HashMap::new();

    for (index, entry) in entries.into_iter().enumerate() {
        let count = taken.entry(entry.kind()).or_insert(0usize);
        if *count < share {
            *count += 1;
            kept.push((index, entry));
        } else {
            overflow.push((index, entry));
        }
    }

    let room = limit.saturating_sub(kept.len());
    kept.extend(overflow.into_iter().take(room));
    kept.sort_by_key(|(index, _)| *index);
    kept.into_iter().map(|(_, entry)| entry).collect()
}

fn ensure_visible(selected: usize, scroll: &mut usize, visible: usize) {
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + visible {
        *scroll = selected.saturating_sub(visible.saturating_sub(1));
    }
}

/// Consume the rest of a terminal escape sequence so its bytes never reach the
/// filter. Sequences end at BEL or a string terminator; the bound stops a
/// malformed one from eating real keystrokes.
fn drain_escape_sequence() -> Result<()> {
    const MAX_EVENTS: usize = 64;

    for _ in 0..MAX_EVENTS {
        if !event::poll(Duration::ZERO)? {
            return Ok(());
        }
        if let Event::Key(key) = event::read()? {
            let is_bel =
                key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL);
            if is_bel || key.code == KeyCode::Char('\\') || key.code == KeyCode::Char('\u{7}') {
                return Ok(());
            }
        }
    }

    Ok(())
}

pub fn run_tui() -> Result<()> {
    let conn = crate::db::schema::open_db()?;

    // Pick up transcripts written since the last run. A warm rescan is well
    // under a second; a cold one is only slow on a machine that has never
    // indexed, so say something before it starts.
    if ai_store::stats(&conn)?.sessions == 0 {
        println!("  Indexing Claude Code and Codex transcripts for the first time...");
    }
    indexer::index_all(&conn, false)?;

    let mut app = App::new(&conn)?;

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    loop {
        let frame_height = terminal.size()?.height;
        terminal.draw(|frame| home::render(frame, &app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // A terminal answering a query (OSC 11 background colour, cursor
            // position, …) arrives on stdin as an escape sequence. crossterm
            // reports its first byte either as a bare Esc or, more often, folded
            // into Alt+<char>; the rest arrives as ordinary characters that
            // would otherwise be typed into the filter.
            //
            // A human pressing Esc has nothing queued behind it, so a pending
            // event is the tell.
            let sequence_start = key.modifiers.contains(KeyModifiers::ALT)
                || (key.code == KeyCode::Esc && event::poll(Duration::ZERO)?);
            if sequence_start {
                drain_escape_sequence()?;
                continue;
            }

            app.handle_key(key, &conn, frame_height)?;
            if app.should_quit {
                break;
            }
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    if let Some(spec) = app.pending_resume.take() {
        println!("▶ {}  in {}", spec.display(), spec.cwd);
        resume::exec(&spec)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point the source parsers at an empty directory so tests never scan — or
    /// depend on — the real transcripts in the developer's home.
    fn isolate_transcript_dirs() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let empty = std::env::temp_dir().join("recall-test-empty");
            std::fs::create_dir_all(&empty).ok();
            unsafe {
                std::env::set_var("RECALL_CLAUDE_DIR", &empty);
                std::env::set_var("RECALL_CODEX_DIR", &empty);
            }
        });
    }

    fn test_app() -> (App, Connection) {
        isolate_transcript_dirs();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::schema::initialize_db(&conn).unwrap();
        let app = App::new(&conn).unwrap();
        (app, conn)
    }

    fn press(app: &mut App, conn: &Connection, code: KeyCode, modifiers: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, modifiers), conn, 40).unwrap();
    }

    fn typed(app: &mut App, conn: &Connection, text: &str) {
        for c in text.chars() {
            press(app, conn, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    #[test]
    fn search_has_focus_on_open_so_typing_filters_immediately() {
        let (mut app, conn) = test_app();
        assert_eq!(app.focus, Focus::Search);
        typed(&mut app, &conn, "git");
        assert_eq!(app.input, "git");
    }

    #[test]
    fn escape_sequence_tails_never_reach_the_filter() {
        // A terminal's OSC 11 reply reaches crossterm as Alt+']' followed by
        // plain characters and a Ctrl+G (BEL). None of it is text.
        let (mut app, conn) = test_app();
        typed(&mut app, &conn, "g");

        press(&mut app, &conn, KeyCode::Char(']'), KeyModifiers::ALT);
        press(&mut app, &conn, KeyCode::Char('g'), KeyModifiers::CONTROL);

        assert_eq!(app.input, "g", "only the real keystroke was typed");
        assert!(!app.should_quit);
    }

    #[test]
    fn escape_clears_the_filter_before_quitting() {
        let (mut app, conn) = test_app();
        typed(&mut app, &conn, "x");

        press(&mut app, &conn, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.input, "");
        assert!(!app.should_quit, "first Esc only clears the filter");

        press(&mut app, &conn, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.should_quit, "second Esc leaves");
    }

    #[test]
    fn tab_cycles_focus_and_frees_letters_for_shortcuts() {
        let (mut app, conn) = test_app();
        press(&mut app, &conn, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::List);

        // In list focus, 'j' navigates rather than typing.
        press(&mut app, &conn, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.input, "");

        press(&mut app, &conn, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::Content);
        press(&mut app, &conn, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::Search);
    }

    #[test]
    fn slash_returns_focus_to_the_search_box() {
        let (mut app, conn) = test_app();
        press(&mut app, &conn, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, &conn, KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::Search);
        assert_eq!(app.input, "", "the slash itself is not typed");
    }

    #[test]
    fn ctrl_bindings_do_not_leak_into_the_query() {
        let (mut app, conn) = test_app();
        press(&mut app, &conn, KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(app.kind_filter, Some(Kind::Claude));


        press(&mut app, &conn, KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert!(!app.grouped);

        press(&mut app, &conn, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(app.sort, Sort::BestMatch);

        assert_eq!(app.input, "");
    }

    #[test]
    fn ctrl_u_clears_the_query_from_any_focus() {
        let (mut app, conn) = test_app();
        typed(&mut app, &conn, "docker");
        press(&mut app, &conn, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, &conn, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(app.input, "");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn the_caret_moves_through_the_query() {
        let (mut app, conn) = test_app();
        typed(&mut app, &conn, "abc");
        assert_eq!(app.cursor, 3);

        press(&mut app, &conn, KeyCode::Left, KeyModifiers::NONE);
        press(&mut app, &conn, KeyCode::Char('X'), KeyModifiers::NONE);
        assert_eq!(app.input, "abXc");

        press(&mut app, &conn, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(app.cursor, 0);
        press(&mut app, &conn, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(app.cursor, app.input.len());
    }

    #[test]
    fn help_overlay_swallows_keys_until_dismissed() {
        let (mut app, conn) = test_app();
        press(&mut app, &conn, KeyCode::F(1), KeyModifiers::NONE);
        assert!(app.show_help);

        typed(&mut app, &conn, "xyz");
        assert_eq!(app.input, "", "typing goes nowhere while help is up");

        press(&mut app, &conn, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.show_help);
        assert!(!app.should_quit, "closing help does not quit");
    }

    #[test]
    fn the_resume_dialog_defaults_to_yes_and_cancels_cleanly() {
        let (mut app, conn) = test_app();
        // No agent sessions in an empty database, so build the dialog directly.
        app.resume_dialog = Some(ResumeDialog {
            session: agent_session("abc"),
            command: resume::resume_command(&agent_session("abc"), None),
            confirmed: true,
            dir_missing: false,
        });

        press(&mut app, &conn, KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(app.resume_dialog.is_none());
        assert!(app.pending_resume.is_none());
        assert!(!app.should_quit, "declining stays in the TUI");
    }

    #[test]
    fn confirming_the_resume_queues_the_command_and_exits() {
        let (mut app, conn) = test_app();
        app.resume_dialog = Some(ResumeDialog {
            session: agent_session("abc"),
            command: resume::resume_command(&agent_session("abc"), None),
            confirmed: true,
            dir_missing: false,
        });

        press(&mut app, &conn, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.should_quit);
        let queued = app.pending_resume.expect("a resume command was queued");
        assert_eq!(queued.args, vec!["claude", "--resume", "abc"]);
    }

    #[test]
    fn toggling_to_no_then_confirming_does_nothing() {
        let (mut app, conn) = test_app();
        app.resume_dialog = Some(ResumeDialog {
            session: agent_session("abc"),
            command: resume::resume_command(&agent_session("abc"), None),
            confirmed: true,
            dir_missing: false,
        });

        press(&mut app, &conn, KeyCode::Left, KeyModifiers::NONE);
        press(&mut app, &conn, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.pending_resume.is_none());
        assert!(!app.should_quit);
    }

    fn app_with_groups() -> (App, Connection) {
        let (mut app, conn) = test_app();
        app.entries = vec![
            agent_entry("a1", 900),
            agent_entry("a2", 800),
            codex_entry("c1", 700),
            shell_entry("s1", 600),
        ];
        app.rebuild_rows(&conn).unwrap();
        (app, conn)
    }

    #[test]
    fn the_first_selection_is_a_session_not_a_header() {
        let (mut app, conn) = test_app();
        app.all_entries = vec![agent_entry("a1", 900), shell_entry("s1", 600)];
        app.counts = vec![(Kind::Claude, 1), (Kind::Codex, 0), (Kind::Shell, 1)];
        app.apply_tab(&conn).unwrap();

        assert!(app.selected_group().is_none(), "not parked on a header");
        assert!(app.selected_entry().is_some());
    }

    #[test]
    fn group_headers_are_selectable_rows() {
        let (app, _conn) = app_with_groups();
        assert!(
            matches!(app.rows.first(), Some(Row::Header { kind: Kind::Claude, count: 2 })),
            "the first row is the Claude group header"
        );
        assert_eq!(app.selected_group(), Some((Kind::Claude, 2)));
    }

    #[test]
    fn space_folds_and_unfolds_the_selected_group() {
        let (mut app, conn) = app_with_groups();
        press(&mut app, &conn, KeyCode::Tab, KeyModifiers::NONE);

        let before = app.rows.len();
        press(&mut app, &conn, KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(app.is_collapsed(Kind::Claude));
        assert_eq!(app.rows.len(), before - 2, "the two Claude rows folded away");

        press(&mut app, &conn, KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(!app.is_collapsed(Kind::Claude));
        assert_eq!(app.rows.len(), before);
    }

    #[test]
    fn enter_on_a_group_header_drills_into_that_source() {
        let (mut app, conn) = app_with_groups();
        assert_eq!(app.tab_index(), 0);

        press(&mut app, &conn, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.tab_index(), 1, "Enter on the Claude header selects its tab");
        assert!(app.pending_resume.is_none(), "it does not resume anything");
        assert!(app.resume_dialog.is_none());
    }

    #[test]
    fn a_selected_header_reports_its_group_rather_than_an_entry() {
        let (app, _conn) = app_with_groups();
        assert!(app.selected_entry().is_none());
        let summary = app.group_summary(Kind::Claude);
        assert_eq!(summary.count, 2);
    }

    #[test]
    fn ctrl_r_rescans_and_reports_what_changed() {
        let (mut app, conn) = test_app();
        press(&mut app, &conn, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(
            app.status.as_deref(),
            Some("index already up to date"),
            "an empty machine has nothing new to pick up"
        );
        assert_eq!(app.input, "", "the binding is not typed");
    }

    #[test]
    fn the_status_note_clears_on_the_next_keystroke() {
        let (mut app, conn) = test_app();
        press(&mut app, &conn, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(app.status.is_some());

        typed(&mut app, &conn, "x");
        assert!(app.status.is_none());
    }

    #[test]
    fn tabs_cycle_forwards_and_backwards() {
        let (mut app, conn) = test_app();
        assert_eq!(app.tab_index(), 0);

        for expected in [1, 2, 3, 0] {
            app.cycle_tab(&conn, 1).unwrap();
            assert_eq!(app.tab_index(), expected);
        }

        app.cycle_tab(&conn, -1).unwrap();
        assert_eq!(app.tab_index(), 3, "wraps backwards to the last tab");
    }

    #[test]
    fn shift_arrows_switch_tabs_from_the_search_box() {
        let (mut app, conn) = test_app();
        typed(&mut app, &conn, "git");

        press(&mut app, &conn, KeyCode::Right, KeyModifiers::SHIFT);
        assert_eq!(app.tab_index(), 1);
        assert_eq!(app.input, "git", "the query survives a tab switch");

        press(&mut app, &conn, KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(app.tab_index(), 0);
    }

    #[test]
    fn digits_jump_straight_to_a_tab_outside_the_search_box() {
        let (mut app, conn) = test_app();
        press(&mut app, &conn, KeyCode::Tab, KeyModifiers::NONE);

        press(&mut app, &conn, KeyCode::Char('3'), KeyModifiers::NONE);
        assert_eq!(app.tab_index(), 2, "3 selects the Codex tab");
        assert_eq!(app.input, "", "the digit is not typed");

        press(&mut app, &conn, KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(app.tab_index(), 0);
    }

    #[test]
    fn digits_still_type_inside_the_search_box() {
        let (mut app, conn) = test_app();
        typed(&mut app, &conn, "v2");
        assert_eq!(app.input, "v2");
        assert_eq!(app.tab_index(), 0);
    }

    #[test]
    fn ensure_visible_scrolls_only_when_needed() {
        let mut scroll = 0;
        ensure_visible(3, &mut scroll, 10);
        assert_eq!(scroll, 0, "already on screen");

        ensure_visible(12, &mut scroll, 10);
        assert_eq!(scroll, 3, "scrolls just far enough to show the selection");

        ensure_visible(1, &mut scroll, 10);
        assert_eq!(scroll, 1, "scrolls back up to the selection");
    }

    fn shell_entry(id: &str, at: i64) -> Entry {
        Entry::Shell {
            session: Session {
                id: id.into(),
                start_time: at,
                end_time: Some(at),
                terminal_app: None,
                initial_dir: None,
            },
            command_count: 1,
            failures: 0,
            repos: vec![],
            snippet: String::new(),
        }
    }

    fn agent_session(id: &str) -> AiSession {
        AiSession {
            uid: format!("claude:{}", id),
            source: Source::Claude,
            session_id: id.into(),
            project: "/p".into(),
            title: Some("a session".into()),
            started_at: 0,
            last_activity: 0,
            model: None,
            message_count: 1,
            file_path: "/tmp/x.jsonl".into(),
            file_mtime: 0,
            file_size: 0,
            custom_name: None,
        }
    }

    fn codex_entry(id: &str, at: i64) -> Entry {
        match agent_entry(id, at) {
            Entry::Agent { mut session, snippet, rank } => {
                session.source = Source::Codex;
                session.uid = format!("codex:{}", id);
                Entry::Agent { session, snippet, rank }
            }
            other => other,
        }
    }

    fn agent_entry(id: &str, at: i64) -> Entry {
        Entry::Agent {
            session: AiSession {
                uid: format!("claude:{}", id),
                source: Source::Claude,
                session_id: id.into(),
                project: "/p".into(),
                title: None,
                started_at: at,
                last_activity: at,
                model: None,
                message_count: 1,
                file_path: "/tmp/x.jsonl".into(),
                file_mtime: 0,
                file_size: 0,
                custom_name: None,
            },
            snippet: String::new(),
            rank: 0.0,
        }
    }

    #[test]
    fn reserving_keeps_shell_entries_that_recency_alone_would_drop() {
        let mut entries: Vec<Entry> = (0..20)
            .map(|i| agent_entry(&format!("a{}", i), 10_000 - i))
            .collect();
        entries.extend((0..5).map(|i| shell_entry(&format!("s{}", i), 100 - i)));

        let kept = reserve_per_kind(entries, 12);
        assert_eq!(kept.len(), 12);
        assert!(
            kept.iter().any(|e| e.kind() == Kind::Shell),
            "shell sessions must survive the cut"
        );
    }

    #[test]
    fn reserving_is_a_no_op_below_the_limit() {
        let entries = vec![agent_entry("a", 2), shell_entry("s", 1)];
        assert_eq!(reserve_per_kind(entries, 10).len(), 2);
    }
}
