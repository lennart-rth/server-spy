use std::cmp::Ordering;
use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::collector::{Antag, MatchInfo, RunRow, Snapshot, UserShare};
use crate::daemon;
use crate::metrics::{fmt_bytes, fmt_clock, fmt_pct, fmt_secs};
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
    regex: bool,
    matches: Vec<MatchInfo>,
    error: Option<String>,
    last_preview: String,
    last_regex: bool,
    confirm_area: Rect,
    cancel_area: Rect,
    simple_area: Rect,
    regex_area: Rect,
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
    hist_cpu: VecDeque<u64>,
    hist_mem: VecDeque<u64>,
    hist_io: VecDeque<u64>,
    hist_wait: VecDeque<u64>,
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
            hist_cpu: VecDeque::new(),
            hist_mem: VecDeque::new(),
            hist_io: VecDeque::new(),
            hist_wait: VecDeque::new(),
            history_len,
            scroll: 0,
            uscroll: 0,
            ascroll: 0,
            runs_sort: (3, false),
            users_sort: (2, false),
            ants_sort: (3, false),
            runs_area: Rect::default(),
            users_area: Rect::default(),
            ants_area: Rect::default(),
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
                self.hist_cpu.clear();
                self.hist_mem.clear();
                self.hist_io.clear();
                self.hist_wait.clear();
                for [c, m, i, w] in &s.history {
                    push(&mut self.hist_cpu, (*c * 1000.0) as u64, self.history_len);
                    push(&mut self.hist_mem, (*m * 1000.0) as u64, self.history_len);
                    push(&mut self.hist_io, (*i * 1000.0) as u64, self.history_len);
                    push(&mut self.hist_wait, (*w * 1000.0) as u64, self.history_len);
                }
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

fn push(q: &mut VecDeque<u64>, v: u64, cap: usize) {
    q.push_back(v);
    while q.len() > cap {
        q.pop_front();
    }
}

fn open_popup(app: &mut App, prefill: String) {
    app.popup = Some(Popup {
        input: prefill,
        cursor: 0,
        regex: false,
        matches: Vec::new(),
        error: None,
        last_preview: String::new(),
        last_regex: false,
        confirm_area: Rect::default(),
        cancel_area: Rect::default(),
        simple_area: Rect::default(),
        regex_area: Rect::default(),
    });
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

fn confirm_filter(app: &mut App) {
    if let Some(p) = app.popup.take() {
        let filter = p.input.trim().to_string();
        if filter.is_empty() {
            app.flash = Some(("filter is empty".into(), Instant::now()));
        } else {
            app.paused = false;
            app.name_input = filter.clone();
            if daemon::set_target_mode(&filter, p.regex).is_err() {
                let _ = daemon::stop();
                let _ = daemon::start(&filter, app.interval, app.history_len);
            }
            app.refresh();
        }
    }
}

fn update_preview(app: &mut App) {
    let Some(p) = &mut app.popup else {
        return;
    };
    if p.input == p.last_preview && p.regex == p.last_regex {
        return;
    }
    if p.input.trim().is_empty() {
        p.matches.clear();
        p.error = None;
        p.last_preview = p.input.clone();
        p.last_regex = p.regex;
        app.dirty = true;
        return;
    }
    match daemon::preview_filter(&p.input, p.regex) {
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
    p.last_preview = p.input.clone();
    p.last_regex = p.regex;
    app.dirty = true;
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
        match k.code {
            KeyCode::Char(c) if !ctrl => {
                if let Some(p) = &mut app.popup {
                    input_char(&mut p.input, &mut p.cursor, c);
                }
            }
            KeyCode::Backspace => {
                if let Some(p) = &mut app.popup {
                    input_backspace(&mut p.input, &mut p.cursor);
                }
            }
            KeyCode::Delete => {
                if let Some(p) = &mut app.popup {
                    input_delete(&mut p.input, &mut p.cursor);
                }
            }
            KeyCode::Left => {
                if let Some(p) = &mut app.popup {
                    p.cursor = cur_left(&p.input, p.cursor);
                }
            }
            KeyCode::Right => {
                if let Some(p) = &mut app.popup {
                    p.cursor = cur_right(&p.input, p.cursor);
                }
            }
            KeyCode::Home => {
                if let Some(p) = &mut app.popup {
                    p.cursor = 0;
                }
            }
            KeyCode::End => {
                if let Some(p) = &mut app.popup {
                    p.cursor = p.input.len();
                }
            }
            KeyCode::Tab => {
                if let Some(p) = &mut app.popup {
                    p.regex = !p.regex;
                }
            }
            KeyCode::Enter => confirm_filter(app),
            KeyCode::Esc => {
                app.popup = None;
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
        KeyCode::Char('f') => {
            let prefill = app
                .snapshot
                .as_ref()
                .map(|s| s.target.clone())
                .unwrap_or_else(|| app.name_input.clone());
            open_popup(app, prefill);
        }
        KeyCode::Char('t') => open_stealth(app),
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
        KeyCode::Down | KeyCode::Char('j') => app.scroll = app.scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => app.scroll = app.scroll.saturating_sub(1),
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

fn toggle_sort(state: &mut (usize, bool), col: usize) {
    if state.0 == col {
        state.1 = !state.1;
    } else {
        state.0 = col;
        state.1 = false;
    }
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    match m.kind {
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
                let simple = in_rect(m.row, m.column, p.simple_area);
                let regex = in_rect(m.row, m.column, p.regex_area);
                if confirm {
                    confirm_filter(app);
                } else if cancel {
                    app.popup = None;
                } else if simple
                    && let Some(p) = &mut app.popup
                {
                    p.regex = false;
                } else if regex
                    && let Some(p) = &mut app.popup
                {
                    p.regex = true;
                }
                return;
            }
            if in_rect(m.row, m.column, app.filter_btn_area) {
                open_popup(app, String::new());
                return;
            }
            if m.row == app.runs_area.y
                && let Some(col) = col_at(m.column, &app.runs_area, &RUNS_FIXED, 0)
            {
                toggle_sort(&mut app.runs_sort, col);
            } else if m.row == app.users_area.y
                && let Some(col) = col_at(m.column, &app.users_area, &USERS_FIXED, 0)
                && col > 0
            {
                toggle_sort(&mut app.users_sort, col);
            } else if m.row == app.ants_area.y
                && let Some(col) = col_at(m.column, &app.ants_area, &ANTS_FIXED, 6)
                && col > 0
            {
                toggle_sort(&mut app.ants_sort, col);
            }
        }
        _ => {}
    }
}

fn draw(f: &mut Frame, app: &mut App) {
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
        ("q", "terminate"),
        ("d", "detach"),
        ("s", "save"),
        ("l", "load"),
        ("f", "update exp. filter"),
        ("t", "stealth"),
        ("r", if app.paused { "live" } else { "restart recording" }),
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
    if w < 24 || h < 9 {
        return;
    }
    let pop = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, pop);
    let block = Block::bordered().title(" define worker filter ");
    let inner = block.inner(pop);
    f.render_widget(block, pop);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let input_line = Line::from(vec![
        Span::styled("filter: ", Style::default().fg(Color::Cyan)),
        Span::styled(p.input.clone(), Style::default().fg(Color::White)),
    ]);
    f.render_widget(Paragraph::new(input_line), rows[0]);
    f.set_cursor_position(Position::new(inner.x + 8 + p.cursor as u16, rows[0].y));

    let simple = if p.regex {
        Span::styled(" simple ", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(" simple ", Style::default().fg(Color::Cyan).bold())
    };
    let regex = if p.regex {
        Span::styled(" regex ", Style::default().fg(Color::Cyan).bold())
    } else {
        Span::styled(" regex ", Style::default().fg(Color::DarkGray))
    };
    let mode_line = Line::from(vec![
        Span::styled("mode: ", Style::default().fg(Color::Cyan)),
        simple,
        Span::raw(" | "),
        regex,
    ]);
    f.render_widget(Paragraph::new(mode_line), rows[1]);
    let s_x = inner.x + 6;
    p.simple_area = Rect::new(s_x, rows[1].y, 8, 1);
    p.regex_area = Rect::new(s_x + 8 + 3, rows[1].y, 8, 1);

    let info = match &p.error {
        Some(e) => Span::styled(e.clone(), Style::default().fg(Color::Red)),
        None => Span::styled(
            format!("matching now: {}", p.matches.len()),
            Style::default().fg(Color::DarkGray),
        ),
    };
    f.render_widget(Paragraph::new(Line::from(vec![info])), rows[2]);

    let marea = rows[3];
    let n = marea.height.saturating_sub(1) as usize;
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
    p.confirm_area = Rect::new(inner.x + 1, rows[4].y, 12, 1);
    p.cancel_area = Rect::new(inner.x + 14, rows[4].y, 11, 1);
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

fn draw_middle(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cols =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);
    let left = Layout::vertical([Constraint::Min(0), Constraint::Length(12)]).split(cols[0]);
    draw_psi(f, app, left[0]);
    draw_util(f, app, left[1]);
    draw_runs(f, app, cols[1]);
}

fn draw_psi(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::bordered().title(" Live congestion ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 5 {
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .split(inner);
    let g = rows[0];
    if let Some(s) = &app.snapshot {
        let items = [
            ("cpu pressure", s.psi_pct.cpu_some, Color::Cyan),
            ("mem pressure", s.psi_pct.mem_some, Color::Magenta),
            ("io pressure", s.psi_pct.io_some, Color::Blue),
            ("sched wait", s.sys_wait.unwrap_or(0.0), Color::Green),
        ];
        let label_w = items.iter().map(|(l, _, _)| l.len()).max().unwrap_or(8);
        for (i, (label, cur, color)) in items.iter().enumerate() {
            let line = gauge_line(label, *cur, *color, g.width, label_w);
            f.render_widget(Paragraph::new(line), Rect::new(g.x, g.y + i as u16, g.width, 1));
        }
    }
    let div_label = " last ~30 min ";
    let total = inner.width as usize;
    let dashes = total.saturating_sub(div_label.len());
    let div = format!("{}{}{}", "─".repeat(dashes / 2), div_label, "─".repeat(dashes - dashes / 2));
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            div,
            Style::default().fg(Color::DarkGray),
        )])),
        rows[1],
    );
    let colors = [Color::Cyan, Color::Magenta, Color::Blue, Color::Green];
    let data = [&app.hist_cpu, &app.hist_mem, &app.hist_io, &app.hist_wait];
    let titles = ["cpu pressure", "mem pressure", "io pressure", "sched wait"];
    for i in 0..4 {
        let color = colors[i];
        let d: Vec<u64> = data[i].iter().copied().collect();
        let peak = d.iter().copied().max().unwrap_or(0);
        let scale = (peak as f64 * 1.25).clamp(50.0, 100_000.0) as u64;
        let spk = Sparkline::default()
            .block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(Span::styled(format!(" {} ", titles[i]), Style::default().fg(color))),
            )
            .data(&d)
            .style(Style::default().fg(color))
            .max(scale);
        f.render_widget(spk, rows[i + 2]);
    }
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

fn gauge_line(label: &str, cur: f64, color: Color, width: u16, label_w: usize) -> Line<'static> {
    let bar_w = (width as i64 - label_w as i64 - 11).max(4) as f64;
    let mut fill = (bar_w * (cur / 100.0).clamp(0.0, 1.0).sqrt()).round() as usize;
    if cur > 0.0 && fill == 0 {
        fill = 1;
    }
    let empty = bar_w as usize - fill;
    let pct = fmt_pct(cur);
    vec![
        Span::styled("█".repeat(fill), Style::default().fg(color)),
        Span::styled("█".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(" {label:<label_w$} "),
            Style::default().fg(color).bold(),
        ),
        Span::styled(format!("{pct:>8}"), Style::default().fg(color).bold()),
    ]
    .into()
}

fn draw_runs(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::bordered().title(" Experiment Runs ");
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
        "params", "wall", "cpu", "wait%", "cpu%", "rss", "max. user", "psi-c", "psi-m",
        "psi-i", "state",
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
    let rows: Vec<Row> = sorted
        .iter()
        .skip(app.scroll)
        .take(visible)
        .map(|r| {
            let st = if r.alive {
                Span::styled("● alive", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ done ", Style::default().fg(Color::DarkGray))
            };
            Row::new(vec![
                Cell::from(Span::raw(r.params.clone())),
                Cell::from(Span::raw(fmt_secs(r.wall))),
                Cell::from(Span::raw(fmt_secs(r.cpu_secs))),
                Cell::from(match r.wait_pct {
                    Some(p) => Span::styled(fmt_pct(p), Style::default().fg(sev(p))),
                    None => Span::styled(fmt_secs(r.wait_secs), Style::default().fg(Color::DarkGray)),
                }),
                Cell::from(Span::raw(fmt_pct(r.cpu_pct))),
                Cell::from(Span::raw(fmt_bytes(r.rss))),
                Cell::from(Span::styled(
                    r.users.to_string(),
                    if r.users >= 3 {
                        Style::default().fg(Color::Yellow)
                    } else if r.users > 0 {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                )),
                Cell::from(Span::styled(fmt_pct(r.psi[0]), Style::default().fg(sev(r.psi[0])))),
                Cell::from(Span::styled(fmt_pct(r.psi[1]), Style::default().fg(sev(r.psi[1])))),
                Cell::from(Span::styled(fmt_pct(r.psi[2]), Style::default().fg(sev(r.psi[2])))),
                Cell::from(st),
            ])
        })
        .collect();
    let widths = [
        Constraint::Min(26),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(11),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(7),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);
    if run_len > visible {
        let mut st = ScrollbarState::new(run_len)
            .position(app.scroll)
            .viewport_content_length(visible);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            Rect::new(inner.x + inner.width - 1, inner.y, 1, inner.height),
            &mut st,
        );
    }
}

const USER_COLORS: [Color; 10] = [
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::Red,
    Color::Blue,
    Color::Green,
    Color::LightCyan,
    Color::LightMagenta,
    Color::LightYellow,
    Color::LightRed,
];

fn draw_users_ants(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cols = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);
    let ublock = Block::bordered().title(" Other users ");
    let uinner = ublock.inner(cols[0]);
    f.render_widget(ublock, cols[0]);
    let ablock = Block::bordered().title(" Other Processes ");
    let ainner = ablock.inner(cols[1]);
    f.render_widget(ablock, cols[1]);
    let users_area = Rect::new(uinner.x, uinner.y, uinner.width.saturating_sub(1), uinner.height);
    let ants_area = Rect::new(ainner.x, ainner.y, ainner.width.saturating_sub(1), ainner.height);
    app.users_area = users_area;
    app.ants_area = ants_area;
    let Some(s) = &app.snapshot else {
        return;
    };
    if s.users.is_empty() {
        f.render_widget(
            Paragraph::new("no impactful other users (cutoff: 1s cpu or 1GiB rss)")
                .style(Style::default().fg(Color::DarkGray)),
            uinner,
        );
    } else {
        let mut sorted: Vec<&UserShare> = s.users.iter().collect();
        sort_users(&mut sorted, app.users_sort);
        let visible = uinner.height.saturating_sub(1).max(1) as usize;
        let total = sorted.len();
        if total > visible {
            app.uscroll = app.uscroll.min(total - visible);
        } else {
            app.uscroll = 0;
        }
        let denom = (s.collecting_secs * s.cores as f64).max(1.0);
        let total_cpu: f64 = sorted.iter().map(|u| u.cpu_secs).sum();
        let mut headers: Vec<String> = [
            "#", "user", "cpu", "wait", "rss", "procs", "cores%", "share",
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
            .map(|u| {
                let color = user_color(&u.user);
                let share = if total_cpu > 0.0 {
                    u.cpu_secs / total_cpu * 100.0
                } else {
                    0.0
                };
                let cores_pct = u.cpu_secs / denom * 100.0;
                Row::new(vec![
                    Cell::from(Span::raw("-")),
                    Cell::from(Span::styled(trunc(&u.user, 9), Style::default().fg(color))),
                    Cell::from(Span::raw(fmt_secs(u.cpu_secs))),
                    Cell::from(Span::raw(fmt_secs(u.wait_secs))),
                    Cell::from(Span::raw(fmt_bytes(u.rss))),
                    Cell::from(Span::raw(u.procs.to_string())),
                    Cell::from(Span::raw(fmt_pct(cores_pct))),
                    Cell::from(Span::styled(
                        fmt_pct(share),
                        Style::default().fg(sev(share)),
                    )),
                ])
            })
            .collect();
        let widths = [
            Constraint::Min(3),
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(6),
        ];
        let table = Table::new(rows, widths).header(header);
        f.render_widget(table, users_area);
        render_scrollbar(
            f,
            total,
            app.uscroll,
            visible,
            Rect::new(uinner.x + uinner.width - 1, uinner.y, 1, uinner.height),
        );
    }
    if s.antagonists.is_empty() {
        f.render_widget(
            Paragraph::new("no impactful processes")
                .style(Style::default().fg(Color::DarkGray)),
            ainner,
        );
        return;
    }
    let mut sorted: Vec<&Antag> = s.antagonists.iter().collect();
    sort_ants(&mut sorted, app.ants_sort);
    let visible = ainner.height.saturating_sub(1).max(1) as usize;
    let total = sorted.len();
    if total > visible {
        app.ascroll = app.ascroll.min(total - visible);
    } else {
        app.ascroll = 0;
    }
    let mut headers: Vec<String> = [
        "#", "user", "comm", "cpu", "wait", "rss", "cmdline",
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
    let cmd_w = (ainner.width as usize).saturating_sub(56);
    let rows: Vec<Row> = sorted
        .iter()
        .skip(app.ascroll)
        .take(visible)
        .map(|a| {
            let color = user_color(&a.user);
            Row::new(vec![
                Cell::from(Span::raw("-")),
                Cell::from(Span::styled(trunc(&a.user, 8), Style::default().fg(color))),
                Cell::from(Span::raw(trunc(&a.comm, 12))),
                Cell::from(Span::raw(fmt_secs(a.cpu_secs))),
                Cell::from(Span::raw(fmt_secs(a.wait_secs))),
                Cell::from(Span::raw(fmt_bytes(a.rss))),
                Cell::from(Span::raw(trunc(&a.cmdline, cmd_w))),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(3),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Min(10),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, ants_area);
    render_scrollbar(
        f,
        total,
        app.ascroll,
        visible,
        Rect::new(ainner.x + ainner.width - 1, ainner.y, 1, ainner.height),
    );
}


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

const RUNS_FIXED: [u16; 10] = [6, 6, 6, 6, 7, 11, 6, 6, 6, 7];
const USERS_FIXED: [u16; 7] = [9, 8, 8, 8, 7, 8, 7];
const ANTS_FIXED: [u16; 6] = [3, 8, 12, 9, 8, 8];

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
            1 => a.wall.total_cmp(&b.wall),
            2 => a.cpu_secs.total_cmp(&b.cpu_secs),
            3 => cmp_opt_f64(a.wait_pct, b.wait_pct),
            4 => a.cpu_pct.total_cmp(&b.cpu_pct),
            5 => a.rss.cmp(&b.rss),
            6 => a.users.cmp(&b.users),
            7 => a.psi[0].total_cmp(&b.psi[0]),
            8 => a.psi[1].total_cmp(&b.psi[1]),
            9 => a.psi[2].total_cmp(&b.psi[2]),
            _ => a.alive.cmp(&b.alive),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
}

fn sort_users(rows: &mut Vec<&UserShare>, (col, asc): (usize, bool)) {
    rows.sort_by(|a, b| {
        let ord = match col {
            1 => a.user.cmp(&b.user),
            2 => a.cpu_secs.total_cmp(&b.cpu_secs),
            3 => a.wait_secs.total_cmp(&b.wait_secs),
            4 => a.rss.cmp(&b.rss),
            5 => a.procs.cmp(&b.procs),
            _ => a.cpu_secs.total_cmp(&b.cpu_secs),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
}

fn sort_ants(rows: &mut Vec<&Antag>, (col, asc): (usize, bool)) {
    rows.sort_by(|a, b| {
        let ord = match col {
            1 => a.user.cmp(&b.user),
            2 => a.comm.cmp(&b.comm),
            3 => a.cpu_secs.total_cmp(&b.cpu_secs),
            4 => a.wait_secs.total_cmp(&b.wait_secs),
            5 => a.rss.cmp(&b.rss),
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

fn render_scrollbar(
    f: &mut Frame,
    total: usize,
    pos: usize,
    viewport: usize,
    bar: Rect,
) {
    if total > viewport {
        let mut st = ScrollbarState::new(total)
            .position(pos.min(total - viewport))
            .viewport_content_length(viewport);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            bar,
            &mut st,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn runs_sort_most_affected_first() {
        let a = run(Some(5.0), 1.0);
        let b = run(Some(50.0), 2.0);
        let c = run(None, 3.0);
        let mut rows = vec![&a, &c, &b];
        sort_runs(&mut rows, (3, false));
        assert_eq!(rows[0].wait_pct, Some(50.0));
        assert_eq!(rows[1].wait_pct, Some(5.0));
        assert_eq!(rows[2].wait_pct, None);
    }

    #[test]
    fn runs_sort_ascending() {
        let a = run(Some(5.0), 1.0);
        let b = run(Some(50.0), 2.0);
        let mut rows = vec![&b, &a];
        sort_runs(&mut rows, (3, true));
        assert_eq!(rows[0].wait_pct, Some(5.0));
    }

    #[test]
    fn runs_sort_by_psi() {
        let a = run(Some(1.0), 1.0);
        let b = run(Some(1.0), 9.0);
        let mut rows = vec![&a, &b];
        sort_runs(&mut rows, (7, false));
        assert_eq!(rows[0].psi[0], 9.0);
    }

    #[test]
    fn col_at_maps_clicks() {
        let area = Rect::new(10, 0, 100, 5);
        assert_eq!(col_at(11, &area, &RUNS_FIXED, 0), Some(0));
        let wall_start = 10 + (100 - 77); // params min width
        assert_eq!(col_at(wall_start + 2, &area, &RUNS_FIXED, 0), Some(1));
        assert_eq!(col_at(10 + 99, &area, &RUNS_FIXED, 0), Some(10));
        assert_eq!(col_at(5, &area, &RUNS_FIXED, 0), None);
    }

    #[test]
    fn col_at_users_mapping() {
        let area = Rect::new(0, 0, 70, 5);
        let min_w: u16 = 70 - USERS_FIXED.iter().sum::<u16>() - 8;
        assert_eq!(col_at(min_w - 1, &area, &USERS_FIXED, 0), Some(0));
        let mut cur = min_w + 1;
        for (i, w) in USERS_FIXED.iter().enumerate() {
            assert_eq!(col_at(cur + w - 1, &area, &USERS_FIXED, 0), Some(i + 1));
            cur += w + 1;
        }
        assert_eq!(col_at(69, &area, &USERS_FIXED, 0), Some(7));
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
}

