use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use irust_core::buffer::{Buffer, BufferLine, SystemKind};
use irust_core::mirc::{parse_mirc, MircStyle, Segment};

use crate::color::color_for_index;
use crate::widgets::nicklist::prefix_color;

pub fn render(
    frame: &mut Frame,
    area: Rect,
    buffer: &Buffer,
    scroll_offset: usize,
    text_colors: bool,
    show_name: bool,
) {
    let visible_height = area.height.saturating_sub(2) as usize;
    // Borders eat 2 columns; `Paragraph`'s own word-wrap needs this to
    // know how many terminal ROWS a long logical line actually takes.
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let total = buffer.lines.len();
    let bottom = total.saturating_sub(scroll_offset);
    let floor = if scroll_offset == 0 { buffer.cleared_before() } else { 0 };

    let mut rendered: Vec<Line> = Vec::new();
    let mut idx = bottom;
    let mut rows_used = 0usize;
    while idx > floor && rows_used < visible_height {
        idx -= 1;
        let raw = &buffer.lines[idx];
        if matches!(
            raw,
            BufferLine::System {
                kind: SystemKind::Join | SystemKind::Leave,
                ..
            }
        ) && !irust_config::settings::SHOW_JOINPART.load(std::sync::atomic::Ordering::Relaxed)
        {
            continue;
        }
        let mut line = render_line(raw, text_colors);
        if !timestamp_shown(buffer, idx) && line.spans.len() > 1 {
            line.spans.remove(0); // every variant leads with the timestamp span
        }
        rows_used += exact_row_count(&line, inner_width);
        rendered.push(line);
    }
    rendered.reverse();
    let assembled_rows = Paragraph::new(rendered.clone())
        .wrap(Wrap { trim: false })
        .line_count(inner_width as u16);
    let overflow = assembled_rows.saturating_sub(visible_height) as u16;

    irust_config::debug_log::line(
        "RENDER",
        &buffer.name,
        &format!(
            "total={total} scroll_offset={scroll_offset} bottom={bottom} floor={floor} \
             visible_height={visible_height} inner_width={inner_width} rows_used={rows_used} \
             overflow={overflow} lines_walked={} idx_stopped_at={idx}",
            rendered.len()
        ),
    );

    let name_part = if show_name {
        format!("{} ", buffer.shown_name())
    } else {
        String::new()
    };
    let title = if scroll_offset > 0 {
        format!(" {name_part}(scrolled, {scroll_offset} lines back - End to jump to live) ")
    } else if name_part.is_empty() {
        String::new()
    } else {
        format!(" {name_part}")
    };
    let paragraph = Paragraph::new(rendered)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((overflow, 0));

    frame.render_widget(paragraph, area);
}

fn exact_row_count(line: &Line, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    Paragraph::new(line.clone())
        .wrap(Wrap { trim: false })
        .line_count(width as u16)
}

/// irssi's `get_timestamp` gates: a line gets a timestamp only if its level
/// is in `timestamp_level`, and (with `timestamp_timeout` > 0) only if at
/// least that many seconds passed since the previous line that qualified.
fn timestamp_shown(buffer: &Buffer, idx: usize) -> bool {
    use irust_config::lookandfeel::{get_level, get_time_ms};
    let mask = get_level("timestamp_level");
    let level_of = |i: usize| irust_core::activity::line_level(&buffer.lines[i], &buffer.name);
    if mask & level_of(idx) == 0 {
        return false;
    }
    let timeout_secs = get_time_ms("timestamp_timeout") / 1000;
    if timeout_secs <= 0 {
        return true;
    }
    let Some(now) = buffer.lines[idx].time() else { return true };
    let previous = (0..idx).rev().find(|&i| mask & level_of(i) != 0).and_then(|i| buffer.lines[i].time());
    previous.is_none_or(|prev| now - prev >= timeout_secs)
}

fn timestamp_span(timestamp: &str) -> Span<'static> {
    // `/set show_timestamps off` -- checked here, not threaded as a
    // parameter through every `render_line` call site, same reasoning
    // as `irust_config::settings`'s own doc comment on why these are
    // global atomics rather than plumbed-through state.
    if !irust_config::settings::SHOW_TIMESTAMPS.load(std::sync::atomic::Ordering::Relaxed) {
        return Span::raw("");
    }
    // Bracketed, matching the on-disk log format (`[HH:MM:SS] ...`,
    // see `irust_config`'s logger) -- the live view used to show it
    // bare.
    Span::styled(
        format!("[{timestamp}] "),
        Style::default().fg(crate::theme::current().timestamp),
    )
}

fn render_line(line: &BufferLine, text_colors: bool) -> Line<'static> {
    match line {
        BufferLine::System {
            text, timestamp, kind, ..
        } => {
            let theme = crate::theme::current();
            let color = match kind {
                SystemKind::Join => theme.join,
                SystemKind::Leave => theme.leave,
                SystemKind::Topic => Color::LightGreen,
                // Plain (terminal default), not the muted `Other` gray --
                // matches irssi's own whois look, which colors only the
                // embedded mIRC-coded bits (bold nick, gray brackets)
                // rather than washing the whole line.
                SystemKind::Whois => Color::Reset,
                SystemKind::Other => Color::Reset,
                SystemKind::Notice => theme.notice,
                SystemKind::Server => theme.join,
                SystemKind::Warning => theme.leave,
                // irssi doesn't color this specially either (its own
                // `daychange` theme format carries no color codes) --
                // plain, same as `Other`.
                SystemKind::DayChange => Color::Reset,
                SystemKind::Wallops => Color::Yellow,
            };
            // WeeChat-style arrows for join/part --
            // display only: the underlying text (what actually gets
            // logged to disk and reloaded via `parse_log_line`, which
            // keys off a literal "-- " prefix) is untouched, only what
            // gets drawn here changes.
            let arrow_text;
            let text: &str = match kind {
                SystemKind::Join => match text.strip_prefix("-- ") {
                    Some(rest) => {
                        arrow_text = format!("--> {rest}");
                        &arrow_text
                    }
                    None => text,
                },
                SystemKind::Leave => match text.strip_prefix("-- ") {
                    Some(rest) => {
                        arrow_text = format!("<-- {rest}");
                        &arrow_text
                    }
                    None => text,
                },
                _ => text,
            };
            let mut spans = vec![timestamp_span(timestamp)];
            if *kind == SystemKind::Server {
                if let Some((hostname, rest)) = text.strip_prefix('!').and_then(|r| r.split_once(' ')) {
                    spans.push(Span::styled(
                        format!("!{hostname} "),
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    ));
                    spans.extend(mirc_spans(rest, Style::default(), true));
                    return Line::from(spans);
                }
            }
            spans.extend(mirc_spans(text, Style::default().fg(color), true));
            Line::from(spans)
        }
        BufferLine::Chat {
            nick,
            color_index,
            rank,
            text,
            highlight,
            highlight_color,
            timestamp,
            ..
        } => {
            let custom_color = highlight_color.as_deref().and_then(|s| s.parse::<Color>().ok());
            let nick_style = line_style(color_for_index(*color_index), *highlight, custom_color);
            let text_base = base_style(color_for_index(*color_index), *highlight, text_colors);
            let mut spans = vec![timestamp_span(timestamp), Span::raw("<")];
            if irust_config::lookandfeel::get_bool("show_nickmode") {
                match rank {
                    Some(r) => spans.push(Span::styled(r.to_string(), Style::default().fg(prefix_color(*r)))),
                    None if irust_config::lookandfeel::get_bool("show_nickmode_empty") => spans.push(Span::raw(" ")),
                    None => {}
                }
            }
            spans.push(Span::styled(nick.clone(), nick_style));
            spans.push(Span::styled("> ", text_base));
            spans.extend(mirc_spans(text, text_base, text_colors || *highlight));
            Line::from(spans)
        }
        BufferLine::Action {
            nick,
            color_index,
            rank,
            text,
            highlight,
            highlight_color,
            timestamp,
            ..
        } => {
            let custom_color = highlight_color.as_deref().and_then(|s| s.parse::<Color>().ok());
            let nick_style = line_style(color_for_index(*color_index), *highlight, custom_color);
            let text_base = base_style(color_for_index(*color_index), *highlight, text_colors);
            let mut spans = vec![timestamp_span(timestamp), Span::styled("* ", text_base)];
            if let Some(r) = rank {
                spans.push(Span::styled(r.to_string(), Style::default().fg(prefix_color(*r))));
            }
            spans.push(Span::styled(nick.clone(), nick_style));
            spans.push(Span::styled(" ", text_base));
            spans.extend(mirc_spans(text, text_base, text_colors));
            Line::from(spans)
        }
    }
}

fn base_style(nick_color: Color, highlight: bool, text_colors: bool) -> Style {
    if !text_colors {
        Style::default()
    } else if highlight {
        Style::default().fg(nick_color).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

const HIGHLIGHT_NICK_COLOR: Color = Color::Indexed(208); // orange

/// `/set hilight_color`, else the built-in orange.
fn highlight_nick_color() -> Color {
    crate::color::from_setting(&irust_config::lookandfeel::get("hilight_color")).unwrap_or(HIGHLIGHT_NICK_COLOR)
}

fn line_style(nick_color: Color, highlight: bool, custom_color: Option<Color>) -> Style {
    if highlight {
        Style::default()
            .fg(custom_color.unwrap_or_else(highlight_nick_color))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(nick_color)
    }
}

/// mIRC's extended colors 16-98 as xterm-256 indexes (the standard
/// mapping, same table modern clients use) -- crash0ver's `.big` themes
/// and `.img` pictures are drawn with these.
const MIRC_EXTENDED_256: [u8; 83] = [
    52, 94, 100, 58, 22, 29, 23, 24, 17, 54, 53, 89, //
    88, 130, 142, 64, 28, 35, 30, 25, 18, 91, 90, 125, //
    124, 166, 184, 106, 34, 49, 37, 33, 19, 129, 127, 161, //
    196, 208, 226, 154, 46, 86, 51, 75, 21, 171, 201, 198, //
    203, 215, 227, 191, 83, 122, 87, 111, 63, 177, 207, 205, //
    217, 223, 229, 193, 157, 158, 159, 153, 147, 183, 219, 212, //
    16, 233, 235, 237, 239, 241, 244, 247, 250, 254, 231,
];

/// mIRC palette: the standard 16 codes (0-15 -- what K9/ChatWorld/OS
/// send) mapped to the nearest terminal color, 16-98 through
/// `MIRC_EXTENDED_256`. 99 ("default") and anything past it falls back
/// to no color change.
fn mirc_color(code: u8) -> Option<Color> {
    if (16..=98).contains(&code) {
        return Some(Color::Indexed(MIRC_EXTENDED_256[usize::from(code - 16)]));
    }
    Some(match code {
        0 => Color::White,
        1 => Color::Black,
        2 => Color::Blue,
        3 => Color::Green,
        4 => Color::Red,
        5 => Color::Indexed(88), // brown/maroon
        6 => Color::Magenta,
        7 => Color::Indexed(208), // orange
        // LightYellow, not Yellow -- matches the voice-prefix `+` color
        // in nicklist.rs; plain Yellow read too dull here.
        8 => Color::LightYellow,
        9 => Color::LightGreen,
        10 => Color::Cyan,
        11 => Color::LightCyan,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::Indexed(240),
        15 => Color::Gray,
        _ => return None,
    })
}

fn mirc_spans(text: &str, base: Style, text_colors: bool) -> Vec<Span<'static>> {
    // irssi's formats.c: hide_text_style drops bold/underline/reverse/italic
    // and implies hide_colors; the codes themselves are always stripped.
    let hide_style = irust_config::lookandfeel::get_bool("hide_text_style");
    let show_colors = text_colors && !hide_style && !irust_config::lookandfeel::get_bool("hide_colors");
    parse_mirc(text)
        .into_iter()
        .map(|seg| {
            let mut style = base;
            if show_colors {
                if let Some(fg) = seg.style.fg.and_then(mirc_color) {
                    style = style.fg(fg);
                }
                if let Some(bg) = seg.style.bg.and_then(mirc_color) {
                    style = style.bg(bg);
                }
            }
            let seg = if hide_style {
                Segment { style: MircStyle { fg: seg.style.fg, bg: seg.style.bg, ..MircStyle::default() }, ..seg }
            } else {
                seg
            };
            if seg.style.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if seg.style.underline {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if seg.style.reverse {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if seg.style.italic {
                style = style.add_modifier(Modifier::ITALIC);
            }
            Span::styled(seg.text, style)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// The extended 16-98 range maps through the xterm-256 table (both
    /// ends checked), 99 and up stay "no color change".
    #[test]
    fn mirc_extended_colors_map_to_xterm_256() {
        assert_eq!(mirc_color(16), Some(Color::Indexed(52)));
        assert_eq!(mirc_color(52), Some(Color::Indexed(196)));
        assert_eq!(mirc_color(98), Some(Color::Indexed(231)));
        assert_eq!(mirc_color(99), None);
        assert_eq!(mirc_color(4), Some(Color::Red));
    }

    #[test]
    fn a_long_word_heavy_reply_does_not_get_clipped_by_a_bad_row_estimate() {
        let mut buffer = Buffer::new("#chatnet");
        for i in 0..5 {
            buffer.push_chat(format!("nick{i}"), 0, None, format!("msg {i}"), false);
        }
        buffer.push_chat(
            "Scout",
            0,
            None,
            "203.0.113.191 Springfield, Exampleland ExampleNet Cable Internet AS64500 \
             ExampleNet Communications Ltd. 12.3456,65.4321 Europe/Example \
             VPN/proxy: no Hosting/datacenter: no",
            false,
        );

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &buffer, 0, true, true))
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            content.contains("datacenter"),
            "newest message was clipped off screen:\n{content}"
        );
    }

    #[test]
    fn exact_row_count_matches_real_word_wrap_not_a_naive_width_division() {
        let line = Line::from("aaaaaaaaaa bbbbbbbbbb");
        assert_eq!(exact_row_count(&line, 10), 2);

        // Realistic under-count case: several ~7-9 char words at width
        // 12 -- naive division undercounts because leftover space per
        // row can't be reused once a word doesn't fit in it.
        let line2 = Line::from("Springfield Exampleland ExampleNet Cable");
        let naive = line2.width().div_ceil(12);
        let real = exact_row_count(&line2, 12);
        assert!(
            real >= naive,
            "word-wrap should never need FEWER rows than the naive estimate"
        );
    }

    #[test]
    fn long_runs_of_spaces_do_not_push_the_newest_message_off_screen() {
        let mut buffer = Buffer::new("#ops");
        buffer.push_chat(
            "ChatWorld",
            0,
            None,
            "NOTICE! Not all commands is compatible with bahamut                      \
             ││[14:51:03] <ChatWorld> Level: USER                                     \
             ││[14:51:03] <ChatWorld> CWINFO  OPERS  HELP  TOGGLEMSG  VERSION COMMANDS",
            false,
        );
        buffer.push_chat(
            "ChatWorld",
            0,
            None,
            "│[14:51:15] <ChatWorld> ╔══════════════════════╗                          \
             ││[14:51:15] <ChatWorld> ║  ChatWorld IRC Services  ║                     \
             ││[14:51:15] <ChatWorld> ╚══════════════════════╝",
            false,
        );
        for i in 0..3 {
            buffer.push_chat(format!("nick{i}"), 0, None, format!("msg {i}"), false);
        }
        buffer.push_chat("abcX", 0, None, "lastmsg", false);

        let backend = TestBackend::new(80, 21);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &buffer, 0, true, true))
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            content.contains("lastmsg"),
            "newest message was clipped off screen:\n{content}"
        );
    }

    #[test]
    fn a_short_whois_worth_of_lines_all_fit_in_a_realistic_pane_height() {
        let mut buffer = Buffer::new("#chatnet");
        buffer.push_system_whois("-- the_nick [~user@some.host.example]".to_string());
        buffer.push_system_whois("-- ircname  : the_nick".to_string());
        buffer.push_system_whois("-- channels : #a #b #c".to_string());
        buffer.push_system_whois("-- server   : irc.example.com [a server]".to_string());
        buffer.push_system_whois("--          : is using a secure connection".to_string());
        buffer.push_system_whois("--          : is connecting from 1.2.3.4".to_string());
        buffer.push_system_whois("-- idle     : 0 days 1 hours 2 mins 3 secs".to_string());
        buffer.push_system_whois("-- End of WHOIS".to_string());

        let backend = TestBackend::new(75, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &buffer, 0, true, true))
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            content.contains("the_nick"),
            "oldest whois line was clipped:\n{content}"
        );
        assert!(
            content.contains("End of WHOIS"),
            "newest line should obviously still show:\n{content}"
        );
    }

    #[test]
    fn a_long_wrapped_topic_does_not_push_the_newest_message_off_screen() {
        let mut buffer = Buffer::new("#chatnet");
        buffer.push_system_topic(format!(
            "-- topic: {}",
            "Network upgrade complete. | Welcome to ExampleNet, online since 1999, rebuilt 2026 | \
             https://example.net | Welcome to our new server Paris.FR.EU.* Admin \
             Erin. | Welcome Rome.* new admin frank | Happy B-Day dana!!!"
        ));
        // A handful of ordinary short lines after the topic -- not
        // enough on their own to push the topic out of a naive
        // "last N logical lines" window.
        for i in 0..3 {
            buffer.push_chat(format!("nick{i}"), 0, None, format!("msg {i}"), false);
        }
        buffer.push_chat("sh", 0, None, "prova1", false);

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &buffer, 0, true, true);
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
            content.contains("prova1"),
            "newest message was clipped off screen:\n{content}"
        );
    }

    /// Same class of bug, but proving it's not topic-specific: any long
    /// wrapped chat line (a pasted URL, a long sentence) sitting in the
    /// window must not push a later, short message off screen either.
    #[test]
    fn a_long_wrapped_chat_message_does_not_push_later_messages_off_screen() {
        let mut buffer = Buffer::new("#chatnet");
        buffer.push_chat(
            "dana",
            0,
            None,
            "questa e' una frase molto lunga che quando va a capo occupera' sicuramente \
             pi\u{f9} di una riga sullo schermo del terminale, proprio come faceva il topic",
            false,
        );
        for i in 0..3 {
            buffer.push_chat(format!("nick{i}"), 0, None, format!("msg {i}"), false);
        }
        buffer.push_chat("sh", 0, None, "ultimo", false);

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &buffer, 0, true, true))
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            content.contains("ultimo"),
            "newest message was clipped off screen:\n{content}"
        );
    }

    #[test]
    fn scrolled_back_past_a_long_wrapped_line_does_not_panic_or_blank_the_view() {
        let mut buffer = Buffer::new("#chatnet");
        buffer.push_system_topic(format!(
            "-- topic: {}",
            "a very long topic line that will definitely wrap across several rows once \
             it hits this narrow test terminal width, same as abcX's real one did"
        ));
        for i in 0..10 {
            buffer.push_chat(format!("nick{i}"), 0, None, format!("msg {i}"), false);
        }

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        // Scrolled back far enough that the long topic line is inside
        // the visible window -- this checks the view stays non-blank and
        // doesn't panic rather than requiring the topic specifically at
        // this exact offset.
        terminal
            .draw(|frame| render(frame, frame.area(), &buffer, 8, true, true))
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            content
                .chars()
                .any(|c| !c.is_whitespace() && c != '│' && c != '─' && c != '┌' && c != '┐'),
            "scrolled-back view should not be blank"
        );
    }

    /// A pathologically narrow pane (e.g. right after a terminal resize
    /// event, before layout settles) must not panic -- `inner_width`
    /// could legitimately hit 0 or 1.
    #[test]
    fn a_very_narrow_area_does_not_panic() {
        let mut buffer = Buffer::new("#chatnet");
        buffer.push_chat("sh", 0, None, "hi", false);

        let backend = TestBackend::new(3, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &buffer, 0, true, true))
            .unwrap();
    }

    #[test]
    fn a_chat_lines_rank_prefix_is_colored_like_the_nicklist() {
        let mut opped = Buffer::new("#chatnet");
        opped.push_chat("abcX", 0, Some('@'), "hey", false);
        let opped_line = render_line(&opped.lines[0], true);
        let at_span = opped_line.spans.iter().find(|s| s.content.as_ref() == "@").unwrap();
        assert_eq!(at_span.style.fg, Some(Color::LightGreen));

        let mut plain = Buffer::new("#chatnet");
        plain.push_chat("abcX", 0, None, "hey", false);
        let plain_line = render_line(&plain.lines[0], true);
        assert!(
            plain_line.spans.iter().all(|s| s.content.as_ref() != "@"),
            "no rank means no prefix span at all"
        );
    }

    #[test]
    fn show_nickmode_settings_control_the_chat_prefix() {
        use irust_config::lookandfeel::with_test_overrides;
        let text_of = |settings: &[(&str, &str)], rank: Option<char>| -> String {
            with_test_overrides(settings, || {
                let mut buf = Buffer::new("#chatnet");
                buf.push_chat("abcX", 0, rank, "hey", false);
                render_line(&buf.lines[0], true).spans.iter().map(|s| s.content.as_ref()).collect::<String>()
            })
        };
        assert!(text_of(&[], Some('@')).contains("<@abcX>"));
        assert!(text_of(&[], None).contains("<abcX>"), "irust default: no padding for a plain member");
        assert!(text_of(&[("show_nickmode_empty", "on")], None).contains("< abcX>"));
        assert!(text_of(&[("show_nickmode_empty", "on")], Some('+')).contains("<+abcX>"));
        assert!(text_of(&[("show_nickmode", "off")], Some('@')).contains("<abcX>"), "no prefix when off");
        let both = [("show_nickmode", "off"), ("show_nickmode_empty", "on")];
        assert!(text_of(&both, None).contains("<abcX>"), "show_nickmode off wins over _empty");
    }

    #[test]
    fn hide_colors_and_hide_text_style_strip_the_mirc_formatting() {
        use irust_config::lookandfeel::with_test_overrides;
        // bold "b", red "r", plain "p"
        let text = "\u{2}b\u{f} \u{3}4r\u{f} p";
        let spans = |settings: &[(&str, &str)]| {
            with_test_overrides(settings, || mirc_spans(text, Style::default(), true))
        };
        let shown = |sp: &[Span]| sp.iter().map(|s| s.content.as_ref()).collect::<String>();
        let normal = spans(&[]);
        assert_eq!(shown(&normal), "b r p", "control codes are never printed");
        assert!(normal.iter().any(|s| s.style.add_modifier.contains(Modifier::BOLD)));
        assert!(normal.iter().any(|s| s.style.fg.is_some()));

        let no_colors = spans(&[("hide_colors", "on")]);
        assert!(no_colors.iter().all(|s| s.style.fg.is_none()), "colors gone");
        assert!(no_colors.iter().any(|s| s.style.add_modifier.contains(Modifier::BOLD)), "bold stays");

        let no_style = spans(&[("hide_text_style", "on")]);
        assert_eq!(shown(&no_style), "b r p");
        assert!(no_style.iter().all(|s| s.style.fg.is_none() && s.style.add_modifier.is_empty()), "style implies no colors");
    }

    #[test]
    fn timestamp_level_and_timeout_decide_which_lines_keep_their_timestamp() {
        use irust_config::lookandfeel::with_test_overrides;
        // three chat lines 0s, 30s and 90s apart, then a join
        let mut buf = Buffer::new("#chatnet");
        for (offset, text) in [(0, "a"), (30, "b"), (90, "c")] {
            buf.push_chat_at("n", 0, None, text, false, Some("12:00:00".to_string()), Some(1_000 + offset));
        }
        buf.push_system_join("-- x joined");
        let stamps = |settings: &[(&str, &str)]| -> Vec<bool> {
            with_test_overrides(settings, || {
                (0..buf.lines.len()).map(|i| timestamp_shown(&buf, i)).collect()
            })
        };
        assert_eq!(stamps(&[]), vec![true, true, true, true], "default: every line");
        assert_eq!(
            stamps(&[("timestamp_timeout", "60")]),
            vec![true, false, true, true],
            "a stamp only after a 60s gap: b follows a by 30s, the join (stamped with the real clock) is far later"
        );
        assert_eq!(
            stamps(&[("timestamp_level", "ALL -JOINS")]),
            vec![true, true, true, false],
            "joins carry no timestamp when JOINS is out of the level list"
        );
    }

    #[test]
    fn hilight_color_overrides_the_highlighted_nick_color() {
        use irust_config::lookandfeel::with_test_overrides;
        let nick_fg = |settings: &[(&str, &str)]| {
            with_test_overrides(settings, || {
                let mut buf = Buffer::new("#chatnet");
                buf.push_chat("Other", 0, None, "hey abcX", true);
                let line = render_line(&buf.lines[0], true);
                line.spans.iter().find(|s| s.content.as_ref() == "Other").unwrap().style.fg
            })
        };
        assert_eq!(nick_fg(&[]), Some(HIGHLIGHT_NICK_COLOR), "built-in orange by default");
        assert_eq!(nick_fg(&[("hilight_color", "%Y")]), Some(Color::LightYellow));
    }

    /// `/textcolors off` (the default) must
    /// leave the message BODY plain even on a highlighted (mention)
    /// line, while the sender's `<nick>` label still gets its own hash
    /// color and still bolds -- only the label should visually flag
    /// "this mentions me", not the whole line.
    #[test]
    fn text_colors_off_keeps_the_nick_label_colored_but_flattens_the_body() {
        let mut buffer = Buffer::new("#chatnet");
        buffer.push_chat("dana", 3, None, "hey abcX check this out", true);
        let rendered = render_line(&buffer.lines[0], false);

        let BufferLine::Chat { .. } = &buffer.lines[0] else {
            unreachable!()
        };
        let nick_span = rendered.spans.iter().find(|s| s.content.as_ref() == "dana").unwrap();
        assert_eq!(
            nick_span.style.fg,
            Some(HIGHLIGHT_NICK_COLOR),
            "a highlighted line's nick label should use the fixed contrast color, not its own hash color"
        );
        assert!(
            nick_span.style.add_modifier.contains(Modifier::BOLD),
            "nick label should still bold"
        );

        let body_span = rendered
            .spans
            .iter()
            .find(|s| s.content.contains("check this out"))
            .unwrap();
        assert_eq!(
            body_span.style.fg, None,
            "message body must stay plain with text_colors off"
        );
        assert!(
            !body_span.style.add_modifier.contains(Modifier::BOLD),
            "message body must not bold either"
        );
    }

    #[test]
    fn a_highlighted_services_notice_shows_its_own_mirc_colors_even_with_text_colors_off() {
        let mut buffer = Buffer::new("K9");
        buffer.push_chat(
            "K9",
            0,
            None,
            "\u{3}8,1Initial HELP Menu                 Minimum Access:  0",
            true,
        );
        let rendered = render_line(&buffer.lines[0], false);

        let body_span = rendered
            .spans
            .iter()
            .find(|s| s.content.contains("Initial HELP Menu"))
            .unwrap();
        assert_eq!(
            body_span.style.fg,
            mirc_color(8),
            "a service notice's own embedded mIRC color must show even with text_colors off"
        );
    }

    #[test]
    fn join_and_leave_render_as_weechat_style_arrows() {
        let mut buffer = Buffer::new("#chatnet");
        buffer.push_system_join("-- boogieman joined #chatnet".to_string());
        buffer.push_system_leave("-- boogieman quit (Client Quit)".to_string());

        let join_rendered = render_line(&buffer.lines[0], true);
        let join_text: String = join_rendered.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            join_text.contains("--> boogieman joined #chatnet"),
            "got: {join_text:?}"
        );

        let leave_rendered = render_line(&buffer.lines[1], true);
        let leave_text: String = leave_rendered.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            leave_text.contains("<-- boogieman quit (Client Quit)"),
            "got: {leave_text:?}"
        );

        // The underlying stored text must be untouched -- log/reload
        // round-tripping depends on the literal "-- " prefix.
        let BufferLine::System { text, .. } = &buffer.lines[0] else {
            unreachable!()
        };
        assert_eq!(text, "-- boogieman joined #chatnet");
    }

    #[test]
    fn no_name_and_not_scrolled_draws_an_unbroken_top_border() {
        let mut buffer = Buffer::new("#chatnet");
        buffer.push_chat("sh", 0, None, "hi", false);

        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &buffer, 0, true, false))
            .unwrap();

        let buf = terminal.backend().buffer();
        let top_row: String = (0..buf.area.width).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(
            !top_row.contains(' '),
            "top border should have no gap, got: {top_row:?}"
        );
    }
}
