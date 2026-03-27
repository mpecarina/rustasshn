use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::sshconfig;
use crate::state;
use crate::termio;

pub type ConnectFn = Arc<dyn Fn(&str, bool) -> Command + Send + Sync>;
pub type ActionFn = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;
pub type TiledFn = Arc<dyn Fn(&[String], &str) -> Result<()> + Send + Sync>;
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

pub struct AppConfig {
    pub hosts: Vec<sshconfig::Host>,
    pub store: state::Store,
    pub state_path: PathBuf,
    pub start_in_search: bool,
    pub implicit_select: bool,
    pub enter_mode: String,

    pub in_tmux: fn() -> bool,
    pub add_host: fn(sshconfig::AddHostInput) -> Result<()>,
    pub exec_credential: fn(&str, &str, &str, &str) -> Result<Command>,

    pub connect_in_pane: ConnectFn,
    pub new_window: ActionFn,
    pub split_vert: ActionFn,
    pub split_horiz: ActionFn,
    pub tiled: TiledFn,
    pub setup_logging: LogFn,
}

#[derive(Clone)]
struct Candidate {
    host: sshconfig::Host,
    alias_lc: String,
    hostname_lc: String,
    proxyjump_lc: String,
    search_all_lc: String,
    line: String,
}

#[derive(Default)]
struct AddHostModal {
    field: usize,
    alias: String,
    hostname: String,
    user: String,
    port: String,
    proxyjump: String,
    identity_file: String,
    status: String,
}

#[derive(Default)]
struct CredentialModal {
    action: String,
    host: String,
    field: usize,
    user: String,
    kind: String,
    status: String,
}

struct Model {
    app: AppConfig,
    search: String,
    last_query: String,
    search_focused: bool,
    candidates: Vec<Candidate>,
    filtered: Vec<Candidate>,
    selected: usize,
    scroll: usize,
    selected_aliases: HashSet<String>,
    filter_favorites: bool,
    filter_recents: bool,
    status: String,
    pending_g: bool,
    show_add: bool,
    show_cred: bool,
    add: AddHostModal,
    cred: CredentialModal,
}

pub fn run(app: AppConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    terminal::enable_raw_mode()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut m = Model::new(app);
    let res = loop {
        terminal.draw(|f| {
            let size = f.area();
            m.draw(f, size);
        })?;

        if event::poll(std::time::Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
            && let Some(action) = m.handle_key(k)?
        {
            break action;
        }
    };

    disable_raw_mode().ok();
    let mut stdout = std::io::stdout();
    execute!(stdout, LeaveAlternateScreen).ok();

    match res {
        Action::Quit => Ok(()),
        Action::Exec(mut cmd) => {
            termio::sanitize_stdin_before_exec().ok();
            let status = cmd.status()?;
            if status.success() {
                Ok(())
            } else {
                bail!("command failed")
            }
        }
        Action::ExecWithPause {
            mut cmd,
            success_hint,
        } => {
            termio::sanitize_stdin_before_exec().ok();
            let status = cmd.status()?;
            if !status.success() {
                bail!("command failed")
            }
            eprintln!("{success_hint}");
            eprintln!("press Enter to continue");
            pause_for_enter();
            Ok(())
        }
    }
}

fn pause_for_enter() {
    #[cfg(unix)]
    {
        use std::io::Read;

        if let Ok(mut tty) = std::fs::OpenOptions::new().read(true).open("/dev/tty") {
            let mut buf = [0u8; 1];
            loop {
                match tty.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if buf[0] == b'\n' || buf[0] == b'\r' {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

enum Action {
    Quit,
    Exec(Command),
    ExecWithPause {
        cmd: Command,
        success_hint: &'static str,
    },
}

impl Model {
    fn new(app: AppConfig) -> Self {
        let candidates = build_candidates(&app.hosts);
        let search_focused = app.start_in_search;
        let mut m = Self {
            app,
            search: String::new(),
            last_query: String::new(),
            search_focused,
            candidates,
            filtered: Vec::new(),
            selected: 0,
            scroll: 0,
            selected_aliases: HashSet::new(),
            filter_favorites: false,
            filter_recents: false,
            status: String::new(),
            pending_g: false,
            show_add: false,
            show_cred: false,
            add: AddHostModal::default(),
            cred: CredentialModal::default(),
        };
        m.cred.kind = "password".to_string();
        m.recompute();
        m
    }

    fn recompute(&mut self) {
        let q = self.search.trim().to_lowercase();

        if q != self.last_query {
            self.selected = 0;
            self.scroll = 0;
            self.last_query = q.clone();
        }

        let mut out = Vec::new();
        if q.is_empty() {
            for c in &self.candidates {
                if self.filter_favorites && !self.app.store.is_favorite(&c.host.alias) {
                    continue;
                }
                if self.filter_recents && !self.app.store.recents.iter().any(|r| r == &c.host.alias)
                {
                    continue;
                }
                out.push(c.clone());
            }
            self.filtered = out;
            if self.selected >= self.filtered.len() {
                self.selected = self.filtered.len().saturating_sub(1);
            }
            self.ensure_visible();
            return;
        }

        let mut scored: Vec<(MatchSortKey, Candidate)> = Vec::new();
        for c in &self.candidates {
            if self.filter_favorites && !self.app.store.is_favorite(&c.host.alias) {
                continue;
            }
            if self.filter_recents && !self.app.store.recents.iter().any(|r| r == &c.host.alias) {
                continue;
            }
            if let Some(key) = match_sort_key(&q, c) {
                scored.push((key, c.clone()));
            }
        }
        scored.sort_by(|(a, _), (b, _)| a.cmp(b));
        out.extend(scored.into_iter().map(|(_, c)| c));

        self.filtered = out;
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        self.ensure_visible();
    }

    fn ensure_visible(&mut self) {
        let height = 12usize; // ratatui gets actual height in draw; keep stable for scrolling.
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + height {
            self.scroll = self.selected.saturating_sub(height - 1);
        }
    }

    fn current(&self) -> Option<&Candidate> {
        self.filtered.get(self.selected)
    }

    fn targets(&self) -> Vec<String> {
        if self.selected_aliases.is_empty() {
            return self
                .current()
                .map(|c| vec![c.host.alias.clone()])
                .unwrap_or_default();
        }
        let mut items = Vec::new();
        for c in &self.filtered {
            if self.selected_aliases.contains(&c.host.alias) {
                items.push(c.host.alias.clone());
            }
        }
        if items.is_empty() {
            for a in &self.selected_aliases {
                items.push(a.clone());
            }
        }
        items
    }

    fn handle_key(&mut self, k: KeyEvent) -> Result<Option<Action>> {
        if self.show_add {
            return self.handle_add_key(k);
        }
        if self.show_cred {
            return self.handle_cred_key(k);
        }

        if self.search_focused {
            return self.handle_search_key(k);
        }
        self.handle_normal_key(k)
    }

    fn handle_search_key(&mut self, k: KeyEvent) -> Result<Option<Action>> {
        match (k.code, k.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(Some(Action::Quit)),
            (KeyCode::Esc, _) => {
                self.search_focused = false;
                return Ok(None);
            }
            (KeyCode::Up, _) => {
                self.move_sel(-1);
                return Ok(None);
            }
            (KeyCode::Down, _) => {
                self.move_sel(1);
                return Ok(None);
            }
            (KeyCode::Enter, _) => {
                if self.app.implicit_select {
                    self.search_focused = false;
                    self.recompute();
                    return self.enter_default();
                }
                self.search_focused = false;
                self.recompute();
                return Ok(None);
            }
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.toggle_select_all_filtered();
                return Ok(None);
            }
            (KeyCode::Backspace, _) => {
                self.search.pop();
                self.recompute();
                return Ok(None);
            }
            (KeyCode::Char(ch), KeyModifiers::NONE) => {
                self.search.push(ch);
                self.recompute();
                return Ok(None);
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_normal_key(&mut self, k: KeyEvent) -> Result<Option<Action>> {
        match (k.code, k.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
            | (KeyCode::Char('q'), _)
            | (KeyCode::Esc, _) => return Ok(Some(Action::Quit)),
            (KeyCode::Char('/'), _) => {
                self.search_focused = true;
                return Ok(None);
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                self.move_sel(-1);
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                self.move_sel(1);
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Char('g'), _) => {
                if self.pending_g {
                    self.selected = 0;
                    self.scroll = 0;
                    self.pending_g = false;
                } else {
                    self.pending_g = true;
                }
                return Ok(None);
            }
            (KeyCode::Char('G'), _) => {
                if !self.filtered.is_empty() {
                    self.selected = self.filtered.len() - 1;
                    self.ensure_visible();
                }
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Char(' '), _) => {
                if let Some(c) = self.current() {
                    let a = c.host.alias.clone();
                    if self.selected_aliases.contains(&a) {
                        self.selected_aliases.remove(&a);
                    } else {
                        self.selected_aliases.insert(a);
                    }
                }
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Char('f'), _) => {
                if let Some(c) = self.current() {
                    let alias = c.host.alias.clone();
                    let on = self.app.store.toggle_favorite(&alias);
                    let _ = state::save(&self.app.state_path, &mut self.app.store);
                    self.status = if on {
                        "favorite added"
                    } else {
                        "favorite removed"
                    }
                    .to_string();
                    self.recompute();
                }
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Char('F'), _) => {
                self.filter_favorites = !self.filter_favorites;
                if self.filter_favorites {
                    self.filter_recents = false;
                }
                self.recompute();
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Char('R'), _) => {
                self.filter_recents = !self.filter_recents;
                if self.filter_recents {
                    self.filter_favorites = false;
                }
                self.recompute();
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Char('c'), _) => {
                self.pending_g = false;
                self.open_cred("set");
                return Ok(None);
            }
            (KeyCode::Char('d'), _) => {
                self.pending_g = false;
                self.open_cred("delete");
                return Ok(None);
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.show_add = true;
                self.add = AddHostModal::default();
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.toggle_select_all_filtered();
                self.pending_g = false;
                return Ok(None);
            }
            (KeyCode::Enter, _) => {
                self.pending_g = false;
                return self.enter_default();
            }
            (KeyCode::Char('v'), _) => {
                self.pending_g = false;
                let action = self.app.split_vert.clone();
                return self.run_multi(action, "opened vertical splits");
            }
            (KeyCode::Char('s'), _) => {
                self.pending_g = false;
                let action = self.app.split_horiz.clone();
                return self.run_multi(action, "opened horizontal splits");
            }
            (KeyCode::Char('w'), _) => {
                self.pending_g = false;
                let action = self.app.new_window.clone();
                return self.run_multi(action, "opened tmux windows");
            }
            (KeyCode::Char('t'), _) => {
                self.pending_g = false;
                return self.run_tiled();
            }
            (KeyCode::Char('p'), _) => {
                self.pending_g = false;
                if let Some(c) = self.current() {
                    let alias = c.host.alias.clone();
                    let user = c.host.user.clone();
                    self.app.store.add_recent(&alias);
                    let _ = state::save(&self.app.state_path, &mut self.app.store);
                    (self.app.setup_logging)(&alias);
                    // determine if askpass should be enabled: stored cred exists.
                    let has_cred = crate::credentials::get(&alias, &user, "password").is_ok();
                    let cmd = (self.app.connect_in_pane)(&alias, has_cred);
                    return Ok(Some(Action::Exec(cmd)));
                }
                return Ok(None);
            }
            _ => {
                self.pending_g = false;
            }
        }
        Ok(None)
    }

    fn enter_default(&mut self) -> Result<Option<Action>> {
        if !self.selected_aliases.is_empty() {
            match self.app.enter_mode.as_str() {
                "v" => {
                    let action = self.app.split_vert.clone();
                    return self.run_multi(action, "opened vertical splits");
                }
                "s" => {
                    let action = self.app.split_horiz.clone();
                    return self.run_multi(action, "opened horizontal splits");
                }
                _ => {
                    let action = self.app.new_window.clone();
                    return self.run_multi(action, "opened tmux windows");
                }
            }
        }
        let Some(c) = self.current().cloned() else {
            return Ok(None);
        };
        let alias = c.host.alias.clone();
        let user = c.host.user.clone();
        self.app.store.add_recent(&alias);
        let _ = state::save(&self.app.state_path, &mut self.app.store);
        match self.app.enter_mode.as_str() {
            "w" => {
                let action = self.app.new_window.clone();
                self.run_multi(action, "opened tmux window")
            }
            "v" => {
                let action = self.app.split_vert.clone();
                self.run_multi(action, "opened vertical split")
            }
            "s" => {
                let action = self.app.split_horiz.clone();
                self.run_multi(action, "opened horizontal split")
            }
            _ => {
                (self.app.setup_logging)(&alias);
                let has_cred = crate::credentials::get(&alias, &user, "password").is_ok();
                let cmd = (self.app.connect_in_pane)(&alias, has_cred);
                Ok(Some(Action::Exec(cmd)))
            }
        }
    }

    fn run_tiled(&mut self) -> Result<Option<Action>> {
        let targets = self.targets();
        if targets.is_empty() {
            return Ok(None);
        }
        for a in &targets {
            self.app.store.add_recent(a);
        }
        let _ = state::save(&self.app.state_path, &mut self.app.store);
        if targets.len() == 1 {
            let action = self.app.new_window.clone();
            return self.run_multi(action, "opened tmux window");
        }
        if !(self.app.in_tmux)() {
            self.status = "tiled layout requires running inside tmux".to_string();
            return Ok(None);
        }
        (self.app.tiled)(&targets, "tiled")?;
        Ok(Some(Action::Quit))
    }

    fn run_multi(&mut self, action: ActionFn, _status: &str) -> Result<Option<Action>> {
        let targets = self.targets();
        if targets.is_empty() {
            return Ok(None);
        }
        for a in &targets {
            self.app.store.add_recent(a);
        }
        let _ = state::save(&self.app.state_path, &mut self.app.store);
        if !(self.app.in_tmux)() {
            self.status = "tmux actions require running inside tmux".to_string();
            return Ok(None);
        }
        for a in &targets {
            (action)(a)?;
        }
        Ok(Some(Action::Quit))
    }

    fn open_cred(&mut self, action: &str) {
        let Some(c) = self.current() else { return };
        let alias = c.host.alias.clone();
        let user = c.host.user.clone();
        self.show_cred = true;
        self.cred = CredentialModal::default();
        self.cred.action = action.to_string();
        self.cred.host = alias;
        self.cred.user = user;
        self.cred.kind = "password".to_string();
    }

    fn handle_cred_key(&mut self, k: KeyEvent) -> Result<Option<Action>> {
        match (k.code, k.modifiers) {
            (KeyCode::Esc, _) => {
                self.show_cred = false;
                return Ok(None);
            }
            (KeyCode::Tab, _) | (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                self.cred.field = (self.cred.field + 1) % 2;
                return Ok(None);
            }
            (KeyCode::BackTab, _) | (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                self.cred.field = (self.cred.field + 1) % 2;
                return Ok(None);
            }
            (KeyCode::Enter, _) => {
                let user = self.cred.user.trim().to_string();
                let mut kind = self.cred.kind.trim().to_string();
                if kind.is_empty() {
                    kind = "password".to_string();
                }
                let cmd =
                    (self.app.exec_credential)(&self.cred.action, &self.cred.host, &user, &kind)?;
                self.show_cred = false;
                if self.cred.action == "set" {
                    return Ok(Some(Action::ExecWithPause {
                        cmd,
                        success_hint: "password saved",
                    }));
                }
                return Ok(Some(Action::Exec(cmd)));
            }
            (KeyCode::Backspace, _) => {
                match self.cred.field {
                    0 => {
                        self.cred.user.pop();
                    }
                    1 => {
                        self.cred.kind.pop();
                    }
                    _ => {}
                }
                return Ok(None);
            }
            (KeyCode::Char(ch), KeyModifiers::NONE) => {
                match self.cred.field {
                    0 => self.cred.user.push(ch),
                    1 => self.cred.kind.push(ch),
                    _ => {}
                }
                return Ok(None);
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_add_key(&mut self, k: KeyEvent) -> Result<Option<Action>> {
        match (k.code, k.modifiers) {
            (KeyCode::Esc, _) => {
                self.show_add = false;
                return Ok(None);
            }
            (KeyCode::Tab, _) | (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                self.add.field = (self.add.field + 1) % 6;
                return Ok(None);
            }
            (KeyCode::BackTab, _) | (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                self.add.field = (self.add.field + 5) % 6;
                return Ok(None);
            }
            (KeyCode::Enter, _) => {
                let port = if self.add.port.trim().is_empty() {
                    0
                } else {
                    match self.add.port.trim().parse::<i32>() {
                        Ok(p) if p > 0 => p,
                        _ => {
                            self.add.status = "port must be a positive integer".to_string();
                            return Ok(None);
                        }
                    }
                };
                let input = sshconfig::AddHostInput {
                    alias: self.add.alias.trim().to_string(),
                    hostname: self.add.hostname.trim().to_string(),
                    user: self.add.user.trim().to_string(),
                    port,
                    proxyjump: self.add.proxyjump.trim().to_string(),
                    identity_file: self.add.identity_file.trim().to_string(),
                };
                (self.app.add_host)(input)?;
                self.app.hosts = sshconfig::load_default()?;
                self.candidates = build_candidates(&self.app.hosts);
                self.recompute();
                self.show_add = false;
                self.status = "host added to ~/.ssh/config".to_string();
                return Ok(None);
            }
            (KeyCode::Backspace, _) => {
                self.add_field_mut().pop();
                return Ok(None);
            }
            (KeyCode::Char(ch), KeyModifiers::NONE) => {
                self.add_field_mut().push(ch);
                return Ok(None);
            }
            _ => {}
        }
        Ok(None)
    }

    fn add_field_mut(&mut self) -> &mut String {
        match self.add.field {
            0 => &mut self.add.alias,
            1 => &mut self.add.hostname,
            2 => &mut self.add.user,
            3 => &mut self.add.port,
            4 => &mut self.add.proxyjump,
            _ => &mut self.add.identity_file,
        }
    }

    fn move_sel(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.selected as i32;
        let mut next = cur + delta;
        if next < 0 {
            next = 0;
        }
        let max = self.filtered.len() as i32 - 1;
        if next > max {
            next = max;
        }
        self.selected = next as usize;
        self.ensure_visible();
    }

    fn toggle_select_all_filtered(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let all_selected = self
            .filtered
            .iter()
            .all(|c| self.selected_aliases.contains(&c.host.alias));
        if all_selected {
            for c in &self.filtered {
                self.selected_aliases.remove(&c.host.alias);
            }
        } else {
            for c in &self.filtered {
                self.selected_aliases.insert(c.host.alias.clone());
            }
        }
        self.status = format!("Selected: {}", self.selected_aliases.len());
    }

    fn draw(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(area);

        if self.show_add {
            self.draw_add(f, area);
            return;
        }
        if self.show_cred {
            self.draw_cred(f, area);
            return;
        }

        let search_style = if self.search_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };
        let search = Paragraph::new(format!("/ {}", self.search))
            .style(search_style)
            .block(Block::default());
        f.render_widget(search, chunks[0]);

        let list_height = chunks[1].height as usize;
        let end = std::cmp::min(self.filtered.len(), self.scroll + list_height);
        let mut lines: Vec<Line> = Vec::new();
        for (i, c) in self.filtered.iter().enumerate().take(end).skip(self.scroll) {
            let prefix = if i == self.selected { "> " } else { "  " };
            let sel = if self.selected_aliases.contains(&c.host.alias) {
                "x"
            } else {
                " "
            };
            let star = if self.app.store.is_favorite(&c.host.alias) {
                "*"
            } else {
                " "
            };
            let mut line = Line::from(vec![
                Span::raw(prefix),
                Span::raw("["),
                Span::raw(sel),
                Span::raw("] "),
                Span::styled(star, Style::default().fg(Color::Yellow)),
                Span::raw(" "),
                Span::raw(c.line.clone()),
            ]);
            if i == self.selected {
                line = line.style(Style::default().fg(Color::White).bg(Color::Blue));
            }
            lines.push(line);
        }
        if self.filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "no hosts matched the current filter",
                Style::default().fg(Color::DarkGray),
            )));
        }

        let list = Paragraph::new(lines)
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: true });
        f.render_widget(list, chunks[1]);

        let help = " / search | enter connect | space select | v split-v | s split-h | w window | t tiled | c store cred | d delete cred | f favorite | F favorites | R recents | a add host | q quit ";
        let status = if self.status.is_empty() {
            help.to_string()
        } else {
            format!("{}\n{}", help, self.status)
        };
        let p = Paragraph::new(status).style(Style::default().fg(Color::Gray));
        f.render_widget(p, chunks[2]);
    }

    fn draw_add(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let block = Block::default().title("Add SSH Host").borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);
        let fields = [
            ("Alias", &self.add.alias),
            ("HostName", &self.add.hostname),
            ("User", &self.add.user),
            ("Port", &self.add.port),
            ("ProxyJump", &self.add.proxyjump),
            ("IdentityFile", &self.add.identity_file),
        ];
        for (idx, (label, val)) in fields.iter().enumerate() {
            let style = if idx == self.add.field {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Cyan)
            } else {
                Style::default()
            };
            let p = Paragraph::new(format!("{}: {}", label, val)).style(style);
            f.render_widget(p, rows[idx]);
        }
        let help = "enter save | tab/j/k move | esc cancel";
        let msg = if self.add.status.is_empty() {
            help.to_string()
        } else {
            format!("{}\n{}", help, self.add.status)
        };
        f.render_widget(
            Paragraph::new(msg).style(Style::default().fg(Color::Gray)),
            rows[6],
        );
    }

    fn draw_cred(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let title = if self.cred.action == "delete" {
            "Delete Credential"
        } else {
            "Store Credential"
        };
        let block = Block::default().title(title).borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);
        f.render_widget(Paragraph::new(format!("Host: {}", self.cred.host)), rows[0]);
        let user_style = if self.cred.field == 0 {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan)
        } else {
            Style::default()
        };
        let kind_style = if self.cred.field == 1 {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan)
        } else {
            Style::default()
        };
        f.render_widget(
            Paragraph::new(format!("User: {}", self.cred.user)).style(user_style),
            rows[1],
        );
        f.render_widget(
            Paragraph::new(format!("Kind: {}", self.cred.kind)).style(kind_style),
            rows[2],
        );
        let help = "enter run | tab/j/k move | esc cancel";
        let msg = if self.cred.status.is_empty() {
            help.to_string()
        } else {
            format!("{}\n{}", help, self.cred.status)
        };
        f.render_widget(
            Paragraph::new(msg).style(Style::default().fg(Color::Gray)),
            rows[3],
        );
    }
}

fn build_candidates(hosts: &[sshconfig::Host]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for h in hosts {
        let mut parts = vec![h.alias.clone()];
        if !h.user.is_empty() {
            parts.push(format!("as {}", h.user));
        }
        if h.port > 0 && h.port != 22 {
            parts.push(format!(":{}", h.port));
        }
        if !h.proxyjump.is_empty() {
            parts.push(format!("via {}", h.proxyjump));
        }
        if !h.hostname.is_empty() && h.hostname != h.alias {
            parts.push(format!("-> {}", h.hostname));
        }
        let alias_lc = h.alias.to_lowercase();
        let hostname_lc = h.hostname.to_lowercase();
        let proxyjump_lc = h.proxyjump.to_lowercase();
        let search_all_lc =
            format!("{} {} {} {}", h.alias, h.hostname, h.user, h.proxyjump).to_lowercase();
        out.push(Candidate {
            host: h.clone(),
            alias_lc,
            hostname_lc,
            proxyjump_lc,
            search_all_lc,
            line: parts.join(" "),
        });
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MatchSortKey {
    bucket: u8,
    position: u16,
    len: u16,
}

impl Ord for MatchSortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.bucket, self.position, self.len).cmp(&(other.bucket, other.position, other.len))
    }
}

impl PartialOrd for MatchSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn match_sort_key(query: &str, c: &Candidate) -> Option<MatchSortKey> {
    // bucket priority: alias prefix, hostname prefix, alias token prefix, hostname token prefix,
    // alias fuzzy, hostname fuzzy, proxyjump matches, any fuzzy.

    let q = query.trim();
    if q.is_empty() {
        return Some(MatchSortKey {
            bucket: 255,
            position: 0,
            len: c.alias_lc.len().min(u16::MAX as usize) as u16,
        });
    }

    if c.alias_lc.starts_with(q) {
        return Some(MatchSortKey {
            bucket: 0,
            position: 0,
            len: c.alias_lc.len().min(u16::MAX as usize) as u16,
        });
    }
    if !c.hostname_lc.is_empty() && c.hostname_lc.starts_with(q) {
        return Some(MatchSortKey {
            bucket: 1,
            position: 0,
            len: c.hostname_lc.len().min(u16::MAX as usize) as u16,
        });
    }

    if let Some(pos) = token_prefix_pos(&c.alias_lc, q) {
        return Some(MatchSortKey {
            bucket: 2,
            position: pos,
            len: c.alias_lc.len().min(u16::MAX as usize) as u16,
        });
    }
    if let Some(pos) = token_prefix_pos(&c.hostname_lc, q) {
        return Some(MatchSortKey {
            bucket: 3,
            position: pos,
            len: c.hostname_lc.len().min(u16::MAX as usize) as u16,
        });
    }

    if let Some(pos) = fuzzy_match_pos(q, &c.alias_lc) {
        return Some(MatchSortKey {
            bucket: 4,
            position: pos,
            len: c.alias_lc.len().min(u16::MAX as usize) as u16,
        });
    }
    if let Some(pos) = fuzzy_match_pos(q, &c.hostname_lc) {
        return Some(MatchSortKey {
            bucket: 5,
            position: pos,
            len: c.hostname_lc.len().min(u16::MAX as usize) as u16,
        });
    }

    if !c.proxyjump_lc.is_empty() {
        if c.proxyjump_lc.starts_with(q) {
            return Some(MatchSortKey {
                bucket: 6,
                position: 0,
                len: c.proxyjump_lc.len().min(u16::MAX as usize) as u16,
            });
        }
        if let Some(pos) = token_prefix_pos(&c.proxyjump_lc, q) {
            return Some(MatchSortKey {
                bucket: 6,
                position: pos,
                len: c.proxyjump_lc.len().min(u16::MAX as usize) as u16,
            });
        }
        if let Some(pos) = fuzzy_match_pos(q, &c.proxyjump_lc) {
            return Some(MatchSortKey {
                bucket: 6,
                position: pos,
                len: c.proxyjump_lc.len().min(u16::MAX as usize) as u16,
            });
        }
    }

    fuzzy_match_pos(q, &c.search_all_lc).map(|pos| MatchSortKey {
        bucket: 7,
        position: pos,
        len: c.alias_lc.len().min(u16::MAX as usize) as u16,
    })
}

fn token_prefix_pos(text: &str, q: &str) -> Option<u16> {
    if q.is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    let q_bytes = q.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let is_boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if is_boundary
            && bytes.len() - i >= q_bytes.len()
            && bytes[i..i + q_bytes.len()] == *q_bytes
        {
            return Some(i.min(u16::MAX as usize) as u16);
        }
        i += 1;
    }
    None
}

fn fuzzy_match_pos(query: &str, text: &str) -> Option<u16> {
    if query.is_empty() {
        return Some(0);
    }
    let mut qi = query.chars();
    let mut cur = qi.next();
    let mut first_match: Option<usize> = None;
    for (i, ch) in text.chars().enumerate() {
        if let Some(q) = cur {
            if ch == q {
                if first_match.is_none() {
                    first_match = Some(i);
                }
                cur = qi.next();
            }
        } else {
            break;
        }
    }
    if cur.is_none() {
        Some(first_match.unwrap_or(0).min(u16::MAX as usize) as u16)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_pos() {
        assert!(fuzzy_match_pos("abc", "a_b_c").is_some());
        assert!(fuzzy_match_pos("abc", "acb").is_none());
    }

    #[test]
    fn test_match_sort_key_prefers_alias_prefix_over_proxyjump() {
        let h = sshconfig::Host {
            alias: "bacchus.lmig.com".to_string(),
            hostname: "".to_string(),
            user: "".to_string(),
            port: 0,
            proxyjump: "".to_string(),
            identity_files: Vec::new(),
            source_path: "".to_string(),
            source_line: 1,
        };
        let c_alias = build_candidates(&[h])[0].clone();

        let h2 = sshconfig::Host {
            alias: "010k-onos-leaf-sw1".to_string(),
            hostname: "10.0.0.1".to_string(),
            user: "".to_string(),
            port: 0,
            proxyjump: "bacchus.lmig.com".to_string(),
            identity_files: Vec::new(),
            source_path: "".to_string(),
            source_line: 1,
        };
        let c_proxy = build_candidates(&[h2])[0].clone();

        let q = "bacchus";
        let k1 = match_sort_key(q, &c_alias).unwrap();
        let k2 = match_sort_key(q, &c_proxy).unwrap();
        assert!(k1.bucket < k2.bucket);
    }
}
