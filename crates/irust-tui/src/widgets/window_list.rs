use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use irust_core::{BufferId, BufferSummary};

use super::nicklist::prefix_color;

const MAX_LINES: u16 = 5;

fn entry_label(i: usize, s: &BufferSummary) -> String {
    if matches!(s.id, BufferId::Status) {
        s.display_name.clone()
    } else {
        let rank = s.own_rank.map(|c| c.to_string()).unwrap_or_default();
        format!("{} {rank}{}", i + 1, s.display_name)
    }
}

/// Groups buffer indices into rows that fit `width` columns (one
/// space between entries per row), capped at [`MAX_LINES`] rows.
fn wrap_rows(summaries: &[BufferSummary], width: u16) -> Vec<Vec<usize>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut row_width: u16 = 0;

    for (i, s) in summaries.iter().enumerate() {
        let label_width = entry_label(i, s).chars().count() as u16;
        let fits_current_row = row_width == 0 || row_width + 1 + label_width <= width;

        if !fits_current_row {
            if rows.len() as u16 >= MAX_LINES {
                break;
            }
            rows.push(Vec::new());
            row_width = 0;
        }

        row_width += if row_width == 0 { label_width } else { 1 + label_width };
        rows.last_mut().unwrap().push(i);
    }

    rows
}

/// How many rows [`render`] will use for this width -- the caller
/// needs this *before* building the surrounding `Layout`, since
/// ratatui constraints are fixed up front.
pub fn required_lines(summaries: &[BufferSummary], width: u16) -> u16 {
    wrap_rows(summaries, width).len() as u16
}

pub fn render(frame: &mut Frame, area: Rect, summaries: &[BufferSummary], active: &BufferId) {
    let lines: Vec<Line> = wrap_rows(summaries, area.width)
        .iter()
        .map(|row| {
            let mut spans = Vec::new();
            for (n, &i) in row.iter().enumerate() {
                if n > 0 {
                    spans.push(Span::raw(" "));
                }
                let s = &summaries[i];
                spans.extend(entry_spans(i, s, &s.id == active));
            }
            Line::from(spans)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}

fn entry_spans(i: usize, s: &BufferSummary, is_active: bool) -> Vec<Span<'static>> {
    let mut style = activity_style(s);
    if is_active {
        style = style.bg(Color::Red);
    }
    if matches!(s.id, BufferId::Status) {
        return vec![Span::styled(s.display_name.clone(), style)];
    }
    let mut spans = vec![Span::styled(format!("{} ", i + 1), style)];
    if let Some(rank) = s.own_rank {
        let mut rank_style = Style::default().fg(prefix_color(rank));
        if is_active {
            rank_style = rank_style.bg(Color::Red);
        }
        spans.push(Span::styled(rank.to_string(), rank_style));
    }
    spans.push(Span::styled(s.display_name.clone(), style));
    spans
}

pub fn hit_test(summaries: &[BufferSummary], area: Rect, x: u16, y: u16) -> Option<usize> {
    if x < area.x || x >= area.x + area.width || y < area.y || y >= area.y + area.height {
        return None;
    }
    let row_idx = (y - area.y) as usize;
    let row = wrap_rows(summaries, area.width).into_iter().nth(row_idx)?;
    let mut col = area.x;
    for (n, i) in row.into_iter().enumerate() {
        if n > 0 {
            col += 1;
        }
        let label_width = entry_label(i, &summaries[i]).chars().count() as u16;
        if x >= col && x < col + label_width {
            return Some(i);
        }
        col += label_width;
    }
    None
}


/// Rows for the network tab row itself (wraps only on a very narrow
/// terminal).
const MAX_TAB_LINES: u16 = 2;

/// What a click on the multi-server window list landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// `summaries[i]`, a window of the active network.
    Window(usize),
    /// A network tab; `i` is the index of that network's status window
    /// in `summaries` (the start of its group).
    Server(usize),
}

struct Cell {
    hit: Hit,
    spans: Vec<Span<'static>>,
    width: u16,
}

impl Cell {
    fn new(hit: Hit, spans: Vec<Span<'static>>) -> Self {
        let width = spans.iter().map(|s| s.content.chars().count() as u16).sum();
        Cell { hit, spans, width }
    }
}

/// Index ranges of each network's windows: `summaries` is the global
/// list, where every network's group starts at its `Status` window.
fn server_groups(summaries: &[BufferSummary]) -> Vec<std::ops::Range<usize>> {
    let starts: Vec<usize> = summaries
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.id, BufferId::Status))
        .map(|(i, _)| i)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| start..starts.get(n + 1).copied().unwrap_or(summaries.len()))
        .collect()
}

/// The activity style of the most urgent window in a group (mention >
/// message > other activity), so a collapsed network's tab still says
/// "something happened in here".
fn group_activity_style(group: &[BufferSummary]) -> Style {
    group
        .iter()
        .max_by_key(|s| (s.has_unread_highlight, s.has_unread_message, s.has_unread_join, s.has_unread))
        .map(activity_style)
        .unwrap_or_default()
}

fn pack(cells: Vec<Cell>, width: u16, max_rows: u16) -> Vec<Vec<Cell>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<Cell>> = vec![Vec::new()];
    let mut used: u16 = 0;
    for cell in cells {
        let fits = used == 0 || used + 1 + cell.width <= width;
        if !fits {
            if rows.len() as u16 >= max_rows {
                break;
            }
            rows.push(Vec::new());
            used = 0;
        }
        used += if used == 0 { cell.width } else { 1 + cell.width };
        rows.last_mut().unwrap().push(cell);
    }
    rows
}

/// The single source of truth for both drawing and click-mapping:
/// `active_idx` is the focused window's position in `summaries`.
fn multi_layout(summaries: &[BufferSummary], width: u16, active_idx: Option<usize>) -> Vec<Vec<Cell>> {
    let groups = server_groups(summaries);
    if groups.is_empty() {
        return vec![Vec::new()];
    }
    let active_group = active_idx
        .and_then(|a| groups.iter().position(|g| g.contains(&a)))
        .unwrap_or(0);

    let tabs: Vec<Cell> = groups
        .iter()
        .enumerate()
        .map(|(gi, g)| {
            let name = &summaries[g.start].display_name;
            let (text, style) = if gi == active_group {
                (format!("[{name}]"), Style::default().add_modifier(Modifier::BOLD).bg(Color::Red))
            } else {
                // Background network: `[name +N]` = N windows with real
                // messages waiting, `!` if one of them mentions you.
                let group = &summaries[g.clone()];
                let waiting = group
                    .iter()
                    .filter(|s| s.has_unread_message || s.has_unread_highlight)
                    .count();
                let mention = group.iter().any(|s| s.has_unread_highlight);
                let text = match waiting {
                    0 => format!("[{name}]"),
                    n => format!("[{name} +{n}{}]", if mention { "!" } else { "" }),
                };
                (text, group_activity_style(group))
            };
            Cell::new(Hit::Server(g.start), vec![Span::styled(text, style)])
        })
        .collect();
    let mut rows = pack(tabs, width, MAX_TAB_LINES);

    let windows: Vec<Cell> = groups[active_group]
        .clone()
        .map(|i| {
            let s = &summaries[i];
            let is_active = active_idx == Some(i);
            let spans = if matches!(s.id, BufferId::Status) {
                let mut style = activity_style(s);
                if is_active {
                    style = style.bg(Color::Red);
                }
                vec![Span::styled(format!("{} status", i + 1), style)]
            } else {
                entry_spans(i, s, is_active)
            };
            Cell::new(Hit::Window(i), spans)
        })
        .collect();
    rows.extend(pack(windows, width, MAX_LINES));
    rows
}

/// Rows [`render_multi`] will use -- needed up front for the layout.
pub fn required_lines_multi(summaries: &[BufferSummary], width: u16, active_idx: Option<usize>) -> u16 {
    multi_layout(summaries, width, active_idx).len() as u16
}

pub fn render_multi(frame: &mut Frame, area: Rect, summaries: &[BufferSummary], active_idx: Option<usize>) {
    let lines: Vec<Line> = multi_layout(summaries, area.width, active_idx)
        .into_iter()
        .map(|row| {
            let mut spans = Vec::new();
            for (n, cell) in row.into_iter().enumerate() {
                if n > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.extend(cell.spans);
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Click mapping for [`render_multi`]'s exact layout.
pub fn hit_test_multi(
    summaries: &[BufferSummary],
    active_idx: Option<usize>,
    area: Rect,
    x: u16,
    y: u16,
) -> Option<Hit> {
    if x < area.x || x >= area.x + area.width || y < area.y || y >= area.y + area.height {
        return None;
    }
    let row = multi_layout(summaries, area.width, active_idx)
        .into_iter()
        .nth((y - area.y) as usize)?;
    let mut col = area.x;
    for (n, cell) in row.into_iter().enumerate() {
        if n > 0 {
            col += 1;
        }
        if x >= col && x < col + cell.width {
            return Some(cell.hit);
        }
        col += cell.width;
    }
    None
}

fn activity_style(s: &BufferSummary) -> Style {
    if s.has_unread_highlight {
        let color = s
            .highlight_actcolor
            .as_deref()
            .and_then(|c| c.parse::<Color>().ok())
            // no per-hilight -actcolor: /set hilight_act_color, else yellow
            .or_else(|| crate::color::from_setting(&irust_config::lookandfeel::get("hilight_act_color")))
            .unwrap_or(Color::Yellow);
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else if s.has_unread_message {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else if s.has_unread_join {
        Style::default().fg(Color::Green)
    } else if s.has_unread {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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

    fn cell_style(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Style {
        buf[(x, y)].style()
    }

    #[test]
    fn the_three_activity_tiers_render_in_the_right_colors() {
        let mut none = summary(BufferId::Status, "status");
        let mut text = summary(BufferId::Channel("#a".to_string()), "#a");
        text.has_unread = true;
        let mut msg = summary(BufferId::Channel("#b".to_string()), "#b");
        msg.has_unread = true;
        msg.has_unread_message = true;
        let mut hilight = summary(BufferId::Channel("#c".to_string()), "#c");
        hilight.has_unread = true;
        hilight.has_unread_message = true;
        hilight.has_unread_highlight = true;
        none.display_name = "status".to_string();

        let summaries = vec![none, text, msg, hilight];
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();

        let buf = terminal.backend().buffer();
        // "1status 2 #a 3 #b 4 #c" -- find where each entry starts to
        // sample its actual rendered style.
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let text_start = content.find("2 #a").unwrap() as u16;
        let msg_start = content.find("3 #b").unwrap() as u16;
        let hilight_start = content.find("4 #c").unwrap() as u16;

        assert_eq!(cell_style(buf, text_start, 0).fg, Some(Color::Cyan));
        assert_eq!(cell_style(buf, msg_start, 0).fg, Some(Color::White));
        assert!(cell_style(buf, msg_start, 0).add_modifier.contains(Modifier::BOLD));
        assert_eq!(cell_style(buf, hilight_start, 0).fg, Some(Color::Yellow));
        assert!(cell_style(buf, hilight_start, 0).add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_background_window_with_only_a_join_renders_green() {
        let mut joined = summary(BufferId::Channel("#Italy".to_string()), "#Italy");
        joined.has_unread = true;
        joined.has_unread_join = true;

        let summaries = vec![joined];
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let start = content.find("#Italy").unwrap() as u16;
        assert_eq!(cell_style(buf, start, 0).fg, Some(Color::Green));
    }

    #[test]
    fn a_part_or_topic_line_alone_stays_the_plain_text_cyan_tier_not_green() {
        // Same shape as `the_three_activity_tiers_render_in_the_right_colors`'s
        // own "text" case: `has_unread` alone (no `has_unread_join`) is what
        // a part/quit/topic/other system line sets -- must stay cyan, not
        // get pulled into the new green join tier.
        let mut left = summary(BufferId::Channel("#a".to_string()), "#a");
        left.has_unread = true;

        let summaries = vec![left];
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let start = content.find("#a").unwrap() as u16;
        assert_eq!(cell_style(buf, start, 0).fg, Some(Color::Cyan));
    }

    #[test]
    fn a_real_message_outranks_a_join_on_the_same_window() {
        let mut both = summary(BufferId::Channel("#Italy".to_string()), "#Italy");
        both.has_unread = true;
        both.has_unread_join = true;
        both.has_unread_message = true;

        let summaries = vec![both];
        let backend = TestBackend::new(40, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let start = content.find("#Italy").unwrap() as u16;
        assert_eq!(cell_style(buf, start, 0).fg, Some(Color::White));
        assert!(cell_style(buf, start, 0).add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn hilight_act_color_sets_the_default_for_a_mention_tier_window() {
        use irust_config::lookandfeel::with_test_overrides;
        let mut hilight = summary(BufferId::Channel("#c".to_string()), "#c");
        hilight.has_unread = true;
        hilight.has_unread_message = true;
        hilight.has_unread_highlight = true;
        assert_eq!(activity_style(&hilight).fg, Some(Color::Yellow), "unchanged default");
        let magenta = with_test_overrides(&[("hilight_act_color", "%M")], || activity_style(&hilight).fg);
        assert_eq!(magenta, Some(Color::LightMagenta));
        hilight.highlight_actcolor = Some("red".to_string());
        let per_hilight = with_test_overrides(&[("hilight_act_color", "%M")], || activity_style(&hilight).fg);
        assert_eq!(per_hilight, Some(Color::Red), "a /hilight -actcolor still wins");
    }

    #[test]
    fn a_hilight_actcolor_overrides_the_default_yellow() {
        let mut hilight = summary(BufferId::Channel("#c".to_string()), "#c");
        hilight.has_unread = true;
        hilight.has_unread_message = true;
        hilight.has_unread_highlight = true;
        hilight.highlight_actcolor = Some("magenta".to_string());

        let summaries = vec![hilight];
        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let start = content.find("1 #c").unwrap() as u16;
        assert_eq!(cell_style(buf, start, 0).fg, Some(Color::Magenta));
        assert!(
            cell_style(buf, start, 0).add_modifier.contains(Modifier::BOLD),
            "actcolor still bold, like the default"
        );
    }

    #[test]
    fn an_unparseable_hilight_actcolor_falls_back_to_the_default_yellow() {
        let mut hilight = summary(BufferId::Channel("#c".to_string()), "#c");
        hilight.has_unread = true;
        hilight.has_unread_message = true;
        hilight.has_unread_highlight = true;
        hilight.highlight_actcolor = Some("not-a-real-color".to_string());

        let summaries = vec![hilight];
        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let start = content.find("1 #c").unwrap() as u16;
        assert_eq!(cell_style(buf, start, 0).fg, Some(Color::Yellow));
    }

    #[test]
    fn the_active_window_gets_a_red_background() {
        let a = summary(BufferId::Channel("#a".to_string()), "#a");
        let b = summary(BufferId::Channel("#b".to_string()), "#b");
        let summaries = vec![a, b];

        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Channel("#b".to_string())))
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let active_start = content.find("2 #b").unwrap() as u16;
        let inactive_start = content.find("1 #a").unwrap() as u16;

        assert_eq!(cell_style(buf, active_start, 0).bg, Some(Color::Red));
        assert_ne!(cell_style(buf, inactive_start, 0).bg, Some(Color::Red));
    }

    /// The rank prefix here should be colored the
    /// same fixed way as the nicklist's own prefixes (green `@`, yellow
    /// `+`), not just inherit the entry's activity color.
    #[test]
    fn own_rank_prefix_is_colored_like_the_nicklist() {
        let mut opped = summary(BufferId::Channel("#a".to_string()), "#a");
        opped.own_rank = Some('@');
        let mut voiced = summary(BufferId::Channel("#b".to_string()), "#b");
        voiced.own_rank = Some('+');
        let summaries = vec![opped, voiced];

        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();

        let buf = terminal.backend().buffer();
        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        let op_pos = content.find('@').unwrap() as u16;
        let voice_pos = content.find('+').unwrap() as u16;

        assert_eq!(cell_style(buf, op_pos, 0).fg, Some(Color::LightGreen));
        assert_eq!(cell_style(buf, voice_pos, 0).fg, Some(Color::LightYellow));
    }

    #[test]
    fn own_rank_prefix_shows_directly_after_the_number() {
        let mut opped = summary(BufferId::Channel("#a".to_string()), "#a");
        opped.own_rank = Some('@');
        let summaries = vec![opped];

        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("1 @#a"), "got: {content:?}");
    }

    #[test]
    fn a_few_buffers_need_only_one_row() {
        let summaries = vec![
            summary(BufferId::Status, "status"),
            summary(BufferId::Channel("#a".to_string()), "#a"),
            summary(BufferId::Channel("#b".to_string()), "#b"),
        ];
        assert_eq!(required_lines(&summaries, 80), 1);
    }

    #[test]
    fn too_narrow_for_one_row_wraps_onto_more_rows() {
        // Each entry is "N #x", 4 cols wide -- a width of 4 has no
        // room left for a separator plus a second entry, so every
        // entry lands on its own row. 6 entries so the 6th falls past
        // MAX_LINES (5) and gets dropped.
        let summaries: Vec<_> = (0..6)
            .map(|n| summary(BufferId::Channel(format!("#{n}")), &format!("#{n}")))
            .collect();

        let lines = required_lines(&summaries, 4);
        assert_eq!(lines, 5, "expected wrapping capped at MAX_LINES, got {lines}");

        let backend = TestBackend::new(4, lines);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &summaries, &BufferId::Status))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // 6 entries, 1 per row, capped at 5 rows -- the 6th is dropped.
        assert!(content.contains("1 #0"), "got: {content:?}");
        assert!(content.contains("2 #1"), "got: {content:?}");
        assert!(content.contains("3 #2"), "got: {content:?}");
        assert!(content.contains("4 #3"), "got: {content:?}");
        assert!(content.contains("5 #4"), "got: {content:?}");
        assert!(
            !content.contains("6 #5"),
            "6th entry should be dropped past the cap: {content:?}"
        );
    }

    #[test]
    fn a_wide_enough_row_never_wraps_regardless_of_buffer_count() {
        let summaries: Vec<_> = (0..8)
            .map(|n| summary(BufferId::Channel(format!("#{n}")), &format!("#{n}")))
            .collect();
        assert_eq!(required_lines(&summaries, 200), 1);
    }

    #[test]
    fn hit_test_finds_the_entry_under_the_click() {
        // "status 2 #a 3 #b" -- clicking anywhere on "2 #a" (cols 7..10)
        // should resolve to index 1, same as clicking "3 #b" (cols
        // 11..14) resolves to index 2.
        let summaries = vec![
            summary(BufferId::Status, "status"),
            summary(BufferId::Channel("#a".to_string()), "#a"),
            summary(BufferId::Channel("#b".to_string()), "#b"),
        ];
        let area = Rect::new(0, 5, 40, 1);
        assert_eq!(hit_test(&summaries, area, 8, 5), Some(1));
        assert_eq!(hit_test(&summaries, area, 12, 5), Some(2));
        assert_eq!(hit_test(&summaries, area, 3, 5), Some(0));
    }

    #[test]
    fn hit_test_outside_the_area_is_none() {
        let summaries = vec![summary(BufferId::Status, "status")];
        let area = Rect::new(0, 5, 40, 1);
        assert_eq!(hit_test(&summaries, area, 0, 4), None, "above the area");
        assert_eq!(hit_test(&summaries, area, 0, 6), None, "below the area");
        assert_eq!(hit_test(&summaries, area, 41, 5), None, "past the width");
    }

    #[test]
    fn hit_test_lands_on_the_right_wrapped_row() {
        // Each entry is 4 cols ("N #x"), width 4 forces one per row --
        // same layout `too_narrow_for_one_row_wraps_onto_more_rows`
        // above already exercises for `render`.
        let summaries: Vec<_> = (0..3)
            .map(|n| summary(BufferId::Channel(format!("#{n}")), &format!("#{n}")))
            .collect();
        let area = Rect::new(0, 0, 4, 3);
        assert_eq!(hit_test(&summaries, area, 0, 0), Some(0));
        assert_eq!(hit_test(&summaries, area, 0, 1), Some(1));
        assert_eq!(hit_test(&summaries, area, 0, 2), Some(2));
    }

    #[test]
    fn hit_test_between_entries_on_the_separator_space_is_none() {
        let summaries = vec![
            summary(BufferId::Status, "status"),
            summary(BufferId::Channel("#a".to_string()), "#a"),
        ];
        // "status 2 #a" -- col 6 is the single separator space between
        // "status" (0..6) and "2 #a" (7..11).
        let area = Rect::new(0, 0, 40, 1);
        assert_eq!(hit_test(&summaries, area, 6, 0), None);
    }


    fn two_networks() -> Vec<BufferSummary> {
        vec![
            summary(BufferId::Status, "alpha"),
            summary(BufferId::Channel("#a".to_string()), "#a"),
            summary(BufferId::Status, "beta"),
            summary(BufferId::Channel("#b".to_string()), "#b"),
            summary(BufferId::Channel("#c".to_string()), "#c"),
        ]
    }

    fn row_text(row: &[Cell]) -> String {
        row.iter()
            .map(|c| c.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn multi_layout_is_a_tab_row_plus_only_the_active_networks_windows() {
        let list = two_networks();
        let rows = multi_layout(&list, 80, Some(3));
        assert_eq!(rows.len(), 2, "one tab row, one row of the active network's windows");
        assert_eq!(row_text(&rows[0]), "[alpha] [beta]");
        assert_eq!(row_text(&rows[1]), "3 status 4 #b 5 #c", "global numbers, alpha's #a hidden");
        assert_eq!(required_lines_multi(&list, 80, Some(3)), 2);
        assert_eq!(row_text(&multi_layout(&list, 80, Some(1))[1]), "1 status 2 #a");
    }

    #[test]
    fn multi_hit_test_maps_tabs_to_servers_and_entries_to_windows() {
        let list = two_networks();
        let area = Rect::new(0, 10, 80, 2);
        // "[alpha] [beta]": alpha tab is columns 0..7, beta tab 8..14
        assert_eq!(hit_test_multi(&list, Some(3), area, 2, 10), Some(Hit::Server(0)));
        assert_eq!(hit_test_multi(&list, Some(3), area, 9, 10), Some(Hit::Server(2)));
        // "3 status 4 #b 5 #c": "4 #b" starts at column 9
        assert_eq!(hit_test_multi(&list, Some(3), area, 10, 11), Some(Hit::Window(3)));
        assert_eq!(hit_test_multi(&list, Some(3), area, 60, 11), None);
        assert_eq!(hit_test_multi(&list, Some(3), area, 0, 9), None, "above the area");
    }

    #[test]
    fn a_busy_background_network_colors_its_tab() {
        let mut list = two_networks();
        list[1].has_unread = true;
        list[1].has_unread_message = true;
        let rows = multi_layout(&list, 80, Some(3));
        assert_eq!(rows[0][0].spans[0].style.fg, Some(Color::White), "alpha's tab shows its message activity");
        assert_eq!(row_text(&rows[0]), "[alpha +1] [beta]", "and how many windows are waiting");
        list[1].has_unread_highlight = true;
        assert_eq!(row_text(&multi_layout(&list, 80, Some(3))[0]), "[alpha +1!] [beta]", "! = a mention");
    }

    #[test]
    fn a_background_networks_tab_turns_green_on_a_join_alone() {
        let mut list = two_networks();
        list[1].has_unread = true;
        list[1].has_unread_join = true;
        let rows = multi_layout(&list, 80, Some(3));
        assert_eq!(rows[0][0].spans[0].style.fg, Some(Color::Green), "alpha's tab shows the join tier");
    }
}

