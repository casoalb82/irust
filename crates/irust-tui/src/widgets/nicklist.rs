use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::color::color_for_index;

/// One nicklist line, pre-resolved by the caller (`app.rs`, which holds
/// the `ClientState` lock) -- keeps this widget free of any borrow on
/// `NickColors`/`Nicklist` themselves.
pub struct NicklistRow {
    pub nick: String,
    pub prefix: Option<char>,
    pub color_index: usize,
    pub away: bool,
}

const AWAY_COLOR: Color = Color::Indexed(244);

/// Rank-prefix color, classic IRC-client convention:
/// bright green for `@` (op), bright yellow for `+` (voice). The other
/// prefixes aren't in active use on ChatNet today but get a sensible
/// color too rather than falling back to unstyled white.
pub(crate) fn prefix_color(prefix: char) -> Color {
    match prefix {
        '~' => Color::LightRed,
        '&' => Color::LightMagenta,
        '@' => Color::LightGreen,
        '%' => Color::LightCyan,
        '+' => Color::LightYellow,
        _ => Color::White,
    }
}

fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let kept: String = s.chars().take(max_chars - 1).collect();
    format!("{kept}…")
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    rows: &[NicklistRow],
    colors_enabled: bool,
    channel_name: &str,
    scroll: usize,
) -> usize {
    let inner_height = area.height.saturating_sub(2) as usize;
    let total = rows.len();
    let max_scroll = total.saturating_sub(inner_height);
    let scroll = scroll.min(max_scroll);
    let visible_rows = if inner_height == 0 {
        &[][..]
    } else {
        &rows[scroll..(scroll + inner_height).min(total)]
    };

    let lines: Vec<Line> = visible_rows
        .iter()
        .map(|row| {
            let nick_color = if row.away {
                AWAY_COLOR
            } else if colors_enabled {
                color_for_index(row.color_index)
            } else {
                Color::White
            };
            let nick_style = Style::default().fg(nick_color);
            match row.prefix {
                Some(c) => Line::from(vec![
                    Span::styled(c.to_string(), Style::default().fg(prefix_color(c))),
                    Span::styled(row.nick.clone(), nick_style),
                ]),
                None => Line::styled(row.nick.clone(), nick_style),
            }
        })
        .collect();

    let prefix = if total > inner_height {
        format!(" {}-{}/{total} ", scroll + 1, scroll + visible_rows.len())
    } else {
        format!(" {total} ")
    };
    let border_cols = 2; // the two vertical border chars either side
    let available = (area.width as usize).saturating_sub(border_cols);
    let max_name_len = available.saturating_sub(prefix.chars().count() + 1); // +1 for the trailing space
    let title = format!("{prefix}{} ", truncate_with_ellipsis(channel_name, max_name_len));
    let paragraph = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(paragraph, area);

    scroll
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn row(nick: &str) -> NicklistRow {
        NicklistRow {
            nick: nick.to_string(),
            prefix: None,
            color_index: 0,
            away: false,
        }
    }

    #[test]
    fn a_channel_with_more_nicks_than_fit_gets_clipped_with_a_range_indicator() {
        let rows: Vec<NicklistRow> = (0..10).map(|i| row(&format!("nick{i}"))).collect();

        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &rows, true, "#chatnet", 0);
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            content.contains("1-3/10"),
            "expected a range indicator in the title:\n{content}"
        );
        assert!(content.contains("nick0") && content.contains("nick2"));
        assert!(
            !content.contains("nick9"),
            "row past the visible window should not be drawn:\n{content}"
        );
    }

    /// The common case (everyone fits) must keep the exact old title --
    /// no range indicator clutter when there's nothing to scroll to.
    #[test]
    fn a_channel_that_fits_entirely_shows_the_plain_count_title() {
        let rows: Vec<NicklistRow> = (0..3).map(|i| row(&format!("nick{i}"))).collect();

        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &rows, true, "#chatnet", 0);
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            content.contains(" 3 #chatnet "),
            "expected the plain count title, got:\n{content}"
        );
        assert!(
            !content.contains('/'),
            "no range indicator should appear when everything fits:\n{content}"
        );
    }

    #[test]
    fn a_long_channel_name_in_a_narrow_sidebar_truncates_with_an_ellipsis() {
        let rows: Vec<NicklistRow> = (0..6).map(|i| row(&format!("nick{i}"))).collect();

        let backend = TestBackend::new(14, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &rows, true, "#operations-desk", 0);
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            content.contains(" 6 "),
            "the count prefix must survive intact:\n{content}"
        );
        assert!(
            content.contains('…'),
            "a cut name should end in an ellipsis:\n{content}"
        );
        assert!(
            !content.contains("#operations-desk"),
            "the full name shouldn't fit in a 14-column pane:\n{content}"
        );
        assert!(
            !content.contains("#operations "),
            "a bare mid-word clip with no ellipsis at all:\n{content}"
        );
    }

    /// Scrolling past the end must clamp to the last full page, not walk
    /// off into an empty view -- e.g. `ctrl-d` held down after already
    /// reaching the bottom.
    #[test]
    fn scrolling_past_the_end_clamps_to_the_last_page() {
        let rows: Vec<NicklistRow> = (0..10).map(|i| row(&format!("nick{i}"))).collect();

        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let clamped = std::cell::Cell::new(0usize);
        terminal
            .draw(|frame| clamped.set(render(frame, frame.area(), &rows, true, "#chatnet", 999)))
            .unwrap();

        // Inner height is 3 (5 - 2 border rows), 10 rows total -> the
        // last valid top-of-window offset is 7 (showing rows 7,8,9).
        assert_eq!(clamped.get(), 7);
    }
}
