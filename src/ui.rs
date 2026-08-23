use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::layout::Flex;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::collector::{Antag, MatchInfo, Rule, RunAnt, RunRow, Snapshot, UserShare};
use crate::conditions::fmt_num;
use crate::daemon;
use crate::metrics::{
    attribution, fmt_bytes, fmt_clock, fmt_pct, fmt_secs, stall_secs, system_congestion_index,
    wait_ratio_pct,
};
use crate::save;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Save,
    Load,
}

fn cur_left(s: &str, c: usize) -> usize {
    s[..c].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
}

fn cur_right(s: &str, c: usize) -> usize {
    s[c..].char_indices().nth(1).map(|(i, _)| c + i).unwrap_or(s.len())
}

fn input_char(s: &mut String, c: &mut usize, ch: char) {
    s.insert(*c, ch);
    *c += ch.len_utf8();
}

fn input_backspace(s: &mut String, c: &mut usize) {
    let l = cur_left(s, *c);
    s.replace_range(l..*c, "");
    *c = l;
}

fn input_delete(s: &mut String, c: &mut usize) {
    let r = cur_right(s, *c);
    s.replace_range(*c..r, "");
}

struct Popup {
    input: String,
    cursor: usize,
    exc_input: String,
    exc_cursor: usize,
    exclude_focused: bool,
    matches: Vec<MatchInfo>,
    error: Option<String>,
    last_rules: Vec<Rule>,
    confirm_area: Rect,
    cancel_area: Rect,
    include_area: Rect,
    exclude_area: Rect,
}

fn rules_from_popup(p: &Popup) -> Vec<Rule> {
    let mut rules = Vec::new();
    if !p.input.trim().is_empty() {
        rules.push(Rule {
            pattern: p.input.trim().to_string(),
            regex: true,
            exclude: false,
        });
    }
    if !p.exc_input.trim().is_empty() {
        rules.push(Rule {
            pattern: p.exc_input.trim().to_string(),
            regex: true,
            exclude: true,
        });
    }
    rules
}

fn open_popup(app: &mut App) {
    let rules = app
        .snapshot
        .as_ref()
        .map(|s| s.rules.clone())
        .unwrap_or_default();
    let mut input = String::new();
    let mut exc_input = String::new();
    for r in rules {
        if r.exclude {
            if exc_input.is_empty() {
                exc_input = r.pattern.clone();
            }
        } else if input.is_empty() {
            input = r.pattern.clone();
        }
    }
    let cursor = input.len();
    let exc_cursor = exc_input.len();
    app.popup = Some(Popup {
        input,
        cursor,
        exc_input,
        exc_cursor,
        exclude_focused: false,
        matches: Vec::new(),
        error: None,
        last_rules: Vec::new(),
        confirm_area: Rect::default(),
        cancel_area: Rect::default(),
        include_area: Rect::default(),
        exclude_area: Rect::default(),
    });
}

fn confirm_filter(app: &mut App) {
    if let Some(p) = app.popup.take() {
        let rules = rules_from_popup(&p);
        if !rules.iter().any(|r| !r.exclude) {
            app.flash = Some(("filter is empty".into(), Instant::now()));
            return;
        }
        app.paused = false;
        app.name_input = crate::collector::rules_display(&rules);
        if daemon::set_rules(&rules).is_err() {
            let _ = daemon::stop();
            let _ = daemon::start("", app.interval, app.history_len);
            let _ = daemon::set_rules(&rules);
        }
        app.refresh();
    }
}

fn update_preview(app: &mut App) {
    let Some(p) = &mut app.popup else {
        return;
    };
    let rules = rules_from_popup(p);
    if rules == p.last_rules {
        return;
    }
    if !rules.iter().any(|r| !r.exclude) {
        p.matches.clear();
        p.error = None;
        p.last_rules = rules;
        app.dirty = true;
        return;
    }
    match daemon::preview_rules(&rules) {
        Ok(m) => {
            p.matches = m;
            p.error = None;
        }
        Err(e) => {
            p.matches.clear();
            p.error = Some(match e.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                    "daemon offline — confirm will start it".to_string()
                }
                _ => format!("daemon: {e}"),
            });
        }
    }
    p.last_rules = rules;
    app.dirty = true;
}

struct Browser {
    mode: InputMode,
    dir: PathBuf,
    entries: Vec<(String, bool)>,
    sel: usize,
    scroll: usize,
    name: String,
    name_cursor: usize,
    list_focused: bool,
    list_area: Rect,
    name_area: Rect,
    confirm_area: Rect,
    cancel_area: Rect,
}

struct Stealth {
    input: String,
    cursor: usize,
    confirm_area: Rect,
    cancel_area: Rect,
    default_areas: Vec<(String, Rect)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Highlight {
    User(String),
    Ant { pid: i32, comm: String },
    Run(u64),
}

pub struct App {
    pub name_input: String,
    snapshot: Option<Snapshot>,
    offline: bool,
    paused: bool,
    flash: Option<(String, Instant)>,
    popup: Option<Popup>,
    browser: Option<Browser>,
    stealth: Option<Stealth>,
    filter_btn_area: Rect,
    dirty: bool,
    interval: Duration,
    history_len: usize,
    scroll: usize,
    uscroll: usize,
    ascroll: usize,
    runs_sort: (usize, bool),
    users_sort: (usize, bool),
    ants_sort: (usize, bool),
    runs_area: Rect,
    users_area: Rect,
    ants_area: Rect,
    focus: usize,
    sel_user: Option<usize>,
    sel_ant: Option<usize>,
    highlight: Option<Highlight>,
    mouse: Option<(u16, u16)>,
    hover: Option<(String, u16, u16)>,
    live: bool,
    user_tabs: (Rect, Rect),
    ants_tabs: (Rect, Rect),
    stat_copy_area: Rect,
    stat_table_area: Rect,
    first_run: bool,
    first_run_rect: Option<Rect>,
    help: bool,
    quit: bool,
}

impl App {
    pub fn new(initial: String, history_len: usize, interval: Duration) -> Self {
        Self {
            name_input: initial,
            snapshot: None,
            offline: false,
            paused: false,
            flash: None,
            popup: None,
            browser: None,
            stealth: None,
            filter_btn_area: Rect::default(),
            dirty: true,
            interval,
            history_len,
            scroll: 0,
            uscroll: 0,
            ascroll: 0,
            runs_sort: (5, false),
            users_sort: (2, false),
            ants_sort: (3, false),
            runs_area: Rect::default(),
            users_area: Rect::default(),
            ants_area: Rect::default(),
            focus: 0,
            sel_user: None,
            sel_ant: None,
            highlight: None,
            mouse: None,
            hover: None,
            live: false,
            user_tabs: (Rect::default(), Rect::default()),
            ants_tabs: (Rect::default(), Rect::default()),
            stat_copy_area: Rect::default(),
            stat_table_area: Rect::default(),
            first_run: first_run_notice_wanted(),
            first_run_rect: None,
            help: false,
            quit: false,
        }
    }

    fn refresh(&mut self) {
        if self.paused {
            return;
        }
        let last_seq = self.snapshot.as_ref().map(|s| s.seq).unwrap_or(0);
        match daemon::request_snapshot(last_seq) {
            Ok(Some(s)) => {
                if self.name_input.is_empty() && !s.target.is_empty() {
                    self.name_input = s.target.clone();
                }
                self.snapshot = Some(s);
                self.offline = false;
                self.dirty = true;
            }
            Ok(None) => {}
            Err(_) => {
                if daemon::ensure_compatible().is_err() {
                    let target = self
                        .snapshot
                        .as_ref()
                        .map(|s| s.target.clone())
                        .unwrap_or_else(|| self.name_input.clone());
                    let _ = daemon::stop();
                    let _ = daemon::start(&target, self.interval, self.history_len);
                }
                self.offline = true;
                self.dirty = true;
            }
        }
    }
}

fn open_browser(app: &mut App, mode: InputMode) {
    let mut b = Browser {
        mode,
        dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        entries: Vec::new(),
        sel: 0,
        scroll: 0,
        name: String::new(),
        name_cursor: 0,
        list_focused: true,
        list_area: Rect::default(),
        name_area: Rect::default(),
        confirm_area: Rect::default(),
        cancel_area: Rect::default(),
    };
    match mode {
        InputMode::Save => {
            let target = app
                .snapshot
                .as_ref()
                .map(|s| s.target.clone())
                .unwrap_or_default();
            b.name = save::default_save_path(&target);
        }
        InputMode::Load => {
            if let Some(n) = save::latest_save() {
                b.name = n;
            }
        }
    }
    refresh_browser(&mut b);
    app.browser = Some(b);
}

fn refresh_browser(b: &mut Browser) {
    b.entries.clear();
    b.sel = 0;
    b.scroll = 0;
    if let Ok(rd) = std::fs::read_dir(&b.dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !is_dir && b.mode == InputMode::Load && !name.ends_with(".csv") {
                continue;
            }
            b.entries.push((name, is_dir));
        }
    }
    b.entries.sort_by(|a, c| match (a.1, c.1) {
        (true, true) | (false, false) => a.0.to_lowercase().cmp(&c.0.to_lowercase()),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
    });
    b.entries.insert(0, ("..".into(), true));
    if let Some(i) = b
        .entries
        .iter()
        .position(|(n, d)| !d && *n == b.name)
    {
        b.sel = i;
    }
}

fn browser_enter(app: &mut App) {
    let Some(b) = &mut app.browser else {
        return;
    };
    let Some((name, is_dir)) = b.entries.get(b.sel).cloned() else {
        return;
    };
    if name == ".." {
        if let Some(p) = b.dir.parent() {
            b.dir = p.to_path_buf();
            refresh_browser(b);
        }
    } else if is_dir {
        b.dir = b.dir.join(&name);
        refresh_browser(b);
    } else if b.mode == InputMode::Load {
        b.name = name;
        browser_confirm(app);
    } else {
        b.name = name;
        b.name_cursor = b.name.len();
    }
}

fn browser_up(app: &mut App) {
    if let Some(b) = &mut app.browser
        && let Some(p) = b.dir.parent()
    {
        b.dir = p.to_path_buf();
        refresh_browser(b);
    }
}

fn browser_confirm(app: &mut App) {
    let Some(b) = app.browser.take() else {
        return;
    };
    let path = b.dir.join(b.name.trim()).to_string_lossy().into_owned();
    match b.mode {
        InputMode::Save => {
            if b.name.trim().is_empty() {
                app.flash = Some(("no filename".into(), Instant::now()));
            } else if let Some(s) = &app.snapshot {
                match save::save_snapshot(s, &path) {
                    Ok(()) => app.flash = Some((format!("saved {path}"), Instant::now())),
                    Err(e) => app.flash = Some((format!("save failed: {e}"), Instant::now())),
                }
            } else {
                app.flash = Some(("nothing to save".into(), Instant::now()));
            }
        }
        InputMode::Load => {
            if b.name.trim().is_empty() {
                app.flash = Some(("no file selected".into(), Instant::now()));
            } else {
                match save::load_snapshot(&path) {
                    Ok(s) => {
                        let runs = s.runs.len();
                        app.snapshot = Some(s);
                        app.paused = true;
                        app.offline = false;
                        app.flash = Some((format!("loaded {path} — {runs} runs"), Instant::now()));
                    }
                    Err(e) => app.flash = Some((format!("load failed: {e}"), Instant::now())),
                }
            }
        }
    }
}

fn open_stealth(app: &mut App) {
    app.stealth = Some(Stealth {
        input: String::new(),
        cursor: 0,
        confirm_area: Rect::default(),
        cancel_area: Rect::default(),
        default_areas: Vec::new(),
    });
}

fn confirm_stealth(app: &mut App) {
    if let Some(st) = app.stealth.take() {
        let name = st.input.trim().to_string();
        if name.is_empty() {
            app.flash = Some(("name is empty".into(), Instant::now()));
        } else {
            let _ = daemon::set_stealth(&name);
            daemon::rename_self(&name);
            app.flash = Some((
                format!("stealth on — processes now show as '{name}'"),
                Instant::now(),
            ));
        }
    }
}

pub fn run(app: &mut App) -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    enable_raw_mode()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, app);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    let mut last_req = Instant::now();
    while !app.quit {
        if last_req.elapsed() >= Duration::from_millis(250) {
            app.refresh();
            update_preview(app);
            last_req = Instant::now();
        }
        if app.dirty {
            terminal.draw(|f| draw(f, app))?;
            app.dirty = false;
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) => {
                    handle_key(app, k);
                    app.dirty = true;
                }
                Event::Mouse(m) => {
                    handle_mouse(app, m);
                    app.dirty = true;
                }
                Event::Resize(_, _) => {
                    terminal.autoresize()?;
                    app.dirty = true;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, k: KeyEvent) {
    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
        app.quit = true;
        return;
    }
    if app.first_run {
        let show_help = k.code == KeyCode::Char('h');
        mark_first_run_done();
        app.first_run = false;
        if show_help {
            app.help = true;
        }
        return;
    }
    if app.help {
        match k.code {
            KeyCode::Char('h') => app.help = false,
            KeyCode::Char('q') => {
                let _ = daemon::stop();
                app.quit = true;
            }
            KeyCode::Char('d') => app.quit = true,
            _ => {}
        }
        return;
    }
    if app.browser.is_some() {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let mut close = false;
        let mut act = None;
        if let Some(b) = &mut app.browser {
            match k.code {
                KeyCode::Esc => close = true,
                KeyCode::Tab => b.list_focused = !b.list_focused,
                KeyCode::Char('j') | KeyCode::Down if b.list_focused => {
                    b.sel = (b.sel + 1).min(b.entries.len().saturating_sub(1));
                }
                KeyCode::Char('k') | KeyCode::Up if b.list_focused => {
                    b.sel = b.sel.saturating_sub(1);
                }
                KeyCode::Enter if b.list_focused => act = Some("enter"),
                KeyCode::Backspace if b.list_focused => act = Some("up"),
                KeyCode::Char(c) if !ctrl && !b.list_focused => {
                    input_char(&mut b.name, &mut b.name_cursor, c);
                }
                KeyCode::Backspace if !b.list_focused => {
                    input_backspace(&mut b.name, &mut b.name_cursor);
                }
                KeyCode::Delete if !b.list_focused => {
                    input_delete(&mut b.name, &mut b.name_cursor);
                }
                KeyCode::Left if !b.list_focused => b.name_cursor = cur_left(&b.name, b.name_cursor),
                KeyCode::Right if !b.list_focused => {
                    b.name_cursor = cur_right(&b.name, b.name_cursor);
                }
                KeyCode::Home => b.name_cursor = 0,
                KeyCode::End => b.name_cursor = b.name.len(),
                KeyCode::Enter if !b.list_focused => act = Some("confirm"),
                _ => {}
            }
        }
        match act {
            Some("enter") => browser_enter(app),
            Some("up") => browser_up(app),
            Some("confirm") => browser_confirm(app),
            _ => {}
        }
        if close {
            app.browser = None;
        }
        return;
    }
    if app.stealth.is_some() {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char(c) if !ctrl => {
                if let Some(st) = &mut app.stealth {
                    input_char(&mut st.input, &mut st.cursor, c);
                }
            }
            KeyCode::Backspace => {
                if let Some(st) = &mut app.stealth {
                    input_backspace(&mut st.input, &mut st.cursor);
                }
            }
            KeyCode::Delete => {
                if let Some(st) = &mut app.stealth {
                    input_delete(&mut st.input, &mut st.cursor);
                }
            }
            KeyCode::Left => {
                if let Some(st) = &mut app.stealth {
                    st.cursor = cur_left(&st.input, st.cursor);
                }
            }
            KeyCode::Right => {
                if let Some(st) = &mut app.stealth {
                    st.cursor = cur_right(&st.input, st.cursor);
                }
            }
            KeyCode::Home => {
                if let Some(st) = &mut app.stealth {
                    st.cursor = 0;
                }
            }
            KeyCode::End => {
                if let Some(st) = &mut app.stealth {
                    st.cursor = st.input.len();
                }
            }
            KeyCode::Enter => confirm_stealth(app),
            KeyCode::Esc => {
                app.stealth = None;
            }
            _ => {}
        }
        return;
    }
    if app.popup.is_some() {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let excl = app.popup.as_ref().map(|p| p.exclude_focused).unwrap_or(false);
        match k.code {
            KeyCode::Esc => {
                app.popup = None;
            }
            KeyCode::Tab => {
                if let Some(p) = &mut app.popup {
                    p.exclude_focused = !p.exclude_focused;
                }
            }
            KeyCode::Enter => confirm_filter(app),
            KeyCode::Char(c) if !ctrl => {
                if let Some(p) = &mut app.popup {
                    if excl {
                        input_char(&mut p.exc_input, &mut p.exc_cursor, c);
                    } else {
                        input_char(&mut p.input, &mut p.cursor, c);
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(p) = &mut app.popup {
                    if excl {
                        input_backspace(&mut p.exc_input, &mut p.exc_cursor);
                    } else {
                        input_backspace(&mut p.input, &mut p.cursor);
                    }
                }
            }
            KeyCode::Delete => {
                if let Some(p) = &mut app.popup {
                    if excl {
                        input_delete(&mut p.exc_input, &mut p.exc_cursor);
                    } else {
                        input_delete(&mut p.input, &mut p.cursor);
                    }
                }
            }
            KeyCode::Left => {
                if let Some(p) = &mut app.popup {
                    if excl {
                        p.exc_cursor = cur_left(&p.exc_input, p.exc_cursor);
                    } else {
                        p.cursor = cur_left(&p.input, p.cursor);
                    }
                }
            }
            KeyCode::Right => {
                if let Some(p) = &mut app.popup {
                    if excl {
                        p.exc_cursor = cur_right(&p.exc_input, p.exc_cursor);
                    } else {
                        p.cursor = cur_right(&p.input, p.cursor);
                    }
                }
            }
            KeyCode::Home => {
                if let Some(p) = &mut app.popup {
                    if excl {
                        p.exc_cursor = 0;
                    } else {
                        p.cursor = 0;
                    }
                }
            }
            KeyCode::End => {
                if let Some(p) = &mut app.popup {
                    if excl {
                        p.exc_cursor = p.exc_input.len();
                    } else {
                        p.cursor = p.input.len();
                    }
                }
            }
            _ => {}
        }
        return;
    }
    match k.code {
        KeyCode::Char('q') => {
            let _ = daemon::stop();
            app.quit = true;
        }
        KeyCode::Char('d') => app.quit = true,
        KeyCode::Char('s') => open_browser(app, InputMode::Save),
        KeyCode::Char('l') => open_browser(app, InputMode::Load),
        KeyCode::Char('f') => open_popup(app),
        KeyCode::Char('t') => open_stealth(app),
        KeyCode::Char('v') => app.live = !app.live,
        KeyCode::Char('h') => app.help = true,
        KeyCode::Char('r') => {
            if app.paused {
                app.paused = false;
                app.refresh();
            } else {
                let target = app
                    .snapshot
                    .as_ref()
                    .map(|s| s.target.clone())
                    .unwrap_or_else(|| app.name_input.clone());
                let _ = daemon::stop();
                let _ = daemon::start(&target, app.interval, app.history_len);
                app.refresh();
            }
        }
        KeyCode::Esc => app.highlight = None,
        KeyCode::Enter => {
            if app.focus == 1 {
                apply_highlight(app, 1);
            } else if app.focus == 2 {
                apply_highlight(app, 2);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => match app.focus {
            0 => app.scroll = app.scroll.saturating_add(1),
            1 => {
                let n = app.snapshot.as_ref().map(|s| s.users.len()).unwrap_or(0);
                if n > 0 {
                    let cur = app.sel_user.unwrap_or(0);
                    app.sel_user = Some((cur + 1).min(n - 1));
                    apply_highlight(app, 1);
                }
            }
            _ => {
                let n = app
                    .snapshot
                    .as_ref()
                    .map(|s| s.antagonists.len())
                    .unwrap_or(0);
                if n > 0 {
                    let cur = app.sel_ant.unwrap_or(0);
                    app.sel_ant = Some((cur + 1).min(n - 1));
                    apply_highlight(app, 2);
                }
            }
        },
        KeyCode::Up | KeyCode::Char('k') => match app.focus {
            0 => app.scroll = app.scroll.saturating_sub(1),
            1 => {
                if let Some(cur) = app.sel_user {
                    app.sel_user = Some(cur.saturating_sub(1));
                    apply_highlight(app, 1);
                }
            }
            _ => {
                if let Some(cur) = app.sel_ant {
                    app.sel_ant = Some(cur.saturating_sub(1));
                    apply_highlight(app, 2);
                }
            }
        },
        _ => {}
    }
}

fn in_rect(row: u16, col: u16, area: Rect) -> bool {
    area.width > 0
        && area.height > 0
        && col >= area.x
        && col < area.x + area.width
        && row >= area.y
        && row < area.y + area.height
}

fn col_at(x: u16, area: &Rect, fixed: &[u16], min_idx: usize) -> Option<usize> {
    if area.width == 0 || x < area.x || x >= area.x + area.width {
        return None;
    }
    let n = fixed.len() + 1;
    let sum: u16 = fixed.iter().sum();
    let gaps = n as u16;
    let min_w = area.width.saturating_sub(sum + gaps);
    let mut cur = area.x;
    for col in 0..n {
        let wcol = if col == min_idx {
            min_w
        } else if col < min_idx {
            fixed[col]
        } else {
            fixed[col - 1]
        };
        if x < cur + wcol {
            return Some(col);
        }
        cur += wcol + 1;
    }
    Some(n - 1)
}

/// Maps a click x to a column index for a fully fixed, left-aligned table
/// (no flexible column); clicks in the trailing empty space hit no column.
fn col_at_left(x: u16, area: &Rect, widths: &[u16]) -> Option<usize> {
    if area.width == 0 || x < area.x || x >= area.x + area.width {
        return None;
    }
    let mut cur = area.x;
    for (i, w) in widths.iter().enumerate() {
        if x < cur + *w {
            return Some(i);
        }
        cur += *w + 1;
    }
    None
}

fn toggle_sort(state: &mut (usize, bool), col: usize) {
    if state.0 == col {
        state.1 = !state.1;
    } else {
        state.0 = col;
        state.1 = false;
    }
}

/// Sets `app.highlight` from the currently selected row of the users (1) or
/// processes (2) panel, in the panel's current sort order.
fn apply_highlight(app: &mut App, panel: usize) {
    let Some(s) = &app.snapshot else {
        return;
    };
    if panel == 1 {
        let list = if app.live { &s.live_users } else { &s.users };
        let mut sorted: Vec<&UserShare> = list.iter().collect();
        let overlap = user_overlap(s);
        sort_users(&mut sorted, app.users_sort, &overlap);
        let Some(i) = app.sel_user else {
            return;
        };
        if let Some(u) = sorted.get(i) {
            app.highlight = Some(Highlight::User(u.user.clone()));
        }
    } else if panel == 2 {
        let list = if app.live { &s.live_ants } else { &s.antagonists };
        let mut sorted: Vec<&Antag> = list.iter().collect();
        let (by_pid, by_comm) = proc_overlap(s);
        sort_ants(&mut sorted, app.ants_sort, &by_pid, &by_comm);
        let Some(i) = app.sel_ant else {
            return;
        };
        if let Some(a) = sorted.get(i) {
            app.highlight = Some(Highlight::Ant {
                pid: a.pid,
                comm: a.comm.clone(),
            });
        }
    }
}

/// Whether a run row was affected by the highlighted process/user: the
/// per-run lists are already thresholded (cpu_secs >= 1s), so membership is
/// the check. Loaded snapshots carry pid -1 for processes, so fall back to
/// matching the comm there.
fn run_affected(r: &RunRow, h: &Highlight) -> bool {
    match h {
        Highlight::User(u) => r.run_users.iter().any(|ru| &ru.user == u),
        Highlight::Ant { pid, comm } => r
            .ants
            .iter()
            .any(|a| a.pid == *pid || (*pid < 0 && &a.comm == comm)),
        Highlight::Run(order) => r.order == *order,
    }
}

/// Per-run interference attribution shares (CPU / MEM / IO, in %).
fn run_attr(r: &RunRow) -> Option<(f64, f64, f64)> {
    attribution(
        r.wait_secs,
        stall_secs(r.psi[1], r.wall),
        stall_secs(r.psi[2], r.wall),
    )
}

/// Per-run System Congestion Index: the run's PSI penalties + scheduler wait
/// compressed into the same saturating 0-100 index as the Live gauge.
fn run_ci(r: &RunRow) -> f64 {
    system_congestion_index(r.psi[0], r.psi[1], r.psi[2], r.wait_pct.unwrap_or(0.0))
}

/// Aggregated overlap impact of a user/process: in how many of our runs they
/// were active (from the per-run attribution lists, thresholded at 1s/run).
type UserOverlap = HashMap<String, usize>;
type ProcOverlap = (HashMap<i32, usize>, HashMap<String, usize>);

fn user_overlap(s: &Snapshot) -> UserOverlap {
    let mut m: UserOverlap = HashMap::new();
    for r in &s.runs {
        for ru in &r.run_users {
            *m.entry(ru.user.clone()).or_insert(0) += 1;
        }
    }
    m
}

/// Overlap impact of every process, keyed by pid (exact for live sessions)
/// and by comm (fallback for loaded snapshots, where pids are -1).
fn proc_overlap(s: &Snapshot) -> ProcOverlap {
    let mut by_pid: HashMap<i32, usize> = HashMap::new();
    let mut by_comm: HashMap<String, usize> = HashMap::new();
    for r in &s.runs {
        for ra in &r.ants {
            *by_pid.entry(ra.pid).or_insert(0) += 1;
            *by_comm.entry(ra.comm.clone()).or_insert(0) += 1;
        }
    }
    (by_pid, by_comm)
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    if app.help {
        return;
    }
    if app.first_run {
        // Only a click on [ got it ] dismisses the welcome notice; mouse
        // moves (and clicks elsewhere) leave it up until then.
        if let Some(pop) = app.first_run_rect
            && m.kind == MouseEventKind::Down(MouseButton::Left)
            && in_rect(m.row, m.column, first_run_got_it_rect(pop))
        {
            mark_first_run_done();
            app.first_run = false;
        }
        return;
    }
    match m.kind {
        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
            app.mouse = Some((m.column, m.row));
        }
        MouseEventKind::ScrollDown => {
            if app.browser.is_some() {
                if let Some(b) = &mut app.browser
                    && in_rect(m.row, m.column, b.list_area)
                {
                    b.sel = (b.sel + 1).min(b.entries.len().saturating_sub(1));
                }
            } else if in_rect(m.row, m.column, app.runs_area) {
                app.scroll = app.scroll.saturating_add(1);
            } else if in_rect(m.row, m.column, app.users_area) {
                app.uscroll = app.uscroll.saturating_add(1);
            } else if in_rect(m.row, m.column, app.ants_area) {
                app.ascroll = app.ascroll.saturating_add(1);
            }
        }
        MouseEventKind::ScrollUp => {
            if app.browser.is_some() {
                if let Some(b) = &mut app.browser
                    && in_rect(m.row, m.column, b.list_area)
                {
                    b.sel = b.sel.saturating_sub(1);
                }
            } else if in_rect(m.row, m.column, app.runs_area) {
                app.scroll = app.scroll.saturating_sub(1);
            } else if in_rect(m.row, m.column, app.users_area) {
                app.uscroll = app.uscroll.saturating_sub(1);
            } else if in_rect(m.row, m.column, app.ants_area) {
                app.ascroll = app.ascroll.saturating_sub(1);
            }
        }
        MouseEventKind::Down(_) => {
            app.mouse = Some((m.column, m.row));
            if let Some(b) = &app.browser {
                let confirm = in_rect(m.row, m.column, b.confirm_area);
                let cancel = in_rect(m.row, m.column, b.cancel_area);
                let name = in_rect(m.row, m.column, b.name_area);
                let list = in_rect(m.row, m.column, b.list_area);
                let row = if list {
                    Some(b.scroll + (m.row - b.list_area.y) as usize)
                } else {
                    None
                };
                if confirm {
                    browser_confirm(app);
                } else if cancel {
                    app.browser = None;
                } else if name {
                    if let Some(b) = &mut app.browser {
                        b.list_focused = false;
                    }
                } else if let Some(idx) = row
                    && idx < b.entries.len()
                    && let Some(b) = &mut app.browser {
                        b.sel = idx;
                        b.list_focused = true;
                    }
                return;
            }
            if let Some(st) = &app.stealth {
                let confirm = in_rect(m.row, m.column, st.confirm_area);
                let cancel = in_rect(m.row, m.column, st.cancel_area);
                let defaults: Vec<String> = st
                    .default_areas
                    .iter()
                    .filter(|(_, r)| in_rect(m.row, m.column, *r))
                    .map(|(n, _)| n.clone())
                    .collect();
                if confirm {
                    confirm_stealth(app);
                } else if cancel {
                    app.stealth = None;
                } else if !defaults.is_empty()
                    && let Some(st) = &mut app.stealth {
                        st.input = defaults[0].clone();
                        st.cursor = st.input.len();
                    }
                return;
            }
            if let Some(p) = &app.popup {
                let confirm = in_rect(m.row, m.column, p.confirm_area);
                let cancel = in_rect(m.row, m.column, p.cancel_area);
                if confirm {
                    confirm_filter(app);
                    return;
                }
                if cancel {
                    app.popup = None;
                    return;
                }
                if let Some(p) = &app.popup
                    && in_rect(m.row, m.column, p.include_area)
                {
                    if let Some(p) = &mut app.popup {
                        p.exclude_focused = false;
                    }
                    return;
                }
                if let Some(p) = &app.popup
                    && in_rect(m.row, m.column, p.exclude_area)
                {
                    if let Some(p) = &mut app.popup {
                        p.exclude_focused = true;
                    }
                    return;
                }
                return;
            }
            if in_rect(m.row, m.column, app.filter_btn_area) {
                open_popup(app);
                return;
            }
            if in_rect(m.row, m.column, app.stat_copy_area) {
                copy_conditions(app, false);
                return;
            }
            if in_rect(m.row, m.column, app.stat_table_area) {
                copy_conditions(app, true);
                return;
            }
            if in_rect(m.row, m.column, app.user_tabs.0)
                || in_rect(m.row, m.column, app.ants_tabs.0)
            {
                app.live = false;
                return;
            }
            if in_rect(m.row, m.column, app.user_tabs.1)
                || in_rect(m.row, m.column, app.ants_tabs.1)
            {
                app.live = true;
                return;
            }
            if m.row == app.runs_area.y
                && let Some(col) = col_at(m.column, &app.runs_area, &RUNS_FIXED, 0)
            {
                toggle_sort(&mut app.runs_sort, col);
            } else if m.row == app.users_area.y
                && let Some(col) = col_at_left(m.column, &app.users_area, &USERS_WIDTHS)
                && col > 0
            {
                toggle_sort(&mut app.users_sort, col);
            } else if m.row == app.ants_area.y
                && let Some(col) = col_at(m.column, &app.ants_area, &ANTS_FIXED, 6)
                && col > 0
            {
                toggle_sort(&mut app.ants_sort, col);
            } else if in_rect(m.row, m.column, app.runs_area)
                && m.row > app.runs_area.y
                && let Some(s) = &app.snapshot
                && !s.runs.is_empty()
            {
                let mut sorted: Vec<&RunRow> = s.runs.iter().collect();
                sort_runs(&mut sorted, app.runs_sort);
                let idx = app.scroll + (m.row - app.runs_area.y) as usize - 1;
                if let Some(r) = sorted.get(idx) {
                    let order = r.order;
                    app.highlight = if app.highlight == Some(Highlight::Run(order)) {
                        None
                    } else {
                        Some(Highlight::Run(order))
                    };
                    app.focus = 0;
                    app.sel_user = None;
                    app.sel_ant = None;
                }
            } else if in_rect(m.row, m.column, app.users_area)
                && m.row > app.users_area.y
                && let Some(s) = &app.snapshot
            {
                let list = if app.live { &s.live_users } else { &s.users };
                if !list.is_empty() {
                    let idx = app.uscroll + (m.row - app.users_area.y) as usize - 1;
                    if idx < list.len() {
                        app.focus = 1;
                        app.sel_user = Some(idx);
                        apply_highlight(app, 1);
                    }
                }
            } else if in_rect(m.row, m.column, app.ants_area)
                && m.row > app.ants_area.y
                && let Some(s) = &app.snapshot
            {
                let list = if app.live { &s.live_ants } else { &s.antagonists };
                if !list.is_empty() {
                    let idx = app.ascroll + (m.row - app.ants_area.y) as usize - 1;
                    if idx < list.len() {
                        app.focus = 2;
                        app.sel_ant = Some(idx);
                        apply_highlight(app, 2);
                    }
                }
            }
        }
        _ => {}
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    app.hover = None;
    if app.help {
        draw_help(f, app);
        return;
    }
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(13),
        Constraint::Length(1),
    ])
    .split(area);
    draw_header(f, app, chunks[0]);
    draw_middle(f, app, chunks[1]);
    draw_users_ants(f, app, chunks[2]);
    draw_footer(f, app, chunks[3]);
    draw_popup(f, app);
    draw_stealth(f, app);
    draw_browser(f, app);
    if let Some((text, x, y)) = &app.hover {
        draw_tooltip(f, text, *x, *y);
    }
    if app.first_run {
        app.first_run_rect = draw_first_run_popup(f);
    }
}

/// A small bordered popup near the mouse showing the full (possibly clipped)
/// cell content. Text wraps so long cmdlines/paths are fully readable.
fn draw_tooltip(f: &mut Frame, text: &str, x: u16, y: u16) {
    let area = f.area();
    let mut w = (text.chars().count() as u16 + 2).min(60);
    w = w.min(area.width.saturating_sub(2));
    let inner_w = (w as usize).saturating_sub(2).max(1);
    let lines = text.chars().count().div_ceil(inner_w) + 1;
    let h = (2 + lines as u16).min(area.height);
    if w < 6 || h < 3 {
        return;
    }
    let px = (x + 1).min(area.width.saturating_sub(w));
    let py = (y + 1).min(area.height.saturating_sub(h));
    let rect = Rect::new(px, py, w, h);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(Text::from(text.to_string()))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White))
            .block(Block::bordered().border_style(Style::default().fg(Color::DarkGray))),
        rect,
    );
}

fn draw_header(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line = vec![
        Span::styled("Worker Filter: ", Style::default().fg(Color::Cyan)),
        Span::styled(
            format!("\"{}\"", app.name_input),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    let p = Paragraph::new(Line::from(line));
    f.render_widget(p, area);
    if let Some(s) = &app.snapshot {
        let stats = format!(
            "cores {} · mem {} · procs {}",
            s.cores,
            fmt_bytes(s.mem_total),
            s.scanned
        );
        let (left, style) = if app.paused {
            (format!("●  {stats}"), Color::Cyan)
        } else if s.collecting {
            (format!("rec {}  {stats}", fmt_clock(s.rec_secs)), Color::DarkGray)
        } else {
            (format!("not recording  {stats}"), Color::Yellow)
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(left, style)]))
                .alignment(Alignment::Right),
            area,
        );
    }
    if app.offline {
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "daemon offline",
                Style::default().fg(Color::Red).bold(),
            )]))
            .alignment(Alignment::Right),
            area,
        );
    }
}

fn draw_footer(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let keys = [
        ("f", "update filter"),
        ("v", if app.live { "overall" } else { "live" }),
        ("t", "go stealth"),
        ("s", "save"),
        ("l", "load"),
        ("r", if app.paused { "live" } else { "restart" }),
        ("esc", "clear selection"),
        ("d", "detach"),
        ("q", "terminate"),
        ("h", "help"),
    ];
    let mut spans: Vec<Span> = Vec::new();
    for (k, label) in keys {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::default().fg(Color::Cyan).bold(),
        ));
        spans.push(Span::styled(
            format!("{label}  "),
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    if let Some((msg, t)) = &app.flash
        && t.elapsed() < Duration::from_secs(5)
    {
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                msg.clone(),
                Style::default().fg(Color::Yellow),
            )]))
            .alignment(Alignment::Right),
            area,
        );
    }
}

fn draw_popup(f: &mut Frame, app: &mut App) {
    let Some(p) = &mut app.popup else {
        return;
    };
    let area = f.area();
    let w = area.width.saturating_sub(4).min(76);
    let h = area.height.saturating_sub(4).min(24);
    if w < 40 || h < 12 {
        return;
    }
    let pop = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, pop);
    let block = Block::bordered().title(" define worker filters (regex) ");
    let inner = block.inner(pop);
    f.render_widget(block, pop);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(inner);

    // filter field with grey example placeholder
    let input_span = if p.input.is_empty() {
        Span::styled(
            "runner\\.py.*",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::styled(p.input.clone(), Style::default().fg(Color::White))
    };
    let filter_line = Line::from(vec![
        Span::styled("filter: ", Style::default().fg(Color::Cyan)),
        input_span,
        Span::styled(
            if p.input.is_empty() && !p.exclude_focused {
                "   (example)"
            } else {
                ""
            },
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(filter_line), rows[0]);
    p.include_area = rows[0];
    if !p.exclude_focused {
        f.set_cursor_position(Position::new(inner.x + 8 + p.cursor as u16, rows[0].y));
    }

    // exclude field with grey example placeholder
    let exc_span = if p.exc_input.is_empty() {
        Span::styled("vim", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(p.exc_input.clone(), Style::default().fg(Color::White))
    };
    let excl_line = Line::from(vec![
        Span::styled("exclude: ", Style::default().fg(Color::Cyan)),
        exc_span,
        Span::styled(
            if p.exc_input.is_empty() && p.exclude_focused {
                "   (example)"
            } else {
                ""
            },
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(excl_line), rows[1]);
    p.exclude_area = rows[1];
    if p.exclude_focused {
        f.set_cursor_position(Position::new(inner.x + 9 + p.exc_cursor as u16, rows[1].y));
    }

    // preview line
    let info = match &p.error {
        Some(e) => Span::styled(e.clone(), Style::default().fg(Color::Red)),
        None => Span::styled(
            format!("matching now: {}", p.matches.len()),
            Style::default().fg(Color::DarkGray),
        ),
    };
    f.render_widget(Paragraph::new(Line::from(vec![info])), rows[2]);

    // matched experiment runners
    let marea = rows[3];
    let n = marea.height as usize;
    for (i, m) in p.matches.iter().take(n).enumerate() {
        let line = format!(
            " {} {:>7} {:12} {}",
            m.pid,
            trunc(&m.user, 7),
            trunc(&m.comm, 12),
            trunc(&m.cmdline, marea.width as usize - 24)
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                line,
                Style::default().fg(Color::White),
            ))),
            Rect::new(marea.x, marea.y + i as u16, marea.width, 1),
        );
    }
    if p.matches.len() > n {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" … {} more", p.matches.len() - n),
                Style::default().fg(Color::DarkGray),
            ))),
            Rect::new(marea.x, marea.y + n as u16, marea.width, 1),
        );
    }

    let confirm = Span::styled(" [ confirm ] ", Style::default().fg(Color::Cyan).bold());
    let cancel = Span::styled(" [ cancel ] ", Style::default().fg(Color::DarkGray));
    let btns = Line::from(vec![Span::raw(" "), confirm, cancel]);
    f.render_widget(Paragraph::new(btns), rows[4]);
    p.confirm_area = Rect::new(inner.x + 1, rows[4].y, 13, 1);
    p.cancel_area = Rect::new(inner.x + 15, rows[4].y, 11, 1);
}

const STEALTH_DEFAULTS: [&str; 6] = ["htop", "nvtop", "glances", "btop", "top", "screen"];

fn draw_stealth(f: &mut Frame, app: &mut App) {
    let Some(st) = &mut app.stealth else {
        return;
    };
    let area = f.area();
    let w = area.width.saturating_sub(4).min(76);
    let h = 12.min(area.height.saturating_sub(2));
    if w < 40 || h < 10 {
        return;
    }
    let pop = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, pop);
    let block = Block::bordered().title(" stealth mode ");
    let inner = block.inner(pop);
    f.render_widget(block, pop);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "rename our processes (server-spy) to look innocuous:",
            Style::default().fg(Color::White),
        ))),
        rows[0],
    );
    let input_line = Line::from(vec![
        Span::styled("name: ", Style::default().fg(Color::Cyan)),
        Span::styled(st.input.clone(), Style::default().fg(Color::White)),
    ]);
    f.render_widget(Paragraph::new(input_line), rows[1]);
    f.set_cursor_position(Position::new(inner.x + 6 + st.cursor as u16, rows[1].y));

    let mut spans = vec![Span::styled(
        "defaults:  ",
        Style::default().fg(Color::DarkGray),
    )];
    st.default_areas.clear();
    let mut x = inner.x + "defaults:  ".len() as u16;
    for d in STEALTH_DEFAULTS {
        let active = st.input == d;
        let span = if active {
            Span::styled(format!(" {d} "), Style::default().fg(Color::Cyan).bold())
        } else {
            Span::styled(format!(" {d} "), Style::default().fg(Color::DarkGray))
        };
        let w = d.len() as u16 + 2;
        st.default_areas.push((d.to_string(), Rect::new(x, rows[2].y, w, 1)));
        x += w + 1;
        spans.push(span);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), rows[2]);

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "applies to this TUI and the daemon — ps/top will show the new name",
            Style::default().fg(Color::DarkGray),
        ))),
        rows[3],
    );

    let confirm = Span::styled(" [ confirm ] ", Style::default().fg(Color::Cyan).bold());
    let cancel = Span::styled(" [ cancel ] ", Style::default().fg(Color::DarkGray));
    let btns = Line::from(vec![Span::raw(" "), confirm, cancel]);
    f.render_widget(Paragraph::new(btns), rows[4]);
    st.confirm_area = Rect::new(inner.x + 1, rows[4].y, 12, 1);
    st.cancel_area = Rect::new(inner.x + 14, rows[4].y, 11, 1);
}

fn draw_browser(f: &mut Frame, app: &mut App) {
    let Some(b) = &mut app.browser else {
        return;
    };
    let area = f.area();
    let w = area.width.saturating_sub(4).min(80);
    let h = area.height.saturating_sub(4).min(24);
    if w < 40 || h < 10 {
        return;
    }
    let pop = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, pop);
    let title = match b.mode {
        InputMode::Save => " save snapshot ",
        InputMode::Load => " load snapshot ",
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(pop);
    f.render_widget(block, pop);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let dir_s = b.dir.to_string_lossy().into_owned();
    let hint = if b.list_focused {
        "  tab: name  ·  j/k: move  ·  enter: open  ·  esc: close"
    } else {
        "  tab: list  ·  enter: confirm  ·  esc: close"
    };
    let dir_line = Line::from(vec![
        Span::styled(
            trunc(&dir_s, inner.width as usize - hint.len()),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(hint, Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(dir_line), rows[0]);

    b.list_area = rows[1];
    let visible = b.list_area.height as usize;
    if b.sel < b.scroll {
        b.scroll = b.sel;
    } else if b.sel >= b.scroll + visible {
        b.scroll = b.sel - visible + 1;
    }
    for (i, (name, is_dir)) in b
        .entries
        .iter()
        .skip(b.scroll)
        .take(visible)
        .enumerate()
    {
        let idx = b.scroll + i;
        let text = if *is_dir {
            format!(" {}/", trunc(name, b.list_area.width as usize - 2))
        } else {
            format!("  {}", trunc(name, b.list_area.width as usize - 2))
        };
        let style = if idx == b.sel {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if *is_dir {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style))),
            Rect::new(b.list_area.x, b.list_area.y + i as u16, b.list_area.width, 1),
        );
    }

    b.name_area = rows[2];
    let name_style = if b.list_focused {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };
    let name_line = Line::from(vec![
        Span::styled("name: ", Style::default().fg(Color::Cyan)),
        Span::styled(b.name.clone(), name_style),
    ]);
    f.render_widget(Paragraph::new(name_line), b.name_area);
    if !b.list_focused {
        f.set_cursor_position(Position::new(
            b.name_area.x + 6 + b.name_cursor as u16,
            b.name_area.y,
        ));
    }

    let confirm = Span::styled(" [ confirm ] ", Style::default().fg(Color::Cyan).bold());
    let cancel = Span::styled(" [ cancel ] ", Style::default().fg(Color::DarkGray));
    let btns = Line::from(vec![Span::raw(" "), confirm, cancel]);
    f.render_widget(Paragraph::new(btns), rows[3]);
    b.confirm_area = Rect::new(inner.x + 1, rows[3].y, 12, 1);
    b.cancel_area = Rect::new(inner.x + 14, rows[3].y, 11, 1);
}

/// Splits the middle-left column height among congestion (17), utilization
/// (12) and statistics (8). Each pane keeps its content minimum and receives
/// an equal share of any excess height (remainder goes to congestion).
fn left_col_heights(h: u16) -> [u16; 3] {
    let mins = [17u16, 12, 8];
    let sum: u16 = mins.iter().sum();
    if h <= sum {
        return mins;
    }
    let excess = h - sum;
    let each = excess / 3;
    let rem = excess % 3;
    [mins[0] + each + rem, mins[1] + each, mins[2] + each]
}

fn draw_middle(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cols =
        Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)]).split(area);
    // Each pane keeps its content minimum; any excess height is shared
    // equally among all three.
    let [h0, h1, h2] = left_col_heights(cols[0].height);
    let left = Layout::vertical([
        Constraint::Length(h0),
        Constraint::Length(h1),
        Constraint::Length(h2),
    ])
    .split(cols[0]);
    draw_psi(f, app, left[0]);
    draw_util(f, app, left[1]);
    draw_stats(f, app, left[2]);
    draw_runs(f, app, cols[1]);
}

fn draw_psi(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::bordered().title(" Live congestion ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(s) = &app.snapshot else {
        return;
    };
    if inner.height < 3 {
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .split(inner);
    let items = [
        (
            "congestion",
            system_congestion_index(
                s.psi_pct.cpu_some,
                s.psi_pct.mem_some,
                s.psi_pct.io_some,
                s.sys_wait.unwrap_or(0.0),
            ),
            Color::Gray,
        ),
        ("cpu pressure", s.psi_pct.cpu_some, Color::Gray),
        ("mem pressure", s.psi_pct.mem_some, Color::Gray),
        ("io pressure", s.psi_pct.io_some, Color::Gray),
        ("sched wait", s.sys_wait.unwrap_or(0.0), Color::Gray),
    ];
    for (i, (label, cur, color)) in items.iter().enumerate() {
        if rows[i].height < 3 {
            continue;
        }
        let frame = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                format!(" {label} "),
                Style::default().fg(Color::Cyan),
            ));
        let binner = frame.inner(rows[i]);
        f.render_widget(frame, rows[i]);
        f.render_widget(bar_value(*cur, *color, binner.width), binner);
    }
}

/// Path of the "first run" marker, per user in the XDG state directory.
fn first_run_marker_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state"))
        })
        .unwrap_or_default();
    base.join("server-spy").join("first-run")
}

/// Whether this user has never run the TUI on this device before.
fn first_run_marker_missing() -> bool {
    !first_run_marker_path().exists()
}

/// The welcome notice only shows for real users: the demo recording and the
/// demo/benchmark scenario run with SERVER_SPY_DEMO=1 and must never see it.
fn first_run_notice_wanted() -> bool {
    std::env::var("SERVER_SPY_DEMO").is_err() && first_run_marker_missing()
}

/// Records that the first-run notice was shown (no-op under tests so the
/// developer's own first-run experience is not consumed by the test suite).
fn mark_first_run_done() {
    if cfg!(test) {
        return;
    }
    let p = first_run_marker_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, "1");
}

/// First-run welcome overlay pointing new users at the help mode. Returns the
/// popup rect so clicks can target the [ got it ] button.
fn draw_first_run_popup(f: &mut Frame) -> Option<Rect> {
    let area = f.area();
    let w = area.width.saturating_sub(4).min(54);
    let h = 9u16;
    if w < 40 || area.height < h + 4 {
        return None;
    }
    let pop = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, pop);
    let block = Block::bordered().title(" welcome to server-spy ");
    let inner = block.inner(pop);
    f.render_widget(block, pop);
    let lines = [
        "Get familiar with the scores and numbers:",
        "press h for an explanation of every metric,",
        "column and click action. A mouse move won't",
        "close this — click [ got it ] or press any",
        "key to dismiss (it won't reappear).",
        "",
        "  [ got it ]  ",
    ];
    for (i, l) in lines.iter().enumerate() {
        let style = if i == 6 {
            Style::default().fg(Color::Cyan).bold()
        } else {
            Style::default().fg(Color::White)
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(*l, style))),
            Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
        );
    }
    Some(pop)
}

/// The clickable [ got it ] row inside the first-run popup.
fn first_run_got_it_rect(pop: Rect) -> Rect {
    let inner = pop.inner(Margin { horizontal: 1, vertical: 1 });
    Rect::new(inner.x, inner.y + 6, 14, 1)
}

/// A horizontal bar with the value printed at its right end. The value is
/// padded to a fixed field width so every bar ends at the same column, and
/// there is no trailing color beyond the fill (like the utilization bars).
fn bar_value(cur: f64, color: Color, width: u16) -> Line<'static> {
    let value = fmt_pct(cur);
    let val_w = 8usize;
    let bar_w = (width as usize).saturating_sub(val_w + 1);
    let mut fill = (bar_w as f64 * (cur / 100.0).clamp(0.0, 1.0).sqrt()).round() as usize;
    if cur > 0.0 && fill == 0 {
        fill = 1;
    }
    let spans = vec![
        Span::styled("█".repeat(fill), Style::default().fg(color)),
        Span::raw(" ".repeat(bar_w.saturating_sub(fill))),
        Span::raw(" "),
        Span::styled(
            format!("{value:>val_w$}"),
            Style::default().fg(color).bold(),
        ),
    ];
    Line::from(spans)
}

/// Conditions-consistency statistics: a compact table of the most telling
/// distribution numbers per metric, with a one-line LaTeX copy control at
/// the bottom. Kept as small as possible so Live congestion gets the space.
fn draw_stats(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height < 7 {
        return;
    }
    let block = Block::bordered().title(" Statistics · server conditions ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(s) = &app.snapshot else {
        return;
    };
    let c = &s.conditions;
    if c.n == 0 {
        f.render_widget(
            Paragraph::new("no completed runs yet")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }
    // header, up to three metric rows, a spacer, then the latex copy row; the
    // metric column grows so the table spans the full pane width
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let header = Line::from(vec![
        Span::styled(format!("{:<7}", "metric"), Style::default().fg(Color::Cyan).bold()),
        Span::styled(format!("{:>4}", "n"), Style::default().fg(Color::Cyan).bold()),
        Span::styled(format!("{:>6}", "med"), Style::default().fg(Color::Cyan).bold()),
        Span::styled(format!("{:>6}", "MAD%"), Style::default().fg(Color::Cyan).bold()),
        Span::styled(format!("{:>7}", "max"), Style::default().fg(Color::Cyan).bold()),
    ]);
    f.render_widget(Paragraph::new(header), rows[0]);
    for (i, (name, d)) in [
        (1usize, ("ci", &c.ci)),
        (2, ("cl", &c.cl)),
        (3, ("wait%", &c.wait)),
    ] {
        if let Some(d) = d {
            let line = Line::from(vec![
                Span::styled(format!("{name:<7}"), Style::default().fg(Color::White)),
                Span::styled(format!("{:>4}", d.n), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:>6}", fmt_num(d.median)),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("{:>6}", fmt_num(d.mad_rel)),
                    Style::default().fg(sev(d.mad_rel)),
                ),
                Span::styled(
                    format!("{:>7}", fmt_num(d.max)),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            f.render_widget(Paragraph::new(line), rows[i]);
        }
    }
    // one left-aligned sentence at the bottom; the bracketed words are the
    // clickable copy buttons
    let btn_row = rows[5];
    let line = Line::from(vec![
        Span::styled(
            "copy latex report ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("[sentence]", Style::default().fg(Color::Cyan).bold()),
        Span::raw(" or "),
        Span::styled("[table]", Style::default().fg(Color::Cyan).bold()),
    ]);
    f.render_widget(Paragraph::new(line), btn_row);
    app.stat_copy_area = Rect::new(inner.x + 18, btn_row.y, 10, 1);
    app.stat_table_area = Rect::new(inner.x + 32, btn_row.y, 7, 1);
}

/// Copies the LaTeX sentence (table=false) or table (table=true) for the
/// current conditions summary to the system clipboard.
fn copy_conditions(app: &mut App, table: bool) {
    let Some(s) = &app.snapshot else {
        return;
    };
    if s.conditions.n == 0 {
        app.flash = Some(("no completed runs to summarize".into(), Instant::now()));
        return;
    }
    let text = if table {
        crate::conditions::latex_table(&s.conditions)
    } else {
        crate::conditions::latex_sentence(&s.conditions)
    };
    if copy_to_clipboard(&text) {
        app.flash = Some((
            if table {
                "copied latex table to clipboard"
            } else {
                "copied latex sentence to clipboard"
            }
            .into(),
            Instant::now(),
        ));
    } else {
        app.flash = Some((
            "clipboard unavailable (need wl-copy, xclip or xsel)".into(),
            Instant::now(),
        ));
    }
}

/// Writes text to the system clipboard via whatever clipboard tool is
/// installed (Wayland, X11 or generic). Each attempt is bounded by a 2s
/// watchdog so a missing display server can never freeze the UI.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::sync::mpsc;
    for (cmd, args) in [
        ("wl-copy", &[][..]),
        ("xclip", &["-selection", "clipboard"][..]),
        ("xsel", &["--clipboard", "--input"][..]),
    ] {
        let Ok(mut child) = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn()
        else {
            continue;
        };
        let wrote = child
            .stdin
            .as_mut()
            .map(|s| s.write_all(text.as_bytes()).is_ok())
            .unwrap_or(false);
        // close the pipe so the tool sees EOF and proceeds
        drop(child.stdin.take());
        if !wrote {
            continue;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = child.wait();
            let _ = tx.send(());
        });
        if rx.recv_timeout(Duration::from_secs(2)).is_ok() {
            return true;
        }
    }
    false
}

fn draw_util(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::bordered().title(" Live Resource utilization ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(s) = &app.snapshot else {
        return;
    };
    if inner.height < 3 {
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .split(inner);
    let titles = ["cpu%", "mem%", "io%"];
    let bars = [
        &[(s.share_cpu[0], Color::Green), (s.share_cpu[1], Color::Red)][..],
        &[(s.share_mem[0], Color::Green), (s.share_mem[1], Color::Red)][..],
        &[(s.psi_pct.io_some, Color::Red)][..],
    ];
    for i in 0..3 {
        if rows[i].height < 3 {
            continue;
        }
        let frame = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                format!(" {} ", titles[i]),
                Style::default().fg(Color::Cyan),
            ));
        let binner = frame.inner(rows[i]);
        f.render_widget(frame, rows[i]);
        f.render_widget(bar_fill(bars[i], binner.width), binner);
    }
    let legend = Line::from(vec![
        Span::styled("■ ", Style::default().fg(Color::Green)),
        Span::styled("target Workers  ", Style::default().fg(Color::DarkGray)),
        Span::styled("■ ", Style::default().fg(Color::Red)),
        Span::styled("other processes", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(legend), rows[3]);
}

fn bar_fill(segs: &[(f64, Color)], width: u16) -> Line<'static> {
    let bar_w = width as usize;
    let mut spans = Vec::new();
    let mut px = 0usize;
    for (pct, color) in segs {
        let mut n = (bar_w as f64 * (pct / 100.0).clamp(0.0, 1.0).sqrt()).round() as usize;
        if *pct > 0.0 && n == 0 {
            n = 1;
        }
        px += n;
        spans.push(Span::styled("█".repeat(n), Style::default().fg(*color)));
    }
    spans.push(Span::raw(" ".repeat(bar_w.saturating_sub(px))));
    Line::from(spans)
}

/// One annotation line: a cyan arrow pointing at the explained column/bar.
/// Truncates to fit a single row (used inside the small bar boxes).
fn help_line_w(arrow: &str, head: &str, rest: &str, width: usize) -> Line<'static> {
    let rest = trunc(rest, width.saturating_sub(arrow.len() + head.len() + 6));
    Line::from(vec![
        Span::styled(
            format!("{arrow} "),
            Style::default().fg(Color::Cyan).bold(),
        ),
        Span::styled(head.to_string(), Style::default().fg(Color::White).bold()),
        Span::styled(format!(" — {rest}"), Style::default().fg(Color::DarkGray)),
    ])
}

/// Untruncated annotation; the renderer wraps long lines over extra rows.
fn help_line_full(arrow: &str, head: &str, rest: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{arrow} "),
            Style::default().fg(Color::Cyan).bold(),
        ),
        Span::styled(head.to_string(), Style::default().fg(Color::White).bold()),
        Span::styled(format!(" — {rest}"), Style::default().fg(Color::DarkGray)),
    ])
}

/// An action hint (click/tab interactions): magenta arrow + head so it reads
/// clearly as something you can do, distinct from the column explanations.
fn action_line(head: &str, rest: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("▶ ", Style::default().fg(Color::Magenta).bold()),
        Span::styled(head.to_string(), Style::default().fg(Color::Magenta).bold()),
        Span::styled(format!(" — {rest}"), Style::default().fg(Color::DarkGray)),
    ])
}

/// How many rows a wrapped help line needs at the given width.
fn line_span(line: &Line, width: usize) -> u16 {
    (line.width().div_ceil(width.max(1)) as u16).clamp(1, 3)
}

/// Help overlay: the bare-bone TUI structure with column names and arrow
/// annotations explaining every value, bar and interaction. No data is shown.
fn draw_help(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(13),
        Constraint::Length(1),
    ])
    .split(area);
    draw_help_header(f, app, chunks[0]);
    draw_help_middle(f, chunks[1]);
    draw_help_lists(f, chunks[2]);
    draw_help_footer(f, chunks[3]);
}

fn draw_help_header(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line = vec![
        Span::styled("Worker Filter: ", Style::default().fg(Color::Cyan)),
        Span::styled(
            format!("\"{}\"", app.name_input),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            "  ← regex filter that defines which processes are experiments",
            Style::default().fg(Color::DarkGray),
        ),
    ];
    f.render_widget(Paragraph::new(Line::from(line)), area);
}

fn draw_help_middle(f: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cols =
        Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)]).split(area);
    let [h0, h1, h2] = left_col_heights(cols[0].height);
    let left = Layout::vertical([
        Constraint::Length(h0),
        Constraint::Length(h1),
        Constraint::Length(h2),
    ])
    .split(cols[0]);
    draw_help_congestion(f, left[0]);
    draw_help_util(f, left[1]);
    draw_help_stats(f, left[2]);
    draw_help_runs(f, cols[1]);
}

fn draw_help_stats(f: &mut Frame, area: Rect) {
    let block = Block::bordered().title(" Statistics · server conditions ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = [
        help_line_full("↓", "med", "typical value across completed runs"),
        help_line_full("↓", "MAD±", "typical run-to-run deviation"),
        action_line(
            "copy latex",
            "paper sentence / table via the clipboard",
        ),
    ];
    let mut y = inner.y;
    for line in lines {
        if y >= inner.y + inner.height {
            break;
        }
        let span = line_span(&line, inner.width as usize);
        f.render_widget(
            Paragraph::new(line).wrap(Wrap { trim: false }),
            Rect::new(inner.x, y, inner.width, span),
        );
        y += span;
    }
}

fn draw_help_congestion(f: &mut Frame, area: Rect) {
    let block = Block::bordered().title(" Live congestion ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let items = [
        ("congestion", "0-100 saturating index of current system pressure"),
        ("cpu pressure", "PSI cpu stalls in the last interval"),
        ("mem pressure", "PSI memory stalls in the last interval"),
        ("io pressure", "PSI io stalls in the last interval"),
        ("sched wait", "scheduler runqueue wait overhead"),
    ];
    for (i, (title, text)) in items.iter().enumerate() {
        let y = inner.y + i as u16 * 2;
        if y + 1 >= inner.y + inner.height {
            break;
        }
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {title} "),
                Style::default().fg(Color::Cyan),
            ))),
            Rect::new(inner.x, y, inner.width, 1),
        );
        f.render_widget(
            Paragraph::new(help_line_w("↑", title, text, inner.width as usize)),
            Rect::new(inner.x, y + 1, inner.width, 1),
        );
    }
}

fn draw_help_util(f: &mut Frame, area: Rect) {
    let block = Block::bordered().title(" Live Resource utilization ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 3 {
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .split(inner);
    let items = [
        ("cpu%", "green = your runs · red = everyone else"),
        ("mem%", "green = your runs · red = everyone else"),
        ("io%", "share of the io pressure"),
    ];
    for (i, (title, text)) in items.iter().enumerate() {
        if rows[i].height < 3 {
            continue;
        }
        let frame = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                format!(" {title} "),
                Style::default().fg(Color::Cyan),
            ));
        let binner = frame.inner(rows[i]);
        f.render_widget(frame, rows[i]);
        f.render_widget(Paragraph::new(help_line_w("↑", title, text, binner.width as usize)), binner);
    }
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "↑ ",
                Style::default().fg(Color::Cyan).bold(),
            ),
            Span::styled("legend", Style::default().fg(Color::White).bold()),
            Span::styled(
                " — green = target workers · red = other processes",
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        rows[3],
    );
    f.render_widget(
        Paragraph::new(help_line_w(
            "↑",
            "scope",
            "cpu% counts only the cores your runs actually use",
            rows[3].width as usize,
        )),
        Rect::new(rows[3].x, rows[3].y + 1, rows[3].width, rows[3].height.saturating_sub(1)),
    );
}


/// Runs-table column widths: every fixed column keeps its size and the params
/// column absorbs the remaining pane width, so `state` sits flush at the right
/// edge and longer command lines stay visible. One entry per header column.
fn runs_widths(total: u16) -> Vec<Constraint> {
    let fixed: u16 = 12 + 6 + 6 + 6 + 8 + 8 + 8 + 7 + 8;
    let params = total.saturating_sub(fixed + 9).max(20);
    vec![
        Constraint::Length(params),
        Constraint::Length(12),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Length(8),
    ]
}

fn draw_help_runs(f: &mut Frame, area: Rect) {
    let block = Block::bordered().title(" Experiment Runs ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let names = [
        "params", "congestion", "cpu%", "mem%", "io%", "wait%", "util%", "wall", "usr", "state",
    ];
    let header = Row::new(names.iter().map(|h| h.to_string()).collect::<Vec<String>>())
        .style(Style::default().fg(Color::Cyan).bold());
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
    let widths = runs_widths(rows[0].width);
    f.render_widget(
        Table::new(Vec::<Row>::new(), widths).header(header),
        rows[0],
    );
    let w = rows[1].width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (head, text) in [
        ("params", "command line of the experiment run"),
        (
            "congestion",
            "congestion index (0-100): the run's psi pressures + scheduler wait, compressed into one saturating number — 0 = idle, 50 = half saturated",
        ),
        ("cpu%", "share of the run's congestion caused by CPU"),
        ("mem%", "share of the run's congestion caused by memory"),
        ("io%", "share of the run's congestion caused by io"),
        (
            "wait%",
            "runqueue wait vs CPU work — 100% means the run waited just as long as it actually worked",
        ),
        ("util%", "how busy the run's processes are, scaled to the cores your runs use"),
        ("wall", "total wall-clock time of the run"),
        ("usr", "max other users active during the run"),
        ("state", "◔ running · ✓ finished"),
    ] {
        if head.is_empty() {
            lines.push(Line::from(Span::styled(
                text,
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            lines.push(help_line_full("↓", head, text));
        }
    }
    lines.push(action_line(
        "click to associate a run",
        "to its users/processes — they light up in the lists below",
    ));
    let mut y = rows[1].y;
    for line in lines {
        let span = line_span(&line, w);
        if y + span > inner.y + inner.height {
            break;
        }
        f.render_widget(
            Paragraph::new(line).wrap(Wrap { trim: false }),
            Rect::new(rows[1].x, y, rows[1].width, span),
        );
        y += span;
    }
}

fn draw_help_lists(f: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cols = Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)]).split(area);
    let ublock = Block::bordered().title(Line::from(vec![
        Span::styled(" Other users ", Style::default()),
        Span::styled(" [overall] [live] ", Style::default().fg(Color::DarkGray)),
    ]));
    let uinner = ublock.inner(cols[0]);
    f.render_widget(ublock, cols[0]);
    let urows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(uinner);
    let unames = ["#", "user", "util%", "wait%", "runs", "share"];
    let uheader = Row::new(unames.iter().map(|h| h.to_string()).collect::<Vec<String>>())
        .style(Style::default().fg(Color::Cyan).bold());
    let uwidths = [
        Constraint::Length(3),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Length(8),
    ];
    f.render_widget(
        Table::new(Vec::<Row>::new(), uwidths).header(uheader).flex(Flex::Start),
        urows[0],
    );
    let uw = urows[1].width as usize;
    let mut ulines: Vec<Line<'static>> = Vec::new();
    for (head, text) in [
        ("user", "users consuming resources"),
        ("util%", "their average core usage"),
        ("wait%", "wait vs CPU work — high = congested machine"),
        ("runs", "runs they were active in"),
        ("share", "share of the other-load"),
    ] {
        ulines.push(help_line_full("↓", head, text));
    }
    ulines.push(action_line(
        "[overall] / [live]",
        "overall vs live (v)",
    ));
    ulines.push(action_line(
        "click to associate a user",
        "to the runs it affected (above)",
    ));
    let mut y = urows[1].y;
    for line in ulines {
        let span = line_span(&line, uw);
        if y + span > uinner.y + uinner.height {
            break;
        }
        f.render_widget(
            Paragraph::new(line).wrap(Wrap { trim: false }),
            Rect::new(urows[1].x, y, urows[1].width, span),
        );
        y += span;
    }

    let ablock = Block::bordered().title(Line::from(vec![
        Span::styled(" Other Processes ", Style::default()),
        Span::styled(" [overall] [live] ", Style::default().fg(Color::DarkGray)),
    ]));
    let ainner = ablock.inner(cols[1]);
    f.render_widget(ablock, cols[1]);
    let arows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(ainner);
    let anames = ["#", "user", "comm", "util%", "wait%", "runs", "cmdline"];
    let aheader = Row::new(anames.iter().map(|h| h.to_string()).collect::<Vec<String>>())
        .style(Style::default().fg(Color::Cyan).bold());
    let awidths = [
        Constraint::Length(3),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Min(10),
    ];
    f.render_widget(
        Table::new(Vec::<Row>::new(), awidths).header(aheader),
        arows[0],
    );
    let aw = arows[1].width as usize;
    let mut alines: Vec<Line<'static>> = Vec::new();
    for (head, text) in [
        ("user", "owner of the process"),
        ("comm", "process name"),
        ("util%", "their average core usage"),
        ("wait%", "their wait vs CPU work — high = congested machine"),
        ("runs", "in how many of your runs it was active"),
        ("cmdline", "full command — hover for tooltip"),
    ] {
        alines.push(help_line_full("↓", head, text));
    }
    alines.push(action_line(
        "click to associate a process",
        "to the runs it affected (they light up above)",
    ));
    let mut y = arows[1].y;
    for line in alines {
        let span = line_span(&line, aw);
        if y + span > ainner.y + ainner.height {
            break;
        }
        f.render_widget(
            Paragraph::new(line).wrap(Wrap { trim: false }),
            Rect::new(arows[1].x, y, arows[1].width, span),
        );
        y += span;
    }
}

fn draw_help_footer(f: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let spans = vec![
        Span::styled(" h ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("hide help", Style::default().fg(Color::DarkGray)),
        Span::styled("  ·  q ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("terminate", Style::default().fg(Color::DarkGray)),
        Span::styled("  ·  d ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("detach", Style::default().fg(Color::DarkGray)),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_runs(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let title = match (&app.snapshot, &app.highlight) {
        (Some(s), Some(h)) => match h {
            Highlight::Run(order) => {
                let params = s
                    .runs
                    .iter()
                    .find(|r| r.order == *order)
                    .map(|r| r.params.clone())
                    .unwrap_or_default();
                format!(" Experiment Runs — selected: {} ", trunc(&params, 30))
            }
            _ => {
                let n = s.runs.iter().filter(|r| run_affected(r, h)).count();
                let what = match h {
                    Highlight::User(u) => format!("user \"{u}\""),
                    Highlight::Ant { pid, comm } => {
                        if *pid < 0 {
                            format!("proc {comm}")
                        } else {
                            format!("proc {comm} (pid {pid})")
                        }
                    }
                    Highlight::Run(_) => unreachable!(),
                };
                format!(" Experiment Runs — affected by {what}: {n} ")
            }
        },
        _ => " Experiment Runs ".to_string(),
    };
    let title = if app.highlight.is_none() {
        Line::from(vec![
            Span::styled(title, Style::default()),
            Span::styled(
                " · click a run to highlight its users & procs ",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(Span::styled(title, Style::default()))
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let table_area = Rect::new(inner.x, inner.y, inner.width.saturating_sub(1), inner.height);
    app.runs_area = table_area;
    let run_len = app.snapshot.as_ref().map(|s| s.runs.len()).unwrap_or(0);
    let visible = inner.height.saturating_sub(1).max(1) as usize;
    if run_len > visible {
        app.scroll = app.scroll.min(run_len - visible);
    } else {
        app.scroll = 0;
    }
    let Some(s) = &app.snapshot else {
        return;
    };
    if s.runs.is_empty() {
        if s.target.is_empty() {
            let btn_w = 34u16;
            let btn_h = 3u16;
            let bx = inner.x + inner.width.saturating_sub(btn_w) / 2;
            let by = inner.y + inner.height.saturating_sub(btn_h) / 2;
            app.filter_btn_area = Rect::new(bx, by, btn_w, btn_h);
            f.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    "click or press f",
                    Style::default().fg(Color::DarkGray),
                )]))
                .block(
                    Block::bordered().title(Span::styled(
                        " define worker filter ",
                        Style::default().fg(Color::Cyan).bold(),
                    )),
                )
                .alignment(Alignment::Center),
                app.filter_btn_area,
            );
        } else {
            f.render_widget(
                Paragraph::new("no experiment runs detected yet")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
        }
        return;
    }
    let mut sorted: Vec<&RunRow> = s.runs.iter().collect();
    sort_runs(&mut sorted, app.runs_sort);
    let mut headers: Vec<String> = [
        "params", "congestion", "cpu%", "mem%", "io%", "wait%", "util%", "wall", "usr", "state",
    ]
    .iter()
    .map(|h| h.to_string())
    .collect();
    headers[app.runs_sort.0] = format!(
        "{}{}",
        headers[app.runs_sort.0],
        if app.runs_sort.1 { " ▲" } else { " ▼" }
    );
    let header = Row::new(headers).style(Style::default().fg(Color::Cyan).bold());
    let widths = runs_widths(table_area.width);
    let [_sel, columns_area] = Layout::horizontal([Constraint::Length(0), Constraint::Fill(0)])
        .areas(Rect::new(0, 0, table_area.width, 1));
    let col_rects = Layout::horizontal(widths.clone())
        .flex(Flex::Start)
        .spacing(1)
        .split(columns_area);
    let params_cap = col_rects[0].width.saturating_sub(2).max(4) as usize;
    let rows: Vec<Row> = sorted
        .iter()
        .skip(app.scroll)
        .take(visible)
        .map(|r| {
            let hl = app.highlight.as_ref();
            let hl_on = hl.map(|h| run_affected(r, h)).unwrap_or(false);
            // Affected rows get the highlight background; every other row is
            // temporarily desaturated so the affected set stands out.
            let fg = |s: Style| {
                if hl_on {
                    s.fg(Color::White)
                } else if hl.is_some() {
                    s.fg(Color::Gray)
                } else {
                    s
                }
            };
            let row_style = if hl_on {
                Style::default().fg(Color::White).bg(HIGHLIGHT_BG)
            } else if hl.is_some() {
                Style::default().fg(Color::Gray)
            } else {
                Style::default()
            };
            let st = if r.alive {
                Span::styled("◔", fg(Style::default().fg(Color::Yellow)))
            } else {
                Span::styled("✓", fg(Style::default().fg(Color::Green)))
            };
            Row::new(vec![
                Cell::from(Span::styled(
                    trunc(&r.params, params_cap),
                    if hl_on {
                        Style::default().fg(Color::White).bold()
                    } else if hl.is_some() {
                        Style::default().fg(Color::Gray)
                    } else {
                        Style::default()
                    },
                )),
                Cell::from(Span::styled(
                    fmt_pct(run_ci(r)),
                    fg(Style::default().fg(sev(run_ci(r)))),
                )),
                Cell::from(Span::styled(
                    match run_attr(r) {
                        Some((c, _, _)) => format!("{c:.0}"),
                        None => "–".to_string(),
                    },
                    fg(match run_attr(r) {
                        Some((c, _, _)) => Style::default().fg(sev(c)),
                        None => Style::default().fg(Color::DarkGray),
                    }),
                )),
                Cell::from(Span::styled(
                    match run_attr(r) {
                        Some((_, m, _)) => format!("{m:.0}"),
                        None => "–".to_string(),
                    },
                    fg(match run_attr(r) {
                        Some((_, m, _)) => Style::default().fg(sev(m)),
                        None => Style::default().fg(Color::DarkGray),
                    }),
                )),
                Cell::from(Span::styled(
                    match run_attr(r) {
                        Some((_, _, i)) => format!("{i:.0}"),
                        None => "–".to_string(),
                    },
                    fg(match run_attr(r) {
                        Some((_, _, i)) => Style::default().fg(sev(i)),
                        None => Style::default().fg(Color::DarkGray),
                    }),
                )),
                Cell::from(match r.wait_pct {
                    Some(p) => Span::styled(fmt_pct(p), fg(Style::default().fg(sev(p)))),
                    None => Span::styled(
                        fmt_secs(r.wait_secs),
                        fg(Style::default().fg(Color::DarkGray)),
                    ),
                }),
                Cell::from(Span::raw(fmt_pct(r.cpu_pct))),
                Cell::from(Span::raw(fmt_secs(r.wall))),
                Cell::from(Span::styled(
                    r.users.to_string(),
                    fg(if r.users >= 3 {
                        Style::default().fg(Color::Yellow)
                    } else if r.users > 0 {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }),
                )),
                Cell::from(st),
            ])
            .style(row_style)
        })
        .collect();
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);
    let div_style = Style::default().fg(Color::DarkGray);
    if col_rects.len() >= 7 {
        let d1 = table_area.x.saturating_add(col_rects[0].width);
        let d2 = table_area.x.saturating_add(col_rects[5].right());
        for d in [d1, d2] {
            if d > table_area.x && d < table_area.right() {
                vline(f, d, table_area.y, table_area.height, div_style);
            }
        }
    }
    let drawn = (sorted.len().saturating_sub(app.scroll)).min(visible);
    if let Some((mx, my)) = app.mouse
        && my > table_area.y
        && my < table_area.y + 1 + drawn as u16
        && mx >= table_area.x
        && mx < table_area.x.saturating_add(col_rects[0].width)
        && let Some(r) = sorted.get(app.scroll + (my - table_area.y - 1) as usize)
        && r.params.chars().count() > params_cap
    {
        app.hover = Some((r.params.clone(), mx, my));
    }
    if run_len > visible {
        let mut st = ScrollbarState::new(run_len)
            .position(scrollbar_pos(app.scroll, run_len, visible))
            .viewport_content_length(visible);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            Rect::new(inner.x + inner.width - 1, inner.y + 1, 1, visible as u16),
            &mut st,
        );
    }
}

// User identity colors. Green / yellow / orange / red hues are reserved for
// value-status coloring (good / medium / bad), so users only get cool and
// neutral true-color hues; 12 entries keep collisions rare.
const USER_COLORS: [Color; 12] = [
    Color::Rgb(0, 200, 255),
    Color::Rgb(255, 100, 255),
    Color::Rgb(80, 140, 255),
    Color::Rgb(170, 90, 255),
    Color::Rgb(255, 120, 200),
    Color::Rgb(0, 170, 190),
    Color::Rgb(140, 170, 255),
    Color::Rgb(255, 90, 150),
    Color::Rgb(120, 200, 200),
    Color::Rgb(210, 140, 255),
    Color::Rgb(150, 150, 150),
    Color::Rgb(230, 230, 230),
];

/// Builds a panel title line with a clickable `[overall] [live]` tab; returns
/// the line plus the two tab click areas (in the block's coordinate space).
fn tab_title(
    base: &str,
    x0: u16,
    y: u16,
    live: bool,
    focused: bool,
) -> (Line<'static>, (Rect, Rect)) {
    let base_style = if focused {
        Style::default().fg(Color::Cyan).bold()
    } else {
        Style::default()
    };
    let mut x = x0;
    let mut spans = vec![Span::styled(format!(" {base} "), base_style)];
    x += (base.len() + 2) as u16;
    let mut tabs = Vec::new();
    for (label, active) in [("overall", !live), ("live", live)] {
        let w = (label.len() + 2) as u16;
        tabs.push(Rect::new(x, y, w, 1));
        spans.push(Span::styled(
            format!("[{label}]"),
            if active {
                Style::default().fg(Color::Cyan).bold()
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));
        x += w + 1;
    }
    spans.push(Span::styled(
        " · click to highlight runs ",
        Style::default().fg(Color::DarkGray),
    ));
    (Line::from(spans), (tabs[0], tabs[1]))
}

/// The core count cpu util% / wait% are measured against: the cores our runs
/// actually use when scoping is active (Snapshot.our_cores), the whole
/// machine otherwise (no runs yet, or a loaded snapshot).
fn cpu_cores(s: &Snapshot) -> f64 {
    if s.our_cores > 0 {
        s.our_cores as f64
    } else {
        s.cores as f64
    }
}

fn draw_users_ants(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cols = Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)]).split(area);
    let (utitle, user_tabs) = tab_title("Other users", cols[0].x + 1, cols[0].y, app.live, app.focus == 1);
    let ublock = Block::bordered().title(utitle);
    let uinner = ublock.inner(cols[0]);
    f.render_widget(ublock, cols[0]);
    let (atitle, ants_tabs) = tab_title("Other Processes", cols[1].x + 1, cols[1].y, app.live, app.focus == 2);
    let ablock = Block::bordered().title(atitle);
    let ainner = ablock.inner(cols[1]);
    f.render_widget(ablock, cols[1]);
    app.user_tabs = user_tabs;
    app.ants_tabs = ants_tabs;
    let users_area = Rect::new(uinner.x, uinner.y, uinner.width.saturating_sub(1), uinner.height);
    let ants_area = Rect::new(ainner.x, ainner.y, ainner.width.saturating_sub(1), ainner.height);
    app.users_area = users_area;
    app.ants_area = ants_area;
    let Some(s) = &app.snapshot else {
        return;
    };
    let users = if app.live { &s.live_users } else { &s.users };
    let ants = if app.live { &s.live_ants } else { &s.antagonists };
    let sel_run_users: Option<HashSet<&str>> = match &app.highlight {
        Some(Highlight::Run(o)) => s
            .runs
            .iter()
            .find(|r| r.order == *o)
            .map(|r| r.run_users.iter().map(|u| u.user.as_str()).collect()),
        _ => None,
    };
    let sel_run_ants: Option<Vec<&RunAnt>> = match &app.highlight {
        Some(Highlight::Run(o)) => s
            .runs
            .iter()
            .find(|r| r.order == *o)
            .map(|r| r.ants.iter().collect()),
        _ => None,
    };
    if users.is_empty() {
        let msg = if app.live {
            "no one is actively running right now"
        } else {
            "no impactful other users (cutoff: 1s cpu or 1GiB rss)"
        };
        f.render_widget(Paragraph::new(msg).style(Style::default().fg(Color::DarkGray)), uinner);
    } else {
        let mut sorted: Vec<&UserShare> = users.iter().collect();
        let overlap = user_overlap(s);
        sort_users(&mut sorted, app.users_sort, &overlap);
        let visible = uinner.height.saturating_sub(1).max(1) as usize;
        let total = sorted.len();
        if total > visible {
            app.uscroll = app.uscroll.min(total - visible);
        } else {
            app.uscroll = 0;
        }
        if let Some(sel) = app.sel_user {
            app.sel_user = Some(sel.min(total.saturating_sub(1)));
            let sel = app.sel_user.unwrap();
            if sel < app.uscroll {
                app.uscroll = sel;
            } else if sel >= app.uscroll + visible {
                app.uscroll = sel - visible + 1;
            }
        }
        let denom = if app.live {
            (s.live_dt * cpu_cores(s)).max(1.0)
        } else {
            (s.collecting_secs * cpu_cores(s)).max(1.0)
        };
        let total_cpu: f64 = sorted.iter().map(|u| u.cpu_secs).sum();
        let mut headers: Vec<String> = [
            "#", "user", "util%", "wait%", "runs", "share",
        ]
        .iter()
        .map(|h| h.to_string())
        .collect();
        headers[app.users_sort.0] = format!(
            "{}{}",
            headers[app.users_sort.0],
            if app.users_sort.1 { " ▲" } else { " ▼" }
        );
        let header = Row::new(headers).style(Style::default().fg(Color::Cyan).bold());
        let rows: Vec<Row> = sorted
            .iter()
            .skip(app.uscroll)
            .take(visible)
            .enumerate()
            .map(|(i, u)| {
                let idx = app.uscroll + i;
                let sel = app.sel_user == Some(idx);
                let hl = sel_run_users.is_some();
                let hl_on = sel_run_users
                    .as_ref()
                    .map(|set| set.contains(u.user.as_str()))
                    .unwrap_or(false);
                let fg = |s: Style| {
                    if hl_on {
                        s.fg(Color::White)
                    } else if hl {
                        s.fg(Color::Gray)
                    } else {
                        s
                    }
                };
                let row_style = if hl_on {
                    Style::default().fg(Color::White).bg(HIGHLIGHT_BG)
                } else if hl {
                    Style::default().fg(Color::Gray)
                } else {
                    Style::default()
                };
                let color = user_color(&u.user);
                let share = if total_cpu > 0.0 {
                    u.cpu_secs / total_cpu * 100.0
                } else {
                    0.0
                };
                let wait = wait_ratio_pct(u.wait_secs, u.cpu_secs);
                let runs = if app.live {
                    None
                } else {
                    overlap.get(&u.user).copied()
                };
                let util = u.cpu_secs / denom * 100.0;
                Row::new(vec![
                    Cell::from(Span::styled(
                        if sel { "▶" } else { "-" },
                        if sel {
                            Style::default().fg(Color::Cyan).bold()
                        } else {
                            Style::default()
                        },
                    )),
                    Cell::from(Span::styled(trunc(&u.user, 9), fg(Style::default().fg(color)))),
                    Cell::from(Span::styled(fmt_pct(util), fg(Style::default().fg(sev(util))))),
                    Cell::from(Span::styled(
                        match wait {
                            Some(w) => fmt_pct(w),
                            None => "—".to_string(),
                        },
                        fg(match wait {
                            Some(w) => Style::default().fg(sev(w)),
                            None => Style::default().fg(Color::DarkGray),
                        }),
                    )),
                    Cell::from(Span::styled(
                        match runs {
                            Some(n) => format!("{n}"),
                            None => "–".to_string(),
                        },
                        fg(match runs {
                            Some(n) if n >= 5 => Style::default().fg(Color::Yellow),
                            Some(n) if n > 0 => Style::default().fg(Color::White),
                            _ => Style::default().fg(Color::DarkGray),
                        }),
                    )),
                    Cell::from(Span::styled(
                        fmt_pct(share),
                        fg(Style::default().fg(sev(share))),
                    )),
                ])
                .style(row_style)
            })
            .collect();
        let widths = [
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(8),
        ];
        let table = Table::new(rows, widths).header(header).flex(Flex::Start);
        f.render_widget(table, users_area);
        let drawn = (sorted.len().saturating_sub(app.uscroll)).min(visible);
        if let Some((mx, my)) = app.mouse
            && my > app.users_area.y
            && my < app.users_area.y + 1 + drawn as u16
            && col_at_left(mx, &app.users_area, &USERS_WIDTHS) == Some(1)
            && let Some(u) = sorted.get(app.uscroll + (my - app.users_area.y - 1) as usize)
            && u.user.chars().count() > 9
        {
            app.hover = Some((u.user.clone(), mx, my));
        }
        render_scrollbar(
            f,
            total,
            app.uscroll,
            visible,
            Rect::new(uinner.x + uinner.width - 1, uinner.y + 1, 1, visible as u16),
        );
    }
    if ants.is_empty() {
        let msg = if app.live {
            "no process is actively running right now"
        } else {
            "no impactful processes"
        };
        f.render_widget(Paragraph::new(msg).style(Style::default().fg(Color::DarkGray)), ainner);
        return;
    }
    let mut sorted: Vec<&Antag> = ants.iter().collect();
    let (proc_by_pid, proc_by_comm) = proc_overlap(s);
    sort_ants(&mut sorted, app.ants_sort, &proc_by_pid, &proc_by_comm);
    let visible = ainner.height.saturating_sub(1).max(1) as usize;
    let total = sorted.len();
    if total > visible {
        app.ascroll = app.ascroll.min(total - visible);
    } else {
        app.ascroll = 0;
    }
    if let Some(sel) = app.sel_ant {
        app.sel_ant = Some(sel.min(total.saturating_sub(1)));
        let sel = app.sel_ant.unwrap();
        if sel < app.ascroll {
            app.ascroll = sel;
        } else if sel >= app.ascroll + visible {
            app.ascroll = sel - visible + 1;
        }
    }
    let mut headers: Vec<String> = [
        "#", "user", "comm", "util%", "wait%", "runs", "cmdline",
    ]
    .iter()
    .map(|h| h.to_string())
    .collect();
    headers[app.ants_sort.0] = format!(
        "{}{}",
        headers[app.ants_sort.0],
        if app.ants_sort.1 { " ▲" } else { " ▼" }
    );
    let header = Row::new(headers).style(Style::default().fg(Color::Cyan).bold());
    let cmd_w = (ainner.width as usize).saturating_sub(53);
    let denom = if app.live {
        (s.live_dt * cpu_cores(s)).max(1.0)
    } else {
        (s.collecting_secs * cpu_cores(s)).max(1.0)
    };
    let rows: Vec<Row> = sorted
        .iter()
        .skip(app.ascroll)
        .take(visible)
        .enumerate()
        .map(|(i, a)| {
            let idx = app.ascroll + i;
            let sel = app.sel_ant == Some(idx);
            let hl = sel_run_ants.is_some();
            let hl_on = sel_run_ants
                .as_ref()
                .map(|ants| {
                    ants.iter().any(|ra| {
                        ra.pid == a.pid || (a.pid < 0 && ra.comm == a.comm)
                    })
                })
                .unwrap_or(false);
            let fg = |s: Style| {
                if hl_on {
                    s.fg(Color::White)
                } else if hl {
                    s.fg(Color::Gray)
                } else {
                    s
                }
            };
            let row_style = if hl_on {
                Style::default().fg(Color::White).bg(HIGHLIGHT_BG)
            } else if hl {
                Style::default().fg(Color::Gray)
            } else {
                Style::default()
            };
            let color = user_color(&a.user);
            let wait = wait_ratio_pct(a.wait_secs, a.cpu_secs);
            let runs = if app.live {
                None
            } else if a.pid >= 0 {
                proc_by_pid.get(&a.pid)
            } else {
                proc_by_comm.get(&a.comm)
            }
            .copied();
            let util = a.cpu_secs / denom * 100.0;
            Row::new(vec![
                Cell::from(Span::styled(
                    if sel { "▶" } else { "-" },
                    if sel {
                        Style::default().fg(Color::Cyan).bold()
                    } else {
                        Style::default()
                    },
                )),
                Cell::from(Span::styled(trunc(&a.user, 8), fg(Style::default().fg(color)))),
                Cell::from(Span::styled(trunc(&a.comm, 12), fg(Style::default()))),
                Cell::from(Span::styled(fmt_pct(util), fg(Style::default().fg(sev(util))))),
                Cell::from(Span::styled(
                    match wait {
                        Some(w) => fmt_pct(w),
                        None => "—".to_string(),
                    },
                    fg(match wait {
                        Some(w) => Style::default().fg(sev(w)),
                        None => Style::default().fg(Color::DarkGray),
                    }),
                )),
                Cell::from(Span::styled(
                    match runs {
                        Some(n) => format!("{n}"),
                        None => "–".to_string(),
                    },
                    fg(match runs {
                        Some(n) if n >= 5 => Style::default().fg(Color::Yellow),
                        Some(n) if n > 0 => Style::default().fg(Color::White),
                        _ => Style::default().fg(Color::DarkGray),
                    }),
                )),
                Cell::from(Span::styled(trunc(&a.cmdline, cmd_w), fg(Style::default()))),
            ])
            .style(row_style)
        })
        .collect();
    let widths = [
        Constraint::Length(3),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Min(10),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, ants_area);
    let drawn = (sorted.len().saturating_sub(app.ascroll)).min(visible);
    if let Some((mx, my)) = app.mouse
        && my > app.ants_area.y
        && my < app.ants_area.y + 1 + drawn as u16
        && let Some(a) = sorted.get(app.ascroll + (my - app.ants_area.y - 1) as usize)
    {
        let [_sel, acols_area] =
            Layout::horizontal([Constraint::Length(0), Constraint::Fill(0)])
                .areas(Rect::new(0, 0, ants_area.width, 1));
        let acol_rects =
            Layout::horizontal(widths).flex(Flex::Start).spacing(1).split(acols_area);
        let col = acol_rects.iter().position(|r| {
            mx >= ants_area.x + r.x && mx < ants_area.x + r.x + r.width
        });
        let full = match col {
            Some(1) => Some((a.user.as_str(), 8)),
            Some(2) => Some((a.comm.as_str(), 12)),
            Some(6) => Some((a.cmdline.as_str(), cmd_w)),
            _ => None,
        };
        if let Some((s, cap)) = full
            && s.chars().count() > cap
        {
            app.hover = Some((s.to_string(), mx, my));
        }
    }
    render_scrollbar(
        f,
        total,
        app.ascroll,
        visible,
        Rect::new(ainner.x + ainner.width - 1, ainner.y + 1, 1, visible as u16),
    );
}


/// Background of experiment runs affected by the selected process/user.
/// Text is inverted to black so highlighted rows stay readable.
/// Background of experiment runs affected by the selected process/user (and
/// vice versa). A muted slate blue instead of a bright color, so highlighted
/// rows stand out without being harsh on the eyes; text inverts to white.
const HIGHLIGHT_BG: Color = Color::Rgb(70, 80, 110);

fn sev(pct: f64) -> Color {
    if pct >= 30.0 {
        Color::Red
    } else if pct >= 10.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn trunc(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

const RUNS_FIXED: [u16; 10] = [6, 6, 6, 6, 6, 8, 8, 7, 6, 8];
const USERS_WIDTHS: [u16; 6] = [3, 9, 8, 8, 7, 8];
const ANTS_FIXED: [u16; 6] = [3, 8, 12, 8, 8, 7];

fn cmp_opt_f64(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

fn sort_runs(rows: &mut Vec<&RunRow>, (col, asc): (usize, bool)) {
    rows.sort_by(|a, b| {
        let ord = match col {
            0 => a.params.cmp(&b.params),
            1 => run_ci(a).total_cmp(&run_ci(b)),
            2 => attr_share(a, 0).total_cmp(&attr_share(b, 0)),
            3 => attr_share(a, 1).total_cmp(&attr_share(b, 1)),
            4 => attr_share(a, 2).total_cmp(&attr_share(b, 2)),
            5 => cmp_opt_f64(a.wait_pct, b.wait_pct),
            6 => a.cpu_pct.total_cmp(&b.cpu_pct),
            7 => a.wall.total_cmp(&b.wall),
            8 => a.users.cmp(&b.users),
            _ => a.alive.cmp(&b.alive),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
}

/// The CPU (0), MEM (1) or IO (2) attribution share of a run, or -1 when
/// there is no congestion (so those sort last when descending).
fn attr_share(r: &RunRow, which: usize) -> f64 {
    match run_attr(r) {
        Some((c, m, i)) => match which {
            0 => c,
            1 => m,
            _ => i,
        },
        None => -1.0,
    }
}

fn sort_users(
    rows: &mut Vec<&UserShare>,
    (col, asc): (usize, bool),
    overlap: &UserOverlap,
) {
    rows.sort_by(|a, b| {
        let ord = match col {
            1 => a.user.cmp(&b.user),
            2 => a.cpu_secs.total_cmp(&b.cpu_secs),
            3 => cmp_opt_f64(
                wait_ratio_pct(a.wait_secs, a.cpu_secs),
                wait_ratio_pct(b.wait_secs, b.cpu_secs),
            ),
            4 => overlap
                .get(&a.user)
                .copied()
                .unwrap_or(0)
                .cmp(&overlap.get(&b.user).copied().unwrap_or(0)),
            _ => a.cpu_secs.total_cmp(&b.cpu_secs),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
}

fn sort_ants(
    rows: &mut Vec<&Antag>,
    (col, asc): (usize, bool),
    by_pid: &HashMap<i32, usize>,
    by_comm: &HashMap<String, usize>,
) {
    let ov = |a: &&Antag| {
        if a.pid >= 0 {
            by_pid.get(&a.pid)
        } else {
            by_comm.get(&a.comm)
        }
        .copied()
        .unwrap_or(0)
    };
    rows.sort_by(|a, b| {
        let ord = match col {
            1 => a.user.cmp(&b.user),
            2 => a.comm.cmp(&b.comm),
            3 => a.cpu_secs.total_cmp(&b.cpu_secs),
            4 => cmp_opt_f64(
                wait_ratio_pct(a.wait_secs, a.cpu_secs),
                wait_ratio_pct(b.wait_secs, b.cpu_secs),
            ),
            5 => ov(a).cmp(&ov(b)),
            _ => a.cmdline.cmp(&b.cmdline),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
}

fn user_color(user: &str) -> Color {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in user.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    USER_COLORS[(h % USER_COLORS.len() as u64) as usize]
}

/// Draws a vertical divider line down a column of the buffer without
/// consuming any layout space (overlays the existing cell spacing). Uses the
/// thin left-block glyph so it reads as a hairline, not a solid bar.
fn vline(f: &mut Frame, x: u16, y: u16, h: u16, style: Style) {
    if h == 0 {
        return;
    }
    let text = Text::from(vec![Line::from(Span::styled("▏", style)); h as usize]);
    f.render_widget(Paragraph::new(text), Rect::new(x, y, 1, h));
}

fn render_scrollbar(
    f: &mut Frame,
    total: usize,
    pos: usize,
    viewport: usize,
    bar: Rect,
) {
    if total > viewport {
        let mut st = ScrollbarState::new(total)
            .position(scrollbar_pos(pos, total, viewport))
            .viewport_content_length(viewport);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            bar,
            &mut st,
        );
    }
}

/// Converts a scroll offset (range 0..total-viewport) into ratatui's
/// scrollbar position scale (0..total-1). Without this the thumb would stop
/// ~3/4 down the track instead of reaching the bottom at max scroll, because
/// ratatui maps `position` over `content_length - 1`, not the scroll range.
fn scrollbar_pos(scroll: usize, total: usize, viewport: usize) -> usize {
    let range = total.saturating_sub(viewport);
    if range == 0 {
        return 0;
    }
    (scroll.saturating_mul(total.saturating_sub(1)) + range / 2) / range
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::{PsiPct, RunUser, TargetStatus};
    use ratatui::backend::TestBackend;

    fn run(wait: Option<f64>, psi: f64) -> RunRow {
        RunRow {
            params: "p".into(),
            roots: vec![],
            wall: 1.0,
            cpu_secs: 1.0,
            wait_secs: wait.unwrap_or(0.0),
            wait_pct: wait,
            cpu_pct: 1.0,
            rss: 100,
            psi: [psi, 0.0, 0.0],
            alive: false,
            order: 0,
            users: 0,
            cf: None,
            cl: None,
            ants: vec![],
            run_users: vec![],
        }
    }

    #[test]
    fn runs_sort_most_affected_first() {
        let a = run(Some(5.0), 1.0);
        let b = run(Some(50.0), 2.0);
        let c = run(None, 3.0);
        let mut rows = vec![&a, &c, &b];
        sort_runs(&mut rows, (5, false));
        assert_eq!(rows[0].wait_pct, Some(50.0));
        assert_eq!(rows[1].wait_pct, Some(5.0));
        assert_eq!(rows[2].wait_pct, None);
    }

    #[test]
    fn runs_sort_ascending() {
        let a = run(Some(5.0), 1.0);
        let b = run(Some(50.0), 2.0);
        let mut rows = vec![&b, &a];
        sort_runs(&mut rows, (5, true));
        assert_eq!(rows[0].wait_pct, Some(5.0));
    }

    #[test]
    fn runs_sort_by_wait_puts_none_last_ascending() {
        let mut a = run(None, 0.0);
        a.wait_pct = Some(2.5);
        let mut b = run(None, 0.0);
        b.wait_pct = None;
        let mut rows = vec![&b, &a];
        sort_runs(&mut rows, (5, true));
        assert_eq!(rows[0].wait_pct, None);
        assert_eq!(rows[1].wait_pct, Some(2.5));
    }

    #[test]
    fn runs_sort_by_ci() {
        let a = run(Some(1.0), 1.0);
        let b = run(Some(1.0), 9.0);
        let mut rows = vec![&a, &b];
        sort_runs(&mut rows, (1, false));
        assert_eq!(rows[0].psi[0], 9.0);
    }

    #[test]
    fn runs_sort_by_c_share() {
        let mut a = run(Some(2.0), 0.0);
        a.params = "a".into();
        a.wall = 10.0;
        a.psi = [0.0, 50.0, 0.0];
        let mut b = run(Some(2.0), 0.0);
        b.params = "b".into();
        b.wall = 10.0;
        let mut rows = vec![&a, &b];
        sort_runs(&mut rows, (2, false));
        assert_eq!(rows[0].params, "b");
    }

    #[test]
    fn overlap_aggregates_across_runs() {
        let mut s = crate::collector::Snapshot {
            seq: 1,
            history: Vec::new(),
            target: "t".into(),
            rules: Vec::new(),
            status: TargetStatus::Active(1),
            psi: crate::procfs::PsiSet::default(),
            psi_pct: PsiPct::default(),
            sys_wait: None,
            rss_total: 0,
            mem_total: 0,
            mem_avail: 0,
            runs: Vec::new(),
            share_cpu: [0.0; 3],
            share_mem: [0.0; 3],
            antagonists: Vec::new(),
            users: Vec::new(),
            live_ants: Vec::new(),
            live_users: Vec::new(),
            live_dt: 1.0,
            conditions: crate::conditions::CondSummary::default(),
            collecting: false,
            cores: 1,
            our_cores: 0,
            collecting_secs: 0.0,
            rec_secs: 0.0,
            scanned: 0,
        };
        let mut r1 = run(None, 0.0);
        r1.run_users.push(RunUser {
            user: "alice".into(),
            cpu_secs: 5.0,
            rss: 0,
            procs: 1,
        });
        r1.ants.push(RunAnt {
            pid: 42,
            comm: "make".into(),
            cpu_secs: 3.0,
            rss: 0,
        });
        let mut r2 = run(None, 0.0);
        r2.run_users.push(RunUser {
            user: "alice".into(),
            cpu_secs: 2.0,
            rss: 0,
            procs: 1,
        });
        r2.run_users.push(RunUser {
            user: "bob".into(),
            cpu_secs: 1.0,
            rss: 0,
            procs: 1,
        });
        r2.ants.push(RunAnt {
            pid: 43,
            comm: "cc1".into(),
            cpu_secs: 4.0,
            rss: 0,
        });
        s.runs = vec![r1, r2];
        let uo = user_overlap(&s);
        assert_eq!(uo["alice"], 2);
        assert_eq!(uo["bob"], 1);
        let (by_pid, by_comm) = proc_overlap(&s);
        assert_eq!(by_pid[&42], 1);
        assert_eq!(by_comm["make"], 1);
        assert_eq!(by_comm["cc1"], 1);
    }

    #[test]
    fn help_overlay_shows_annotations() {
        let mut app = app_with_run("x");
        app.help = true;
        let backend = TestBackend::new(120, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let all = buffer_text(&mut terminal);
        for token in [
            "Worker Filter",
            "Live congestion",
            "congestion",
            "Experiment Runs",
            "params",
            "state",
            "click to associate a run",
            "click to associate a user",
            "click to associate a process",
            "hide help",
        ] {
            assert!(all.contains(token), "help missing {token}");
        }
        assert!(!all.contains("design"), "design explanations removed");
        assert!(!all.contains("1.50"), "no real data rows in help mode");
    }

    #[test]
    fn help_key_toggles_mode() {
        let mut app = app_with_run("x");
        assert!(!app.help);
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert!(app.help, "h enters help");
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(app.help, "other keys ignored in help mode");
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert!(!app.help, "h again leaves help");
    }

    #[test]
    fn scrollbar_pos_maps_scroll_range_to_full_track() {
        assert_eq!(scrollbar_pos(0, 10, 4), 0);
        assert_eq!(scrollbar_pos(6, 10, 4), 9, "max scroll maps to content-1");
        assert_eq!(scrollbar_pos(3, 10, 4), 5);
        assert_eq!(scrollbar_pos(0, 5, 5), 0);
    }

    #[test]
    fn scrollbar_thumb_touches_bottom_at_max_scroll() {
        let backend = TestBackend::new(3, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let mut top = ScrollbarState::new(10)
                    .position(scrollbar_pos(0, 10, 4))
                    .viewport_content_length(4);
                let mut bottom = ScrollbarState::new(10)
                    .position(scrollbar_pos(6, 10, 4))
                    .viewport_content_length(4);
                f.render_stateful_widget(
                    Scrollbar::default()
                        .orientation(ScrollbarOrientation::VerticalRight)
                        .begin_symbol(None)
                        .end_symbol(None),
                    Rect::new(0, 0, 1, 6),
                    &mut top,
                );
                f.render_stateful_widget(
                    Scrollbar::default()
                        .orientation(ScrollbarOrientation::VerticalRight)
                        .begin_symbol(None)
                        .end_symbol(None),
                    Rect::new(2, 0, 1, 6),
                    &mut bottom,
                );
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(0, 0)].symbol(), "█", "thumb at top when scrolled to top");
        assert_eq!(buf[(2, 5)].symbol(), "█", "thumb at bottom when scrolled to bottom");
    }

    #[test]
    fn first_run_popup_renders_and_h_opens_help() {
        let mut app = app_with_run("x");
        app.first_run = true;
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let all = buffer_text(&mut terminal);
        assert!(
            all.contains("Get familiar with the scores and numbers"),
            "first-run popup missing"
        );
        assert!(all.contains("[ got it ]"), "dismiss button missing");
        // 'h' from the first-run popup dismisses it and opens help
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert!(!app.first_run, "popup dismissed");
        assert!(app.help, "h opens the help overlay");
        // any other key just dismisses
        let mut app = app_with_run("x");
        app.first_run = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert!(!app.first_run && !app.help, "other keys only dismiss");
    }

    #[test]
    fn first_run_popup_only_got_it_click_dismisses() {
        let mut app = app_with_run("x");
        app.first_run = true;
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let pop = app.first_run_rect.expect("popup rect recorded");
        // 120x40 -> pop centered at (33, 15) 54x9, button row inner.y + 6
        // a mouse move (even over the popup) must not dismiss it
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: pop.x + 2,
                row: pop.y + 6,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(app.first_run, "mouse move must not dismiss the notice");
        // clicks elsewhere neither
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 5,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(app.first_run, "click outside must not dismiss the notice");
        // clicking [ got it ] dismisses
        let btn = first_run_got_it_rect(pop);
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: btn.x + 4,
                row: btn.y,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(!app.first_run, "clicking [ got it ] dismisses the notice");
    }

    #[test]
    fn demo_env_disables_first_run_notice() {
        // SAFETY: single-threaded ui tests never read this var elsewhere
        unsafe { std::env::set_var("SERVER_SPY_DEMO", "1") };
        let suppressed = !first_run_notice_wanted();
        unsafe { std::env::remove_var("SERVER_SPY_DEMO") };
        assert!(suppressed, "SERVER_SPY_DEMO must suppress the notice");
    }

    #[test]
    fn left_panes_share_excess_height_fairly() {
        // 37 = sum of minimums: no excess
        assert_eq!(left_col_heights(37), [17, 12, 8]);
        // 40: excess 3, split equally
        assert_eq!(left_col_heights(40), [18, 13, 9]);
        // 50: excess 13 -> each +4, remainder 1 to congestion
        assert_eq!(left_col_heights(50), [22, 16, 12]);
        assert_eq!(left_col_heights(30), [17, 12, 8], "never below minimums");
        let rects = Layout::vertical([
            Constraint::Length(19),
            Constraint::Length(13),
            Constraint::Length(8),
        ])
        .split(Rect::new(0, 0, 1, 40));
        assert_eq!(rects[0].height + rects[1].height + rects[2].height, 40);
    }

    #[test]
    fn stats_pane_renders_lines_and_copy_buttons() {
        let mut app = app_with_run("x");
        let mut r1 = run(Some(5.0), 1.0);
        r1.wall = 100.0;
        r1.cl = Some(2.0);
        let mut r2 = run(Some(40.0), 9.0);
        r2.wall = 200.0;
        r2.cl = Some(20.0);
        let c = crate::conditions::build_conditions(&[r1, r2], 16);
        if let Some(s) = &mut app.snapshot {
            s.conditions = c;
        }
        let backend = TestBackend::new(42, 9);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_stats(f, &mut app, Rect::new(0, 0, 42, 9)))
            .unwrap();
        let all = buffer_text(&mut terminal);
        for token in ["med", "MAD", "[sentence]", "[table]"] {
            assert!(all.contains(token), "stats pane missing {token}");
        }
        assert!(
            !app.stat_copy_area.is_empty() && !app.stat_table_area.is_empty(),
            "copy buttons have click areas"
        );
        // empty conditions produce a flash without touching the clipboard
        let mut app = app_with_run("x");
        copy_conditions(&mut app, false);
        assert!(app.flash.is_some());
    }

    #[test]
    fn stats_pane_empty_state() {
        let mut app = app_with_run("x");
        let backend = TestBackend::new(42, 9);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_stats(f, &mut app, Rect::new(0, 0, 42, 9)))
            .unwrap();
        let all = buffer_text(&mut terminal);
        assert!(all.contains("no completed runs yet"), "{all}");
    }

    #[test]
    fn popup_renders_two_fields_and_buttons() {
        let mut app = app_with_run("x");
        open_popup(&mut app);
        if let Some(p) = &mut app.popup {
            p.input = "runner\\.py.*".into();
            p.exc_input = "vim".into();
        }
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw_popup(f, &mut app)).unwrap();
        let all = buffer_text(&mut terminal);
        for token in [
            "filter:",
            "runner\\.py.*",
            "exclude:",
            "vim",
            "matching now",
            "[ confirm ]",
            "[ cancel ]",
        ] {
            assert!(all.contains(token), "popup missing {token}");
        }
    }

    #[test]
    fn popup_typing_builds_filter_and_exclude_rules() {
        let mut app = app_with_run("x");
        open_popup(&mut app);
        for c in ['r', 'u', 'n'] {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        for c in ['v', 'i'] {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        if let Some(p) = &app.popup {
            assert!(p.exclude_focused);
            assert_eq!(p.input, "run");
            assert_eq!(p.exc_input, "vi");
        }
        let rules = rules_from_popup(app.popup.as_ref().unwrap());
        assert_eq!(rules.len(), 2);
        assert!(rules[0].regex && !rules[0].exclude && rules[0].pattern == "run");
        assert!(rules[1].regex && rules[1].exclude && rules[1].pattern == "vi");
        // empty filter must not produce rules
        let mut app = app_with_run("x");
        open_popup(&mut app);
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        );
        let rules = rules_from_popup(app.popup.as_ref().unwrap());
        assert_eq!(rules.len(), 1);
        assert!(rules[0].exclude);
    }

    #[test]
    fn col_at_maps_clicks() {
        let area = Rect::new(10, 0, 100, 5);
        assert_eq!(col_at(11, &area, &RUNS_FIXED, 0), Some(0));
        let min_w = 100 - RUNS_FIXED.iter().sum::<u16>() - (RUNS_FIXED.len() + 1) as u16;
        let wall_start = 10 + min_w;
        assert_eq!(col_at(wall_start + 2, &area, &RUNS_FIXED, 0), Some(1));
        assert_eq!(col_at(10 + 99, &area, &RUNS_FIXED, 0), Some(10));
        assert_eq!(col_at(5, &area, &RUNS_FIXED, 0), None);
    }

    #[test]
    fn col_at_users_mapping() {
        let area = Rect::new(0, 0, 70, 5);
        let mut cur = 0u16;
        for (i, w) in USERS_WIDTHS.iter().enumerate() {
            assert_eq!(col_at_left(cur + w - 1, &area, &USERS_WIDTHS), Some(i));
            cur += w + 1;
        }
        assert_eq!(col_at_left(69, &area, &USERS_WIDTHS), None);
        assert_eq!(col_at_left(71, &area, &USERS_WIDTHS), None);
    }

    #[test]
    fn col_at_ants_mapping() {
        let area = Rect::new(0, 0, 70, 5);
        let mut cur = 0u16;
        for (i, w) in ANTS_FIXED.iter().enumerate() {
            assert_eq!(col_at(cur + w - 1, &area, &ANTS_FIXED, 6), Some(i));
            cur += w + 1;
        }
        assert_eq!(col_at(69, &area, &ANTS_FIXED, 6), Some(6));
    }

    #[test]
    fn toggle_first_click_descending() {
        let mut st = (0usize, true);
        toggle_sort(&mut st, 3);
        assert_eq!(st, (3, false));
        toggle_sort(&mut st, 3);
        assert_eq!(st, (3, true));
        toggle_sort(&mut st, 1);
        assert_eq!(st, (1, false));
    }

    fn app_with_run(params: &str) -> App {
        let mut app = App::new("t".into(), 100, Duration::from_secs(1));
        app.first_run = false;
        let mut s = crate::collector::Snapshot {
            seq: 1,
            history: Vec::new(),
            target: "t".into(),
            rules: Vec::new(),
            status: TargetStatus::Active(1),
            psi: crate::procfs::PsiSet::default(),
            psi_pct: PsiPct::default(),
            sys_wait: None,
            rss_total: 0,
            mem_total: 0,
            mem_avail: 0,
            runs: Vec::new(),
            share_cpu: [0.0; 3],
            share_mem: [0.0; 3],
            antagonists: Vec::new(),
            users: Vec::new(),
            live_ants: Vec::new(),
            live_users: Vec::new(),
            live_dt: 1.0,
            conditions: crate::conditions::CondSummary::default(),
            collecting: false,
            cores: 1,
            our_cores: 0,
            collecting_secs: 0.0,
            rec_secs: 0.0,
            scanned: 0,
        };
        let mut r = run(None, 0.0);
        r.params = params.into();
        r.wall = 10.0;
        r.wait_secs = 4.0;
        r.cf = Some(1.5);
        s.runs.push(r);
        app.snapshot = Some(s);
        app
    }

    fn buffer_text(terminal: &mut Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let (w, h) = buf.area.as_size().into();
        (0..h)
            .flat_map(|y| (0..w).map(move |x| buf[(x, y)].symbol().to_string()))
            .collect()
    }

    #[test]
    fn runs_table_renders_all_headers_and_attribution() {
        let mut app = app_with_run("worker.py --algo=hnsw");
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_runs(f, &mut app, Rect::new(0, 0, 120, 20)))
            .unwrap();
        let all = buffer_text(&mut terminal);
        for title in [
            "params", "congestion", "cpu%", "mem%", "io%", "wait%", "util%", "wall", "usr",
            "state",
        ] {
            assert!(all.contains(title), "header {title} missing");
        }
        assert!(all.contains("worker.py"), "run row missing");
        assert!(all.contains("✓"), "done symbol missing");
        assert!(all.contains("▏"), "dividers missing");
    }

    #[test]
    fn hover_over_clipped_params_shows_tooltip() {
        let long = "python3 /exp/worker.py --algo=hnsw --M=16 --ef=64 --dataset=glove-100 --batch=1000 --mode=benchmark".to_string();
        let mut app = app_with_run(&long);
        app.mouse = Some((5, 2));
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw_runs(f, &mut app, Rect::new(0, 0, 120, 20));
                if let Some((text, x, y)) = &app.hover {
                    draw_tooltip(f, text, *x, *y);
                }
            })
            .unwrap();
        let all = buffer_text(&mut terminal);
        for w in long.split_whitespace() {
            assert!(all.contains(w), "tooltip missing {w}");
        }
    }

    #[test]
    fn clipped_params_only_tooltips_when_truncated() {
        let mut app = app_with_run("short");
        app.mouse = Some((5, 2));
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_runs(f, &mut app, Rect::new(0, 0, 120, 20)))
            .unwrap();
        assert!(app.hover.is_none(), "short params must not show a tooltip");
    }

    #[test]
    fn run_affected_matches_user_and_pid() {
        let mut r = run(None, 0.0);
        r.ants.push(RunAnt {
            pid: 42,
            comm: "make".into(),
            cpu_secs: 3.0,
            rss: 0,
        });
        r.run_users.push(RunUser {
            user: "alice".into(),
            cpu_secs: 5.0,
            rss: 0,
            procs: 1,
        });
        assert!(run_affected(&r, &Highlight::User("alice".into())));
        assert!(!run_affected(&r, &Highlight::User("bob".into())));
        assert!(run_affected(
            &r,
            &Highlight::Ant { pid: 42, comm: "make".into() }
        ));
        assert!(!run_affected(
            &r,
            &Highlight::Ant { pid: 43, comm: "make".into() }
        ));
    }

    #[test]
    fn run_affected_falls_back_to_comm_for_loaded_snapshots() {
        let mut r = run(None, 0.0);
        r.ants.push(RunAnt {
            pid: 42,
            comm: "make".into(),
            cpu_secs: 3.0,
            rss: 0,
        });
        assert!(run_affected(
            &r,
            &Highlight::Ant { pid: -1, comm: "make".into() }
        ));
        assert!(!run_affected(
            &r,
            &Highlight::Ant { pid: -1, comm: "cc1".into() }
        ));
    }

    #[test]
    fn run_affected_matches_run_by_order() {
        let mut a = run(None, 0.0);
        a.order = 7;
        let mut b = run(None, 0.0);
        b.order = 8;
        assert!(run_affected(&a, &Highlight::Run(7)));
        assert!(!run_affected(&a, &Highlight::Run(8)));
        assert!(run_affected(&b, &Highlight::Run(8)));
    }
}

