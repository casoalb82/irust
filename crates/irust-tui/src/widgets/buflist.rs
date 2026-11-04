use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use irust_config::statusbar::{Alignment, Item};
use irust_core::{now_hms, BufferId, BufferSummary};

use super::nicklist::prefix_color;

#[derive(Clone, Copy)]
pub struct StatusbarInfo<'a> {
    pub own_nick: &'a str,
    pub own_modes: &'a str,
    pub away: bool,
    pub lag_ms: Option<u64>,
    pub server_tag: Option<&'a str>,
    /// 1-based position in the global (all-servers) window list, when
    /// more than one server is connected -- overrides the per-server
    /// index in the `window` item so it matches the window list bar.
    pub global_number: Option<usize>,
}

/// Which unread buffers show in the Act statusbar item, and under what
/// name. `actlist_prefer_window_name` (irssi default off) picks
/// between the item's own name (`real_name`) and any `/window name`
/// override (`display_name`) -- only this one statusbar item is
/// affected, every other display still prefers the custom name.
fn build_act_items<'a>(summaries: &'a [BufferSummary], active: &BufferId) -> Vec<(usize, &'a str)> {
    let prefer_window_name = irust_config::lookandfeel::get_bool("actlist_prefer_window_name");
    summaries
        .iter()
        .enumerate()
        .filter(|(_, s)| s.has_unread && &s.id != active)
        .map(|(i, s)| {
            let name = if prefer_window_name { s.display_name.as_str() } else { s.real_name.as_str() };
            (i + 1, name)
        })
        .collect()
}

pub fn render(frame: &mut Frame, area: Rect, summaries: &[BufferSummary], active: &BufferId, info: StatusbarInfo) {
    let theme = crate::theme::current();
    let bar_style = Style::default()
        .bg(theme.statusbar_bg)
        .fg(theme.statusbar_fg)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Block::default().style(bar_style), area);

    let active_index = summaries.iter().position(|s| &s.id == active);
    let active_summary = active_index.and_then(|i| summaries.get(i));
    // Numbered like the `window` item's own fallback (position + 1 in
    // this same list) -- irssi's real `act` default shows window
    // numbers, not names.
    let act_items = build_act_items(summaries, active);

    let mut left_spans: Vec<Span<'static>> = Vec::new();
    let mut right_spans: Vec<Span<'static>> = Vec::new();

    for entry in irust_config::statusbar::items() {
        let Some(inner) = render_item(entry.item, active_index, active_summary, &info, &act_items) else {
            continue;
        };
        let target = match entry.alignment {
            Alignment::Left => &mut left_spans,
            Alignment::Right => &mut right_spans,
        };
        if !target.is_empty() {
            target.push(Span::raw(" "));
        }
        target.extend(bracketed(inner));
    }

    let left_width: u16 = left_spans.iter().map(|s| s.content.chars().count() as u16).sum();
    let mut right_width: u16 = right_spans.iter().map(|s| s.content.chars().count() as u16).sum();
    if left_width + right_width > area.width {
        right_spans.clear();
        right_width = 0;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(left_width),
            Constraint::Min(0),
            Constraint::Length(right_width),
        ])
        .split(area);

    frame.render_widget(Paragraph::new(Line::from(left_spans)).style(bar_style), cols[0]);
    frame.render_widget(Paragraph::new(Line::from(right_spans)).style(bar_style), cols[2]);
}

fn render_item(
    item: Item,
    active_index: Option<usize>,
    active_summary: Option<&BufferSummary>,
    info: &StatusbarInfo,
    act_items: &[(usize, &str)],
) -> Option<Vec<Span<'static>>> {
    let StatusbarInfo {
        own_nick,
        own_modes,
        away,
        lag_ms,
        server_tag,
        global_number,
    } = *info;
    match item {
        Item::Clock => Some(vec![Span::raw(now_hms())]),
        Item::User => {
            let mut user_inner = Vec::new();
            if let Some(s) = active_summary {
                if let Some(rank) = s.own_rank {
                    user_inner.push(Span::styled(
                        rank.to_string(),
                        Style::default().fg(prefix_color(rank)).add_modifier(Modifier::BOLD),
                    ));
                }
            }
            user_inner.push(Span::raw(own_nick.to_string()));
            if !own_modes.is_empty() {
                // `sbmode = "%K(%_%G+%n%_$*%K)%n"` -- black parens, bold
                // green "+", the mode letters themselves bold
                // (inheriting the bar's white, not colored).
                user_inner.push(Span::styled(
                    "(",
                    Style::default().fg(Color::Black).add_modifier(Modifier::BOLD),
                ));
                user_inner.push(Span::styled(
                    "+",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ));
                user_inner.push(Span::styled(
                    own_modes.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                user_inner.push(Span::styled(
                    ")",
                    Style::default().fg(Color::Black).add_modifier(Modifier::BOLD),
                ));
            }
            if away {
                user_inner.push(Span::raw(" "));
                user_inner.push(Span::styled(
                    "(",
                    Style::default().fg(Color::Black).add_modifier(Modifier::BOLD),
                ));
                user_inner.push(Span::styled(
                    "zZzZ",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ));
                user_inner.push(Span::styled(
                    ")",
                    Style::default().fg(Color::Black).add_modifier(Modifier::BOLD),
                ));
            }
            Some(user_inner)
        }
        Item::Window => {
            let (i, s) = (active_index?, active_summary?);
            let mut window_inner = vec![Span::raw(format!("{}:", global_number.unwrap_or(i + 1)))];
            if let Some(tag) = server_tag {
                window_inner.push(Span::raw(format!("{tag}/")));
            }
            window_inner.push(Span::raw(s.display_name.clone()));
            Some(window_inner)
        }
        // `lag_min_show` (irssi default: 1sec): below this threshold
        // the item shows nothing at all, same as no lag measurement.
        Item::Lag => lag_ms.filter(|&ms| ms >= irust_config::settings_net::time_ms_of("lag_min_show")).map(|ms| vec![Span::raw(format!("Lag: {ms}ms"))]),
        Item::Act => {
            if act_items.is_empty() {
                None
            } else {
                // irssi's `actlist_names`: off (default) shows bare
                // window numbers ("Act: 2,3"); on, each number is
                // followed by the item's name ("Act: 2:#chat,3:#other").
                let names = irust_config::lookandfeel::get_bool("actlist_names");
                let parts: Vec<String> = act_items
                    .iter()
                    .map(|(n, name)| if names { format!("{n}:{name}") } else { n.to_string() })
                    .collect();
                Some(vec![Span::raw(format!("Act: {}", parts.join(&irust_config::lookandfeel::get("actlist_separator"))))])
            }
        }
    }
}

fn bracketed(inner: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let bracket_style = Style::default().fg(Color::Black).add_modifier(Modifier::BOLD);
    let mut out = vec![Span::styled("[", bracket_style)];
    out.extend(inner);
    out.push(Span::styled("]", bracket_style));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    static STATUSBAR_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock_statusbar() -> std::sync::MutexGuard<'static, ()> {
        STATUSBAR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn cell_style(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Style {
        buf[(x, y)].style()
    }

    fn summary(id: BufferId, name: &str) -> BufferSummary {
        BufferSummary {
            id,
            display_name: name.to_string(),
            real_name: name.to_string(),
            has_unread: false,
            has_unread_message: false,
            has_unread_join: false,
            has_unread_highlight: false,
            highlight_actcolor: None,
            own_rank: None,
        }
    }

    #[test]
    fn the_whole_row_gets_a_blue_background() {
        let _guard = lock_statusbar();
        let summaries = vec![summary(BufferId::Status, "status")];
        let backend = TestBackend::new(30, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &summaries,
                    &BufferId::Status,
                    StatusbarInfo {
                        own_nick: "sh",
                        own_modes: "",
                        away: false,
                        lag_ms: None,
                        server_tag: None,
                        global_number: None,
                    },
                )
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        // Sample a cell in the middle -- past the bracketed segments,
        // still inside the row -- to confirm the fill, not just the
        // text spans themselves.
        assert_eq!(buf[(20, 0)].style().bg, Some(Color::Blue));
    }

    #[test]
    fn segments_are_individually_bracketed() {
        let _guard = lock_statusbar();
        let summaries = vec![summary(BufferId::Status, "status")];
        let backend = TestBackend::new(30, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &summaries,
                    &BufferId::Status,
                    StatusbarInfo {
                        own_nick: "sh",
                        own_modes: "",
                        away: false,
                        lag_ms: None,
                        server_tag: None,
                        global_number: None,
                    },
                )
            })
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains('['), "got: {content:?}");
        assert!(content.contains(']'), "got: {content:?}");
    }

    #[test]
    fn away_marker_is_bold_yellow_not_green() {
        let _guard = lock_statusbar();
        let summaries = vec![summary(BufferId::Status, "status")];
        let backend = TestBackend::new(30, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &summaries,
                    &BufferId::Status,
                    StatusbarInfo {
                        own_nick: "sh",
                        own_modes: "",
                        away: true,
                        lag_ms: None,
                        server_tag: None,
                        global_number: None,
                    },
                )
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let pos = content.find("zZzZ").unwrap() as u16;
        assert_eq!(buf[(pos, 0)].style().fg, Some(Color::Yellow));
    }

    #[test]
    fn a_too_narrow_row_drops_the_right_side_instead_of_cutting_a_bracket() {
        let _guard = lock_statusbar();
        let mut chan = summary(BufferId::Channel("#irust".to_string()), "#Irust");
        chan.own_rank = Some('@');
        let summaries = vec![
            chan,
            summary(BufferId::Status, "status"),
            summary(BufferId::Channel("#a".to_string()), "#chatnet"),
        ];
        let backend = TestBackend::new(35, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &summaries,
                    &BufferId::Channel("#irust".to_string()),
                    StatusbarInfo {
                        own_nick: "sh",
                        own_modes: "WZaioswxz",
                        away: false,
                        lag_ms: Some(38),
                        server_tag: None,
                        global_number: None,
                    },
                )
            })
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            !content.contains("Irust[Lag"),
            "left side's closing bracket got eaten: {content:?}"
        );
    }

    /// This rank prefix used to be bold-only
    /// (irssi-theme-accurate but colorless); now it should match the
    /// nicklist's own fixed rank colors (green `@`, yellow `+`).
    #[test]
    fn own_rank_prefix_is_colored_like_the_nicklist() {
        let _guard = lock_statusbar();
        let mut chan = summary(BufferId::Channel("#chatnet".to_string()), "#chatnet");
        chan.own_rank = Some('@');
        let summaries = vec![chan];
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &summaries,
                    &BufferId::Channel("#chatnet".to_string()),
                    StatusbarInfo {
                        own_nick: "sh",
                        own_modes: "",
                        away: false,
                        lag_ms: None,
                        server_tag: None,
                        global_number: None,
                    },
                )
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let at_pos = content.find('@').unwrap() as u16;
        assert_eq!(cell_style(buf, at_pos, 0).fg, Some(Color::LightGreen));
    }

    #[test]
    fn own_rank_shows_before_the_nick_in_the_active_channel() {
        let _guard = lock_statusbar();
        let mut chan = summary(BufferId::Channel("#chatnet".to_string()), "#chatnet");
        chan.own_rank = Some('@');
        let summaries = vec![chan];
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &summaries,
                    &BufferId::Channel("#chatnet".to_string()),
                    StatusbarInfo {
                        own_nick: "sh",
                        own_modes: "",
                        away: false,
                        lag_ms: None,
                        server_tag: None,
                        global_number: None,
                    },
                )
            })
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("@sh"), "got: {content:?}");
    }

    #[test]
    fn removing_an_item_via_statusbar_actually_hides_it_from_the_render() {
        let _guard = lock_statusbar();
        let mut sb = irust_config::statusbar::Statusbar::default();
        sb.remove("act").unwrap();

        let mut unread = summary(BufferId::Channel("#other".to_string()), "#other");
        unread.has_unread = true;
        let summaries = vec![summary(BufferId::Status, "status"), unread];
        let backend = TestBackend::new(60, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &summaries,
                    &BufferId::Status,
                    StatusbarInfo {
                        own_nick: "sh",
                        own_modes: "",
                        away: false,
                        lag_ms: None,
                        server_tag: None,
                        global_number: None,
                    },
                )
            })
            .unwrap();

        sb.reset();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            !content.contains("Act:"),
            "removed item must not render, got: {content:?}"
        );
    }

    #[test]
    fn actlist_separator_controls_how_act_entries_are_joined() {
        use irust_config::lookandfeel::with_test_overrides;
        let info = StatusbarInfo {
            own_nick: "n",
            own_modes: "",
            away: false,
            lag_ms: None,
            server_tag: None,
            global_number: None,
        };
        let text = |settings: &[(&str, &str)], items: &[(usize, &str)]| -> String {
            with_test_overrides(settings, || {
                render_item(Item::Act, None, None, &info, items).unwrap().iter().map(|s| s.content.as_ref()).collect()
            })
        };
        assert_eq!(text(&[("actlist_names", "true")], &[(1, "#a"), (2, "#b")]), "Act: 1:#a 2:#b", "default separator, names on");
        assert_eq!(
            text(&[("actlist_separator", ","), ("actlist_names", "true")], &[(1, "#a"), (2, "#b"), (3, "#c")]),
            "Act: 1:#a,2:#b,3:#c"
        );
    }

    #[test]
    fn actlist_prefer_window_name_off_by_default_shows_the_item_name_not_the_window_override() {
        use irust_config::lookandfeel::with_test_overrides;
        let mut renamed = summary(BufferId::Channel("chan".to_string()), "#chan");
        renamed.display_name = "my custom name".to_string();
        renamed.has_unread = true;
        let summaries = vec![summary(BufferId::Status, "status"), renamed];
        let active = BufferId::Status;

        let off = with_test_overrides(&[], || build_act_items(&summaries, &active));
        assert_eq!(off, vec![(2, "#chan")], "default: Act shows the item's own name");

        let on =
            with_test_overrides(&[("actlist_prefer_window_name", "true")], || build_act_items(&summaries, &active));
        assert_eq!(on, vec![(2, "my custom name")], "on: Act shows the window override");
    }

    #[test]
    fn actlist_names_off_by_default_shows_bare_numbers() {
        use irust_config::lookandfeel::with_test_overrides;
        let info = StatusbarInfo {
            own_nick: "n",
            own_modes: "",
            away: false,
            lag_ms: None,
            server_tag: None,
            global_number: None,
        };
        let text = |items: &[(usize, &str)]| -> String {
            with_test_overrides(&[], || {
                render_item(Item::Act, None, None, &info, items).unwrap().iter().map(|s| s.content.as_ref()).collect()
            })
        };
        assert_eq!(text(&[(2, "#a"), (5, "#b")]), "Act: 2 5", "irssi's real default: numbers only, no names");
    }

    #[test]
    fn reordering_via_statusbar_changes_the_rendered_order() {
        let _guard = lock_statusbar();
        let mut sb = irust_config::statusbar::Statusbar::default();
        sb.remove("window").unwrap();
        sb.add("window", Some("clock"), None, None).unwrap();

        let mut chan = summary(BufferId::Channel("#chatnet".to_string()), "#chatnet");
        chan.own_rank = None;
        let summaries = vec![chan];
        let backend = TestBackend::new(60, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &summaries,
                    &BufferId::Channel("#chatnet".to_string()),
                    StatusbarInfo {
                        own_nick: "sh",
                        own_modes: "",
                        away: false,
                        lag_ms: None,
                        server_tag: None,
                        global_number: None,
                    },
                )
            })
            .unwrap();

        sb.reset();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // Default order is clock, user, window -- "window" (here,
        // "1:#chatnet") now comes BEFORE "sh" (the user item).
        let window_pos = content.find("1:#chatnet").unwrap();
        let user_pos = content.find("sh").unwrap();
        assert!(
            window_pos < user_pos,
            "window should now render before user, got: {content:?}"
        );
    }
}
