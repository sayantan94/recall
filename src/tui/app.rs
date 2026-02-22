use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use rusqlite::Connection;
use std::io::stdout;

use crate::db::models::{Command, Session};
use crate::db::queries;

use super::detail;
use super::search;
use super::timeline;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Timeline,
    Detail,
    Search,
}

pub struct SessionInfo {
    pub session: Session,
    pub command_count: usize,
    pub repos: Vec<String>,
    pub has_failures: bool,
}

pub struct App {
    pub view: View,
    pub session_infos: Vec<SessionInfo>,
    pub selected_session: usize,
    pub scroll_offset: usize,
    pub session_commands: Vec<Command>,
    pub selected_command: usize,
    pub cmd_scroll_offset: usize,
    pub search_input: String,
    pub search_results: Vec<Command>,
    pub search_selected: usize,
    pub search_scroll_offset: usize,
    pub total_commands: usize,
    pub should_quit: bool,
}

impl App {
    pub fn new(conn: &Connection) -> Result<Self> {
        let sessions = queries::get_sessions(conn, 200, 0)?;
        let total_commands = queries::get_all_commands(conn, 1_000_000)?
            .len();

        let mut session_infos = Vec::with_capacity(sessions.len());
        for session in sessions {
            let cmds = queries::get_session_commands(conn, &session.id)?;
            let has_failures = cmds.iter().any(|c| c.exit_code.is_some_and(|code| code != 0));
            let mut repos: Vec<String> = cmds
                .iter()
                .filter_map(|c| c.git_repo.clone())
                .collect();
            repos.sort();
            repos.dedup();
            session_infos.push(SessionInfo {
                command_count: cmds.len(),
                session,
                repos,
                has_failures,
            });
        }

        Ok(Self {
            view: View::Timeline,
            session_infos,
            selected_session: 0,
            scroll_offset: 0,
            session_commands: Vec::new(),
            selected_command: 0,
            cmd_scroll_offset: 0,
            search_input: String::new(),
            search_results: Vec::new(),
            search_selected: 0,
            search_scroll_offset: 0,
            total_commands,
            should_quit: false,
        })
    }

    pub fn visible_height(&self, frame_height: u16) -> usize {
        frame_height.saturating_sub(6) as usize
    }

    pub fn handle_key(&mut self, key: KeyCode, conn: &Connection, frame_height: u16) -> Result<()> {
        let visible = self.visible_height(frame_height);

        match self.view {
            View::Timeline => match key {
                KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Char('/') => {
                    self.search_input.clear();
                    self.search_results.clear();
                    self.search_selected = 0;
                    self.search_scroll_offset = 0;
                    self.view = View::Search;
                }
                KeyCode::Up => {
                    if self.selected_session > 0 {
                        self.selected_session -= 1;
                        ensure_visible(self.selected_session, &mut self.scroll_offset, visible);
                    }
                }
                KeyCode::Down => {
                    if self.selected_session + 1 < self.session_infos.len() {
                        self.selected_session += 1;
                        ensure_visible(self.selected_session, &mut self.scroll_offset, visible);
                    }
                }
                KeyCode::Enter => {
                    if let Some(info) = self.session_infos.get(self.selected_session) {
                        self.session_commands =
                            queries::get_session_commands(conn, &info.session.id)?;
                        self.selected_command = 0;
                        self.cmd_scroll_offset = 0;
                        self.view = View::Detail;
                    }
                }
                _ => {}
            },
            View::Detail => match key {
                KeyCode::Esc | KeyCode::Char('q') => self.view = View::Timeline,
                KeyCode::Up => {
                    if self.selected_command > 0 {
                        self.selected_command -= 1;
                        ensure_visible(self.selected_command, &mut self.cmd_scroll_offset, visible);
                    }
                }
                KeyCode::Down => {
                    if self.selected_command + 1 < self.session_commands.len() {
                        self.selected_command += 1;
                        ensure_visible(self.selected_command, &mut self.cmd_scroll_offset, visible);
                    }
                }
                _ => {}
            },
            View::Search => match key {
                KeyCode::Esc => self.view = View::Timeline,
                KeyCode::Enter => {
                    if !self.search_input.is_empty() {
                        let results =
                            queries::search_commands(conn, &self.search_input, 100)?;
                        self.search_results =
                            results.into_iter().map(|r| r.command).collect();
                        self.search_selected = 0;
                        self.search_scroll_offset = 0;
                    }
                }
                KeyCode::Backspace => {
                    self.search_input.pop();
                }
                KeyCode::Char(c) => {
                    self.search_input.push(c);
                }
                KeyCode::Up => {
                    if self.search_selected > 0 {
                        self.search_selected -= 1;
                        ensure_visible(self.search_selected, &mut self.search_scroll_offset, visible.saturating_sub(3));
                    }
                }
                KeyCode::Down => {
                    if self.search_selected + 1 < self.search_results.len() {
                        self.search_selected += 1;
                        ensure_visible(self.search_selected, &mut self.search_scroll_offset, visible.saturating_sub(3));
                    }
                }
                _ => {}
            },
        }
        Ok(())
    }
}

fn ensure_visible(selected: usize, scroll: &mut usize, visible: usize) {
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + visible {
        *scroll = selected.saturating_sub(visible.saturating_sub(1));
    }
}

pub fn run_tui() -> Result<()> {
    let conn = crate::db::schema::open_db()?;
    let mut app = App::new(&conn)?;

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    loop {
        let frame_height = terminal.size()?.height;
        terminal.draw(|frame| match app.view {
            View::Timeline => timeline::render(frame, &app),
            View::Detail => detail::render(frame, &app),
            View::Search => search::render(frame, &app),
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                app.handle_key(key.code, &conn, frame_height)?;
                if app.should_quit {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}
