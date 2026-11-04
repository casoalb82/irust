use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, EventStream, KeyCode,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Terminal;
use tokio::sync::{mpsc, Mutex};

use irust_core::{Buffer, BufferId, Servers};

use crate::widgets::nicklist::NicklistRow;
use crate::widgets::{buffer_view, buflist, input, nicklist, topic_bar, window_list};

const QUIT_ACK_TIMEOUT: Duration = Duration::from_millis(1500);
const UPGRADE_ACK_TIMEOUT: Duration = Duration::from_secs(130);
const QUIT_MESSAGE_MIN_VISIBLE: Duration = Duration::from_millis(300);
/// Lines moved per mouse-wheel notch -- deliberately smaller than
/// a whole `PageUp`/`PageDown` page (`/set scroll_page_count`), matching ordinary
/// terminal-app wheel behavior (a few lines per notch, not a full page).
const SCROLL_WHEEL_LINES: usize = 3;
const DEFAULT_VIEW_HEIGHT: usize = 20;

/// Lines one `PageUp`/`PageDown` moves: `/set scroll_page_count` against
/// the main pane's height, irssi's `get_scroll_count`.
fn page_scroll_lines(view_height: usize) -> usize {
    let setting = irust_config::settings_history_log::get_str("scroll_page_count");
    irust_config::settings_history_log::scroll_page_lines(&setting, view_height)
}
const MIN_RAW_PASTE_QUEUE_LEN: usize = 3;
#[allow(dead_code)]
const RAW_PASTE_GRACE_WAIT: Duration = Duration::from_millis(150);
const MAX_RAW_PASTE_LEN: usize = 8192;
/// Smaller than a `PageUp`/`PageDown` page -- the nicklist sidebar is usually
/// much shorter than the chat pane, so a chat-sized page would often
/// overshoot the whole list in one keypress.
const NICKLIST_SCROLL_STEP: usize = 5;
const MIN_BELL_INTERVAL: Duration = Duration::from_millis(1000);
const MIN_REDRAW_INTERVAL: Duration = Duration::from_millis(50);

const REDRAW_DEBOUNCE: Duration = Duration::from_millis(15);

/// One input-recall history -- either the single shared one (`/set
/// window_history off`, the default) or one window's own (`on`). See
/// `event_loop`'s own `global_history`/`per_window_history` doc
/// comment for the full picture.
type HistoryState = irust_core::command_history::CommandHistory;

/// `/set max_command_history` / `/set command_history_editable`, read
/// live each time so a `/set` takes effect on the next keypress.
fn history_cap() -> i64 {
    irust_config::settings_history_log::get_int("max_command_history")
}

fn history_editable() -> bool {
    irust_config::settings_history_log::get_bool("command_history_editable")
}

async fn active_history_key(servers: &Arc<Mutex<Servers>>) -> Option<(String, BufferId)> {
    let guard = servers.lock().await;
    let tag = guard.active_tag()?.to_string();
    let buffer = guard.active_state()?.active_buffer.clone();
    Some((tag, buffer))
}

/// Resolves which `HistoryState` an input-recall action should
/// actually read/mutate right now -- the shared `global` one when
/// `/set window_history` is off (irust's own original behavior,
/// unchanged) or there's no active window to key by, otherwise
/// `per_window`'s own entry for `key` (created empty on first use).
fn history_state<'a>(
    key: Option<&(String, BufferId)>,
    global: &'a mut HistoryState,
    per_window: &'a mut HashMap<(String, BufferId), HistoryState>,
) -> &'a mut HistoryState {
    if !irust_config::settings::WINDOW_HISTORY.load(std::sync::atomic::Ordering::Relaxed) {
        return global;
    }
    match key {
        Some(k) => per_window.entry(k.clone()).or_default(),
        None => global,
    }
}

fn pasted_text_to_input(data: &str) -> String {
    let mut lines: Vec<&str> = data.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    if lines.len() <= 1 {
        lines.first().copied().unwrap_or(data).to_string()
    } else {
        lines
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn pasted_command_lines(data: &str) -> Option<Vec<String>> {
    let mut lines: Vec<&str> = data.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let trimmed: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if trimmed.len() >= 2 && trimmed.iter().all(|l| l.starts_with('/')) {
        Some(trimmed.into_iter().map(str::to_string).collect())
    } else {
        None
    }
}

fn canonical_key_name(code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('h') => return Some("backspace".to_string()),
            KeyCode::Char(c) => return Some(format!("ctrl-{}", c.to_ascii_lowercase())),
            KeyCode::Left => return Some("ctrl-left".to_string()),
            KeyCode::Right => return Some("ctrl-right".to_string()),
            _ => {}
        }
    }
    if modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(c) = code {
            return Some(format!("alt-{}", c.to_ascii_lowercase()));
        }
    }
    let name = match code {
        KeyCode::Tab => "tab",
        KeyCode::Enter => "enter",
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        KeyCode::Left => "left",
        KeyCode::Right => "right",
        KeyCode::PageUp => "pageup",
        KeyCode::PageDown => "pagedown",
        KeyCode::Home => "home",
        KeyCode::End => "end",
        KeyCode::Backspace => "backspace",
        KeyCode::Delete => "delete",
        KeyCode::Esc => "esc",
        _ => return None,
    };
    Some(name.to_string())
}

/// State for one Tab-completion chain (see `event_loop`'s
/// `tab_completion`/`last_action_was_tab`).
struct TabCompletion {
    word_start: usize,
    /// Full replacement words (completion char / kept typed word included).
    candidates: Vec<String>,
    index: usize,
}

/// irssi's `word_complete` tail: replaces `word_start..end` with `cand`
/// and leaves the cursor after one separating space (inserted unless the
/// text already continues with a space). Returns the new cursor.
fn insert_completion(buf: &mut String, word_start: usize, end: usize, cand: &str) -> usize {
    buf.replace_range(word_start..end, cand);
    let pos = word_start + cand.len();
    if !buf[pos..].starts_with(' ') {
        buf.insert(pos, ' ');
    }
    pos + 1
}

pub async fn run(
    servers: Arc<Mutex<Servers>>,
    outgoing: mpsc::UnboundedSender<String>,
    mut redraw_rx: mpsc::UnboundedReceiver<()>,
    mut force_redraw_rx: mpsc::UnboundedReceiver<()>,
    mut bell_rx: mpsc::UnboundedReceiver<()>,
    quit_ack_rx: mpsc::UnboundedReceiver<String>,
    restart_rx: mpsc::UnboundedReceiver<()>,
    completions: Arc<Mutex<HashMap<String, String>>>,
    keybinds: Arc<Mutex<HashMap<String, String>>>,
    show_startup_banner: bool,
) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(
        &mut terminal,
        servers,
        outgoing,
        &mut redraw_rx,
        &mut force_redraw_rx,
        &mut bell_rx,
        quit_ack_rx,
        restart_rx,
        completions,
        keybinds,
        show_startup_banner,
    )
    .await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    result
}

/// irssi's own startup banner (`fe-text/module-formats.c`'s
/// `irssi_banner` format, `Ii` monogram in the "smslant" figlet font)
/// prints once, right after init, only if nothing was configured to
/// autoconnect. Same idea here, rendered with the same font on "irust".
const IRUST_BANNER: &[&str] = &[
    "   _              __",
    "  (_)_____ _____ / /_",
    " / / __/ // (_-</ __/",
    "/_/_/  \\_,_/___/\\__/",
];

pub fn banner_lines() -> Vec<String> {
    let mut lines: Vec<String> = IRUST_BANNER.iter().map(|s| s.to_string()).collect();
    lines.push(format!("irust v{}", env!("CARGO_PKG_VERSION")));
    lines.push("by abcX".to_string());
    // See `irust_config::settings::init_user_defaults`'s own doc
    // comment -- irssi-parity notice, shown only on the run that
    // actually filled in nick/ident/realname for the first time.
    lines.extend(irust_config::settings::user_defaults_notice());
    // See `irust_config::warnings`'s own doc comment -- a malformed
    // config/aliases/ignores/etc. file used to fall back to defaults
    // with nothing on screen explaining why.
    lines.extend(irust_config::startup_warnings());
    lines
}

fn no_server_buffer(show_banner: bool, pending_messages: &[(bool, String)]) -> Buffer {
    let mut buf = Buffer::new("(no server)".to_string());
    if show_banner {
        for line in banner_lines() {
            buf.push_system(line);
        }
    }
    buf.push_system("-- not connected to any server -- /connect <network> to get started".to_string());
    for (warning, message) in pending_messages {
        if *warning {
            buf.push_system_warning(message.clone());
        } else {
            buf.push_system(message.clone());
        }
    }
    buf
}

async fn deadline_or_pending(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    servers: Arc<Mutex<Servers>>,
    outgoing: mpsc::UnboundedSender<String>,
    redraw_rx: &mut mpsc::UnboundedReceiver<()>,
    force_redraw_rx: &mut mpsc::UnboundedReceiver<()>,
    bell_rx: &mut mpsc::UnboundedReceiver<()>,
    quit_ack_rx: mpsc::UnboundedReceiver<String>,
    mut restart_rx: mpsc::UnboundedReceiver<()>,
    completions: Arc<Mutex<HashMap<String, String>>>,
    keybinds: Arc<Mutex<HashMap<String, String>>>,
    mut show_startup_banner: bool,
) -> anyhow::Result<()> {
    let mut input_buf = String::new();
    let mut cursor: usize = 0;
    // Single-slot cutbuffer for `Ctrl-W`/`Ctrl-K`/`kill_to_start` +
    // `Ctrl-Y` -- irssi's own cutbuffer is a single slot too, no ring.
    let mut kill_buffer = String::new();
    let mut nicklist_scroll: usize = 0;
    let mut last_bell: Option<Instant> = None;
    let mut last_redraw_at: Instant = Instant::now();
    let mut events = EventStream::new();
    let mut pending_events: VecDeque<crossterm::event::Event> = VecDeque::new();
    let mut window_list_area = Rect::default();
    let mut window_list_summaries: Vec<irust_core::BufferSummary> = Vec::new();
    let mut window_list_multi = false;
    let mut window_list_active: Option<usize> = None;
    let mut buffer_view_area = Rect::default();
    // Where each `/split` pane last landed on screen, paired with the
    // buffer it's showing -- read back when a mouse wheel event needs
    // to know which pane (focused or which split) it landed over. The
    // focused pane's own area is tracked separately, above
    // (`buffer_view_area`) -- this is only the OTHER panes.
    let mut split_pane_areas: Vec<(irust_core::BufferId, Rect)> = Vec::new();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    let mut global_history = HistoryState::new();
    let mut main_view_height: usize = DEFAULT_VIEW_HEIGHT;
    let mut per_window_history: HashMap<(String, BufferId), HistoryState> = HashMap::new();

    // Tab nick completion, irssi-style: type a prefix, Tab completes it
    // (with a trailing ": " if it's the line's first word, plain " "
    // otherwise), Tab again cycles through the rest of the matches.
    // `last_action_was_tab` gates cycling vs. starting a fresh
    // completion -- any other key breaks the chain.
    let mut tab_completion: Option<TabCompletion> = None;
    let mut last_action_was_tab = false;
    // See `REDRAW_DEBOUNCE`'s doc comment. `should_draw` gates the
    // lock-acquire-and-render block below each iteration; only the
    // `redraw_rx` arm ever sets it `false` (while it's debouncing a
    // burst), every other `select!` arm leaves it at its default `true`
    // so their existing "redraw right away" behavior is unchanged.
    let mut should_draw = true;
    let mut pending_redraw_deadline: Option<tokio::time::Instant> = None;

    loop {
        if should_draw {
            let lock_wait_start = Instant::now();
            let mut guard = servers.lock().await;
            let lock_wait = lock_wait_start.elapsed();
            if lock_wait > Duration::from_millis(200) {
                irust_config::debug_log::line(
                    "LOCK",
                    "-",
                    &format!("render loop waited {lock_wait:?} to acquire servers"),
                );
            }
            let multi_server = guard.tags().len() > 1;
            let server_tag = guard.active_tag().map(str::to_string);
            let pending_messages = guard.pending_messages().to_vec();
            if irust_config::lookandfeel::get_bool("activity_hide_visible") {
                if let Some(st) = guard.active_state_mut() {
                    st.mark_current_buffer_read();
                }
            }
            // irssi-style one window list across every server: built
            // here, before `st` mutably borrows the guard. A status
            // window is labelled with its server tag (and numbered) so
            // the servers are told apart.
            let global_list: Option<(Vec<irust_core::BufferSummary>, Option<usize>)> = if multi_server {
                let wins = guard.global_windows();
                let active_idx = guard.active_global_index();
                let list = wins
                    .into_iter()
                    .map(|w| {
                        let mut s = w.summary;
                        if matches!(s.id, BufferId::Status) {
                            // A dropped server (waiting to reconnect)
                            // says so in the list instead of looking
                            // exactly like a healthy one.
                            let online = guard.state(&w.tag).is_none_or(|st| st.connected);
                            let label = if online { w.tag.clone() } else { format!("{}(off)", w.tag) };
                            s.display_name = label;
                        }
                        s
                    })
                    .collect();
                Some((list, active_idx))
            } else {
                None
            };
            let mut st = guard.active_state_mut();

            let summaries = st.as_deref().map(|s| s.buffer_summaries()).unwrap_or_default();
            let active = st
                .as_deref()
                .map(|s| s.active_buffer.clone())
                .unwrap_or(BufferId::Status);
            let active_channel_entries = st.as_deref().and_then(|s| match &s.active_buffer {
                irust_core::BufferId::Channel(key) => s.channels.get(key).map(|c| {
                    c.nicklist
                        .sorted()
                        .into_iter()
                        .map(|e| (e.nick, c.nicklist.prefix_char(e.rank), e.away))
                        .collect::<Vec<_>>()
                }),
                irust_core::BufferId::Status | irust_core::BufferId::Query(_) | irust_core::BufferId::Empty(_) => None,
            });
            let active_nicklist: Option<Vec<NicklistRow>> = active_channel_entries.map(|entries| {
                entries
                    .into_iter()
                    .map(|(nick, prefix, away)| {
                        let color_index = st.as_deref_mut().map(|s| s.nick_colors.resolve(&nick)).unwrap_or(0);
                        NicklistRow {
                            nick,
                            prefix,
                            color_index,
                            away,
                        }
                    })
                    .collect()
            });
            let st = st.map(|s| &*s);
            let active_topic: Option<String> = st.as_deref().and_then(|s| match &s.active_buffer {
                irust_core::BufferId::Channel(key) => s.channels.get(key).and_then(|c| c.topic.clone()),
                irust_core::BufferId::Status | irust_core::BufferId::Query(_) | irust_core::BufferId::Empty(_) => None,
            });
            if st.is_some() {
                show_startup_banner = false;
            }
            let no_server_buf = no_server_buffer(show_startup_banner, &pending_messages);
            let current_buffer = st.map(|s| s.current_buffer()).unwrap_or(&no_server_buf);
            let scroll_offset = st
                .map(|s| s.active_scroll_offset)
                .unwrap_or(0)
                .min(current_buffer.lines.len().saturating_sub(1));
            irust_config::debug_log::line(
                "DRAW",
                server_tag.as_deref().unwrap_or("-"),
                &format!("buffer={} lines={}", current_buffer.name, current_buffer.lines.len()),
            );
            terminal.draw(|frame| {
                let list_summaries: &[irust_core::BufferSummary] =
                    global_list.as_ref().map(|g| g.0.as_slice()).unwrap_or(&summaries);
                let list_active = match &global_list {
                    Some((_, idx)) => *idx,
                    None => summaries.iter().position(|s| s.id == active),
                };
                let window_list_height = if multi_server {
                    window_list::required_lines_multi(list_summaries, frame.area().width, list_active)
                } else {
                    window_list::required_lines(list_summaries, frame.area().width)
                };
                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Min(1),
                        Constraint::Length(window_list_height),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .split(frame.area());

                topic_bar::render(frame, rows[0], active_topic.as_deref());

                let split_panes: Vec<irust_core::SplitPane> = st.map(|s| s.split_panes.clone()).unwrap_or_default();
                let pane_count = 1 + split_panes.len();
                let pane_areas: Vec<Rect> = if split_panes.is_empty() {
                    vec![rows[1]]
                } else {
                    let direction = match st.map(|s| s.split_orientation).unwrap_or_default() {
                        irust_core::SplitOrientation::Horizontal => Direction::Vertical,
                        irust_core::SplitOrientation::Vertical => Direction::Horizontal,
                    };
                    Layout::default()
                        .direction(direction)
                        .constraints(vec![Constraint::Ratio(1, pane_count as u32); pane_count])
                        .split(rows[1])
                        .to_vec()
                };
                let main_area = pane_areas[0];
                main_view_height = main_area.height as usize;

                let text_colors = st.map(|s| s.text_colors).unwrap_or(false);
                if let Some(chan_nicklist) = &active_nicklist {
                    let main_row = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Min(20), Constraint::Length(16)])
                        .split(main_area);
                    buffer_view_area = main_row[0];
                    buffer_view::render(frame, main_row[0], current_buffer, scroll_offset, text_colors, false);
                    nicklist_scroll = nicklist::render(
                        frame,
                        main_row[1],
                        chan_nicklist,
                        st.map(|s| s.nicklist_colors).unwrap_or(false),
                        current_buffer.shown_name(),
                        nicklist_scroll,
                    );
                } else {
                    // Query buffers don't have a channel nicklist to show
                    // alongside them -- give the scrollback the full width.
                    buffer_view_area = main_area;
                    buffer_view::render(frame, main_area, current_buffer, scroll_offset, text_colors, true);
                }
                split_pane_areas.clear();
                for (i, pane) in split_panes.iter().enumerate() {
                    let Some(area) = pane_areas.get(i + 1).copied() else {
                        continue;
                    };
                    let Some(buf) = st.and_then(|s| s.buffer(&pane.buffer)) else {
                        continue;
                    };
                    buffer_view::render(frame, area, buf, pane.scroll_offset, text_colors, true);
                    split_pane_areas.push((pane.buffer.clone(), area));
                }

                window_list_area = rows[2];
                window_list_summaries = list_summaries.to_vec();
                window_list_multi = multi_server;
                window_list_active = list_active;
                if multi_server {
                    window_list::render_multi(frame, rows[2], list_summaries, list_active);
                } else {
                    window_list::render(frame, rows[2], list_summaries, &active);
                }

                buflist::render(
                    frame,
                    rows[3],
                    &summaries,
                    &active,
                    buflist::StatusbarInfo {
                        own_nick: st.map(|s| s.nick.as_str()).unwrap_or("irust"),
                        own_modes: st.map(|s| s.own_modes.as_str()).unwrap_or(""),
                        away: st.map(|s| s.away).unwrap_or(false),
                        lag_ms: st.and_then(|s| s.lag_ms),
                        server_tag: server_tag.as_deref().filter(|_| multi_server),
                        global_number: global_list.as_ref().and_then(|g| g.1).map(|i| i + 1),
                    },
                );

                input::render(frame, rows[4], &input_buf, cursor);
            })?;
            last_redraw_at = Instant::now();
        }

        should_draw = true;
        tokio::select! {
            maybe_event = async {
                match pending_events.pop_front() {
                    Some(ev) => Some(Ok(ev)),
                    None => events.next().await,
                }
            } => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        // `/bind` override for this key, falling back to
                        // the built-in default for it (if any) -- `None`
                        // means "not a bindable key at all" (a plain
                        // character) or "no override and no default"
                        // (nothing happens).
                        let key_name = canonical_key_name(key.code, key.modifiers);
                        let action: Option<String> = match &key_name {
                            Some(name) => {
                                let overridden = keybinds.lock().await.get(name).cloned();
                                overridden.or_else(|| {
                                    irust_core::DEFAULT_KEYBINDS
                                        .iter()
                                        .find(|(k, _)| k == name)
                                        .map(|(_, a)| a.to_string())
                                })
                            }
                            None => None,
                        };
                        if action.as_deref() != Some("tab_complete") {
                            last_action_was_tab = false;
                            tab_completion = None;
                        }
                        match action.as_deref() {
                            Some("tab_complete") => {
                                if last_action_was_tab {
                                    if let Some(tc) = &mut tab_completion {
                                        tc.index = (tc.index + 1) % tc.candidates.len();
                                        let cand = tc.candidates[tc.index].clone();
                                        cursor = insert_completion(&mut input_buf, tc.word_start, cursor, &cand);
                                    }
                                } else {
                                    let word_start =
                                        input_buf[..cursor].rfind(' ').map(|i| i + 1).unwrap_or(0);
                                    let prefix = input_buf[word_start..cursor].to_lowercase();
                                    // `/completion` words: an exact (not
                                    // prefix) match on the whole current
                                    // word, checked before nick completion
                                    // -- irssi's own `completion_find`
                                    // priority order.
                                    let completion_match =
                                        if prefix.is_empty() { None } else { completions.lock().await.get(&prefix).cloned() };
                                    if let Some(replacement) = completion_match {
                                        input_buf.replace_range(word_start..cursor, &format!("{replacement} "));
                                        cursor = word_start + replacement.len() + 1;
                                        tab_completion = None;
                                    } else {
                                        let is_channel_prefix = prefix.starts_with(['#', '&', '+', '!']);
                                        let guard = servers.lock().await;
                                        let mut start = word_start;
                                        let mut candidates: Vec<String> = Vec::new();
                                        if let Some(st) = guard.active_state() {
                                            if is_channel_prefix {
                                                candidates = irust_core::nick_completion::with_kept_word(
                                                    irust_core::channel_completion_candidates(&st.channels, &prefix),
                                                    &input_buf[word_start..cursor],
                                                );
                                                candidates.retain(|c| !c.is_empty());
                                            } else {
                                                use irust_core::nick_completion::{tab_candidates, Options, Source, TabInput};
                                                let nick_list: Vec<String> = match &st.active_buffer {
                                                    BufferId::Channel(key) => st
                                                        .channels
                                                        .get(key)
                                                        .map(|c| c.nicklist.sorted().into_iter().map(|e| e.nick).collect())
                                                        .unwrap_or_default(),
                                                    _ => Vec::new(),
                                                };
                                                let no_msgs: &[irust_core::nick_completion::LastMsg] = &[];
                                                let last_msgs = match &st.active_buffer {
                                                    BufferId::Channel(key) => {
                                                        st.channels.get(key).map(|c| c.last_msgs.as_slice()).unwrap_or(no_msgs)
                                                    }
                                                    _ => no_msgs,
                                                };
                                                // A query has exactly one other party --
                                                // its display-cased nick (`Buffer::shown_name`,
                                                // not the lowercased `BufferId::Query` key
                                                // itself) is the only completion candidate.
                                                let query_name = match &st.active_buffer {
                                                    BufferId::Query(key) => {
                                                        st.query_buffers.get(key).map(|b| b.shown_name().to_string())
                                                    }
                                                    _ => None,
                                                };
                                                let source = match (&st.active_buffer, &query_name) {
                                                    (BufferId::Channel(_), _) => Source::Channel {
                                                        nicks: &nick_list,
                                                        own_nick: &st.nick,
                                                        last_msgs,
                                                    },
                                                    (_, Some(name)) => Source::Query { name },
                                                    _ => Source::Status,
                                                };
                                                let cmdchars = irust_config::settings::cmdchars();
                                                let completion_char = irust_config::settings::completion_char();
                                                let found = tab_candidates(
                                                    &TabInput {
                                                        line: &input_buf,
                                                        cursor,
                                                        source,
                                                        privates: &st.private_last_msgs,
                                                        cmdchars: &cmdchars,
                                                        completion_char: &completion_char,
                                                    },
                                                    &Options::from_settings(),
                                                    irust_config::settings_history_log::get_bool("completion_empty_line"),
                                                );
                                                if let Some(r) = found {
                                                    start = r.word_start;
                                                    candidates = r.candidates;
                                                }
                                            }
                                        }
                                        drop(guard);
                                        if !candidates.is_empty() {
                                            cursor = insert_completion(&mut input_buf, start, cursor, &candidates[0]);
                                            tab_completion = Some(TabCompletion { word_start: start, candidates, index: 0 });
                                        }
                                    }
                                }
                                last_action_was_tab = true;
                            }
                            Some("submit") => {
                                let mut peeked_chars: Vec<Event> = Vec::new();
                                while peeked_chars.len() < MIN_RAW_PASTE_QUEUE_LEN {
                                    let Ok(Some(Ok(peeked))) =
                                        tokio::time::timeout(Duration::from_millis(0), events.next()).await
                                    else {
                                        break;
                                    };
                                    let is_plain_char = matches!(
                                        &peeked,
                                        Event::Key(k) if k.kind == KeyEventKind::Press
                                            && matches!(k.code, KeyCode::Char(_))
                                    );
                                    peeked_chars.push(peeked);
                                    if !is_plain_char {
                                        break;
                                    }
                                }
                                let mid_raw_paste = peeked_chars.len() >= MIN_RAW_PASTE_QUEUE_LEN;
                                if mid_raw_paste {
                                    let mut raw = String::new();
                                    for ev in &peeked_chars {
                                        if let Event::Key(k) = ev {
                                            if let KeyCode::Char(c) = k.code {
                                                raw.push(c);
                                            }
                                        }
                                    }
                                    raw.push('\n'); // the Enter that triggered this branch
                                    let mut last_event_at = Instant::now();
                                    loop {
                                        if raw.len() >= MAX_RAW_PASTE_LEN {
                                            break;
                                        }
                                        let grace_wait = Duration::from_millis(
                                            irust_config::settings::PASTE_GRACE_MS
                                                .load(std::sync::atomic::Ordering::Relaxed),
                                        );
                                        let mut next =
                                            tokio::time::timeout(Duration::from_millis(0), events.next()).await;
                                        if !matches!(next, Ok(Some(Ok(_)))) {
                                            tokio::time::sleep(grace_wait).await;
                                            next =
                                                tokio::time::timeout(Duration::from_millis(0), events.next()).await;
                                        }
                                        if !matches!(next, Ok(Some(Ok(_)))) {
                                            irust_config::debug_log::line(
                                                "RAWPASTE",
                                                "-",
                                                &format!(
                                                    "gave up after gap={:?} raw_len={} (paste_grace_ms={:?})",
                                                    last_event_at.elapsed(),
                                                    raw.len(),
                                                    grace_wait
                                                ),
                                            );
                                        }
                                        let Ok(Some(Ok(ev))) = next else { break };
                                        irust_config::debug_log::line(
                                            "RAWPASTE",
                                            "-",
                                            &format!("event after gap={:?} raw_len={}", last_event_at.elapsed(), raw.len()),
                                        );
                                        last_event_at = Instant::now();
                                        match &ev {
                                            Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                                                KeyCode::Char(c) => raw.push(c),
                                                KeyCode::Enter => raw.push('\n'),
                                                _ => {
                                                    pending_events.push_back(ev);
                                                    break;
                                                }
                                            },
                                            _ => {
                                                pending_events.push_back(ev);
                                                break;
                                            }
                                        }
                                    }
                                    let cleaned = pasted_text_to_input(&raw);
                                    cursor = irust_core::line_edit::insert_str(&mut input_buf, cursor, &cleaned);
                                    if !input_buf.is_empty() {
                                        let text = std::mem::take(&mut input_buf);
                                        cursor = 0;
                                        {
                                            let key = active_history_key(&servers).await;
                                            let hist =
                                                history_state(key.as_ref(), &mut global_history, &mut per_window_history);
                                            hist.record_sent(&text, history_cap());
                                        }
                                        let trimmed_text = text.trim_start();
                                        let is_quit = trimmed_text.starts_with("/quit");
                                        let _ = outgoing.send(text);
                                        if is_quit {
                                            return wait_for_quit_ack(terminal, quit_ack_rx, QUIT_ACK_TIMEOUT).await;
                                        }
                                        if let Some(st) = servers.lock().await.active_state_mut() {
                                            st.reset_active_scroll();
                                        }
                                        nicklist_scroll = 0;
                                    }
                                } else {
                                    pending_events.extend(peeked_chars);
                                    if !input_buf.is_empty() {
                                        let text = std::mem::take(&mut input_buf);
                                        cursor = 0;
                                        {
                                            let key = active_history_key(&servers).await;
                                            let hist =
                                                history_state(key.as_ref(), &mut global_history, &mut per_window_history);
                                            hist.record_sent(&text, history_cap());
                                        }
                                        let trimmed_text = text.trim_start();
                                        let is_quit = trimmed_text.starts_with("/quit");
                                        let _ = outgoing.send(text);
                                        if is_quit {
                                            return wait_for_quit_ack(terminal, quit_ack_rx, QUIT_ACK_TIMEOUT).await;
                                        }
                                        if let Some(st) = servers.lock().await.active_state_mut() {
                                            st.reset_active_scroll();
                                        }
                                        nicklist_scroll = 0;
                                    }
                                }
                            }
                            Some("history_up") => {
                                let key = active_history_key(&servers).await;
                                let hist = history_state(key.as_ref(), &mut global_history, &mut per_window_history);
                                input_buf = hist.prev(&input_buf, history_cap(), history_editable());
                                // Lands the cursor at the end of the
                                // recalled line, matching irssi.
                                cursor = input_buf.len();
                            }
                            Some("history_down") => {
                                let key = active_history_key(&servers).await;
                                let hist = history_state(key.as_ref(), &mut global_history, &mut per_window_history);
                                input_buf = hist.next(&input_buf, history_cap(), history_editable());
                                cursor = input_buf.len();
                            }
                            Some("quit") => {
                                let _ = outgoing.send("/quit".to_string());
                                return wait_for_quit_ack(terminal, quit_ack_rx, QUIT_ACK_TIMEOUT).await;
                            }
                            Some("cycle_next") => {
                                // `cycle_buffer` resets `active_scroll_offset`
                                // itself when it actually changes buffer --
                                // see its own doc comment.
                                servers.lock().await.cycle_global(true);
                                nicklist_scroll = 0;
                            }
                            Some("server_next") => {
                                servers.lock().await.cycle_server(true);
                                nicklist_scroll = 0;
                            }
                            Some("server_prev") => {
                                servers.lock().await.cycle_server(false);
                                nicklist_scroll = 0;
                            }
                            Some("cycle_prev") => {
                                servers.lock().await.cycle_global(false);
                                nicklist_scroll = 0;
                            }
                            Some(action) if action.starts_with("goto_buffer_") => {
                                // `goto_buffer_index` resets
                                // `active_scroll_offset` itself, same as
                                // `cycle_buffer` above.
                                if let Some(n) = action.strip_prefix("goto_buffer_").and_then(|s| s.parse().ok()) {
                                    servers.lock().await.goto_global_index(n);
                                }
                                nicklist_scroll = 0;
                            }
                            Some("page_up") => {
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.active_state_mut() {
                                    let total = st.current_buffer().lines.len();
                                    st.active_scroll_offset =
                                        (st.active_scroll_offset + page_scroll_lines(main_view_height)).min(total.saturating_sub(1));
                                }
                            }
                            Some("page_down") => {
                                if let Some(st) = servers.lock().await.active_state_mut() {
                                    st.active_scroll_offset = st.active_scroll_offset.saturating_sub(page_scroll_lines(main_view_height));
                                }
                            }
                            Some("scroll_home") => {
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.active_state_mut() {
                                    let total = st.current_buffer().lines.len();
                                    st.active_scroll_offset = total.saturating_sub(1);
                                }
                            }
                            Some("scroll_end") => {
                                if let Some(st) = servers.lock().await.active_state_mut() {
                                    st.active_scroll_offset = 0;
                                }
                            }
                            Some("nicklist_up") => {
                                nicklist_scroll = nicklist_scroll.saturating_sub(NICKLIST_SCROLL_STEP);
                            }
                            Some("nicklist_down") => {
                                nicklist_scroll = nicklist_scroll.saturating_add(NICKLIST_SCROLL_STEP);
                            }
                            Some("backspace") => {
                                cursor = irust_core::line_edit::backspace(&mut input_buf, cursor);
                            }
                            Some("delete_forward") => {
                                irust_core::line_edit::delete_forward(&mut input_buf, cursor);
                            }
                            Some("cursor_left") => {
                                cursor = irust_core::line_edit::prev_boundary(&input_buf, cursor);
                            }
                            Some("cursor_right") => {
                                cursor = irust_core::line_edit::next_boundary(&input_buf, cursor);
                            }
                            Some("cursor_word_left") => {
                                cursor = irust_core::line_edit::word_left(&input_buf, cursor);
                            }
                            Some("cursor_word_right") => {
                                cursor = irust_core::line_edit::word_right(&input_buf, cursor);
                            }
                            Some("cursor_line_start") => {
                                cursor = 0;
                            }
                            Some("cursor_line_end") => {
                                cursor = input_buf.len();
                            }
                            Some("delete_word_backward") => {
                                let (new_cursor, removed) =
                                    irust_core::line_edit::delete_word_backward(&mut input_buf, cursor);
                                if !removed.is_empty() {
                                    kill_buffer = removed;
                                } else if irust_config::lookandfeel::get_bool("empty_kill_clears_cutbuffer") {
                                    kill_buffer.clear();
                                }
                                cursor = new_cursor;
                            }
                            Some("kill_to_end") => {
                                let removed = irust_core::line_edit::kill_to_end(&mut input_buf, cursor);
                                if !removed.is_empty() {
                                    kill_buffer = removed;
                                } else if irust_config::lookandfeel::get_bool("empty_kill_clears_cutbuffer") {
                                    kill_buffer.clear();
                                }
                            }
                            Some("kill_to_start") => {
                                let removed = irust_core::line_edit::kill_to_start(&mut input_buf, cursor);
                                if !removed.is_empty() {
                                    kill_buffer = removed;
                                } else if irust_config::lookandfeel::get_bool("empty_kill_clears_cutbuffer") {
                                    kill_buffer.clear();
                                }
                                cursor = 0;
                            }
                            Some("yank") => {
                                if !kill_buffer.is_empty() {
                                    cursor = irust_core::line_edit::insert_str(&mut input_buf, cursor, &kill_buffer);
                                }
                            }
                            _ => {
                                if let KeyCode::Char(c) = key.code {
                                    cursor = irust_core::line_edit::insert_char(&mut input_buf, cursor, c);
                                }
                            }
                        }
                    }
                    Some(Ok(Event::Paste(data))) => {
                        if let Some(lines) = pasted_command_lines(&data) {
                            irust_config::debug_log::line(
                                "PASTE",
                                "-",
                                &format!("raw_len={} command_lines={}", data.len(), lines.len()),
                            );
                            for line in lines {
                                {
                                    let key = active_history_key(&servers).await;
                                    let hist =
                                        history_state(key.as_ref(), &mut global_history, &mut per_window_history);
                                    hist.record_sent(&line, history_cap());
                                }
                                let trimmed_line = line.trim_start();
                                // See the other `is_quit` sites' comment: `/upgrade`
                                // no longer short-circuits here, only `restart_rx`
                                // (fired by the backend once an update actually
                                // installs) does.
                                let is_quit = trimmed_line.starts_with("/quit");
                                let _ = outgoing.send(line);
                                if is_quit {
                                    return wait_for_quit_ack(terminal, quit_ack_rx, QUIT_ACK_TIMEOUT).await;
                                }
                                if let Some(st) = servers.lock().await.active_state_mut() {
                                    st.reset_active_scroll();
                                }
                                nicklist_scroll = 0;
                            }
                        } else {
                            irust_config::debug_log::line(
                                "PASTE",
                                "-",
                                &format!("raw_len={} converted_len={}", data.len(), pasted_text_to_input(&data).len()),
                            );
                            let inserted = pasted_text_to_input(&data);
                            cursor = irust_core::line_edit::insert_str(&mut input_buf, cursor, &inserted);
                        }
                    }
                    Some(Ok(Event::Mouse(mouse))) => match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            let mut guard = servers.lock().await;
                            if window_list_multi {
                                match window_list::hit_test_multi(
                                    &window_list_summaries,
                                    window_list_active,
                                    window_list_area,
                                    mouse.column,
                                    mouse.row,
                                ) {
                                    Some(window_list::Hit::Window(i)) => {
                                        guard.goto_global_index(i + 1);
                                    }
                                    // A network tab: switch to that network
                                    // (it keeps the window you last had open).
                                    Some(window_list::Hit::Server(i)) => {
                                        if let Some(w) = guard.global_windows().into_iter().nth(i) {
                                            guard.switch_active(&w.tag);
                                        }
                                    }
                                    None => {}
                                }
                            } else if let Some(i) =
                                window_list::hit_test(&window_list_summaries, window_list_area, mouse.column, mouse.row)
                            {
                                guard.goto_global_index(i + 1);
                            }
                            drop(guard);
                            nicklist_scroll = 0;
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                            let delta: isize =
                                if mouse.kind == MouseEventKind::ScrollUp { SCROLL_WHEEL_LINES as isize } else {
                                    -(SCROLL_WHEEL_LINES as isize)
                                };
                            let in_area = |area: Rect| {
                                mouse.column >= area.x
                                    && mouse.column < area.x + area.width
                                    && mouse.row >= area.y
                                    && mouse.row < area.y + area.height
                            };
                            if in_area(buffer_view_area) {
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.active_state_mut() {
                                    let total = st.current_buffer().lines.len();
                                    st.active_scroll_offset = (st.active_scroll_offset as isize + delta)
                                        .max(0)
                                        .min(total.saturating_sub(1) as isize)
                                        as usize;
                                }
                            } else if let Some((buffer, _)) =
                                split_pane_areas.iter().find(|(_, area)| in_area(*area)).cloned()
                            {
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.active_state_mut() {
                                    let total = st.buffer(&buffer).map(|b| b.lines.len()).unwrap_or(0);
                                    if let Some(pane) = st.split_panes.iter_mut().find(|p| p.buffer == buffer) {
                                        pane.scroll_offset = (pane.scroll_offset as isize + delta)
                                            .max(0)
                                            .min(total.saturating_sub(1) as isize)
                                            as usize;
                                    }
                                }
                            }
                        }
                        _ => {}
                    },
                    Some(Err(_)) | None => return Ok(()),
                    _ => {}
                }
            }
            _ = redraw_rx.recv() => {
                while redraw_rx.try_recv().is_ok() {}
                should_draw = false;
                let now = tokio::time::Instant::now();
                let ceiling = tokio::time::Instant::from_std(last_redraw_at) + MIN_REDRAW_INTERVAL;
                pending_redraw_deadline = Some((now + REDRAW_DEBOUNCE).min(ceiling));
            }
            _ = deadline_or_pending(pending_redraw_deadline) => {
                should_draw = true;
                pending_redraw_deadline = None;
            }
            _ = force_redraw_rx.recv() => {
                while force_redraw_rx.try_recv().is_ok() {}
                match terminal.size() {
                    Ok(size) => {
                        let area = Rect::new(0, 0, size.width, size.height);
                        if let Err(e) = terminal.resize(area) {
                            irust_config::debug_log::line(
                                "REDRAW",
                                "force_redraw",
                                &format!("terminal.resize() failed, skipped: {e}"),
                            );
                        }
                    }
                    Err(e) => {
                        irust_config::debug_log::line(
                            "REDRAW",
                            "force_redraw",
                            &format!("terminal.size() failed, skipped: {e}"),
                        );
                    }
                }
            }
            _ = bell_rx.recv() => {
                let now = Instant::now();
                let should_ring = last_bell.is_none_or(|t| now.duration_since(t) >= MIN_BELL_INTERVAL);
                if should_ring {
                    last_bell = Some(now);
                    // Writing the BEL byte here (not from the net task) is
                    // what keeps this safe: `terminal`'s backend is the
                    // one and only writer of the alternate-screen stdout,
                    // so there's no risk of racing ratatui's own output.
                    let _ = terminal.backend_mut().write_all(b"\x07");
                    let _ = terminal.backend_mut().flush();
                }
            }
            _ = sigterm.recv() => {
                return Ok(());
            }
            _ = restart_rx.recv() => {
                return wait_for_quit_ack(terminal, quit_ack_rx, UPGRADE_ACK_TIMEOUT).await;
            }
        }
    }
}

async fn wait_for_quit_ack(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut quit_ack_rx: mpsc::UnboundedReceiver<String>,
    ack_timeout: Duration,
) -> anyhow::Result<()> {
    let _ = terminal.draw(|frame| {
        let area = frame.area();
        let bottom_row = ratatui::layout::Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(1),
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            ratatui::widgets::Paragraph::new("-- Closing connection...").alignment(ratatui::layout::Alignment::Left),
            bottom_row,
        );
    });
    let ack_wait = tokio::time::timeout(ack_timeout, quit_ack_rx.recv());
    let min_visible = tokio::time::sleep(QUIT_MESSAGE_MIN_VISIBLE);
    let _ = tokio::join!(ack_wait, min_visible);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_completion_adds_one_separating_space_unless_one_follows() {
        let mut buf = "hi ali".to_string();
        let cursor = insert_completion(&mut buf, 3, 6, "alice");
        assert_eq!((buf.as_str(), cursor), ("hi alice ", 9));

        // The cursor sat mid-line before an existing space: no doubled space.
        let mut buf = "hi ali there".to_string();
        let cursor = insert_completion(&mut buf, 3, 6, "alice");
        assert_eq!((buf.as_str(), cursor), ("hi alice there", 9));

        // Cycling replaces the previous candidate plus its space.
        let mut buf = "hi alice ".to_string();
        let cursor = insert_completion(&mut buf, 3, 9, "alfred");
        assert_eq!((buf.as_str(), cursor), ("hi alfred ", 10));
    }

    #[test]
    fn a_single_line_paste_passes_through_unchanged() {
        assert_eq!(pasted_text_to_input("hey there"), "hey there");
    }

    #[test]
    fn a_paste_with_one_trailing_newline_is_still_single_line() {
        assert_eq!(pasted_text_to_input("hey there\n"), "hey there");
    }

    #[test]
    fn a_word_wrapped_sentence_is_joined_into_one_line() {
        let pasted = "In plain words: when you closed a channel\n\
                       (/wc or /part) and then rejoined, irust\n\
                       always reloaded the whole history.\n";
        assert_eq!(
            pasted_text_to_input(pasted),
            "In plain words: when you closed a channel (/wc or /part) and then rejoined, irust always reloaded the whole history."
        );
    }

    #[test]
    fn indentation_on_pasted_lines_does_not_leave_stray_gaps() {
        assert_eq!(pasted_text_to_input("first\n  second\n    third"), "first second third");
    }

    #[test]
    fn blank_lines_in_the_middle_of_a_paste_are_dropped() {
        assert_eq!(pasted_text_to_input("first\n\nsecond"), "first second");
    }

    #[test]
    fn a_large_accidental_paste_still_collapses_to_one_line() {
        let screenshot_like = (0..30).map(|i| format!("│ line {i} │")).collect::<Vec<_>>().join("\n");
        let joined = pasted_text_to_input(&screenshot_like);
        assert_eq!(joined.matches('\n').count(), 0);
        assert!(joined.contains("line 0"));
        assert!(joined.contains("line 29"));
    }

    #[test]
    fn a_paste_of_only_slash_commands_is_split_into_separate_lines() {
        let pasted = "/part #bbbb\n/part #ccc\n/part #ddd\n";
        assert_eq!(
            pasted_command_lines(pasted),
            Some(vec![
                "/part #bbbb".to_string(),
                "/part #ccc".to_string(),
                "/part #ddd".to_string()
            ])
        );
    }

    #[test]
    fn a_single_command_line_paste_is_not_treated_as_a_batch() {
        // Only one line -- nothing to disambiguate from normal typing,
        // falls through to the regular join/insert path.
        assert_eq!(pasted_command_lines("/part #bbbb"), None);
    }

    #[test]
    fn a_sentence_that_merely_mentions_a_command_is_not_a_batch() {
        let pasted = "In plain words: when you closed a channel\n\
                       (/wc or /part) and then rejoined, irust\n\
                       always reloaded the whole history.\n";
        assert_eq!(pasted_command_lines(pasted), None);
    }

    #[test]
    fn a_screenshot_like_paste_is_not_a_command_batch() {
        let screenshot_like = (0..30).map(|i| format!("│ line {i} │")).collect::<Vec<_>>().join("\n");
        assert_eq!(pasted_command_lines(&screenshot_like), None);
    }

    #[test]
    fn blank_lines_between_commands_are_dropped_not_treated_as_a_mix() {
        assert_eq!(
            pasted_command_lines("/part #a\n\n/part #b\n"),
            Some(vec!["/part #a".to_string(), "/part #b".to_string()])
        );
    }
}
