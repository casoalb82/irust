
const BOLD: char = '\u{02}';
const COLOR: char = '\u{03}';
const ITALIC: char = '\u{1D}';
const UNDERLINE: char = '\u{1F}';
const REVERSE: char = '\u{16}';
const RESET: char = '\u{0F}';

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MircStyle {
    pub fg: Option<u8>,
    pub bg: Option<u8>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub style: MircStyle,
}

/// Splits `input` into styled runs. A segment with `style ==
/// MircStyle::default()` and no control codes anywhere is the common
/// case (plain text) -- callers can special-case a single all-default
/// segment to skip building extra spans.
pub fn parse_mirc(input: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut style = MircStyle::default();
    let mut current = String::new();
    let mut chars = input.chars().peekable();

    macro_rules! flush {
        () => {
            if !current.is_empty() {
                segments.push(Segment {
                    text: std::mem::take(&mut current),
                    style,
                });
            }
        };
    }

    while let Some(c) = chars.next() {
        match c {
            BOLD => {
                flush!();
                style.bold = !style.bold;
            }
            ITALIC => {
                flush!();
                style.italic = !style.italic;
            }
            UNDERLINE => {
                flush!();
                style.underline = !style.underline;
            }
            REVERSE => {
                flush!();
                style.reverse = !style.reverse;
            }
            RESET => {
                flush!();
                style = MircStyle::default();
            }
            COLOR => {
                flush!();
                let fg = take_color_digits(&mut chars);
                let bg = if fg.is_some() && chars.peek() == Some(&',') {
                    let mut lookahead = chars.clone();
                    lookahead.next(); // the comma
                    let bg = take_color_digits(&mut lookahead);
                    if bg.is_some() {
                        chars = lookahead;
                    }
                    bg
                } else {
                    None
                };
                match fg {
                    Some(fg) => {
                        style.fg = Some(fg);
                        if let Some(bg) = bg {
                            style.bg = Some(bg);
                        }
                    }
                    // Bare \x03 with no digits resets color only, per
                    // the mIRC spec -- other attributes are untouched.
                    None => {
                        style.fg = None;
                        style.bg = None;
                    }
                }
            }
            other => current.push(other),
        }
    }
    flush!();
    segments
}

/// Reads up to 2 ASCII digits, mIRC's own color-code width, and parses
/// them as a `0`-`98` palette index. Doesn't consume anything (via a
/// cloned peekable iterator at the call site) unless it finds at least
/// one digit.
fn take_color_digits(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<u8> {
    let mut digits = String::new();
    while digits.len() < 2 {
        match chars.peek() {
            Some(c) if c.is_ascii_digit() => {
                digits.push(*c);
                chars.next();
            }
            _ => break,
        }
    }
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_default_segment() {
        let segs = parse_mirc("hello there");
        assert_eq!(
            segs,
            vec![Segment {
                text: "hello there".to_string(),
                style: MircStyle::default()
            }]
        );
    }

    #[test]
    fn color_code_with_fg_and_bg() {
        let segs = parse_mirc("\u{3}8,1Initial HELP Menu");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "Initial HELP Menu");
        assert_eq!(segs[0].style.fg, Some(8));
        assert_eq!(segs[0].style.bg, Some(1));
    }

    #[test]
    fn color_code_with_fg_only() {
        let segs = parse_mirc("\u{3}4danger");
        assert_eq!(segs[0].style.fg, Some(4));
        assert_eq!(segs[0].style.bg, None);
        assert_eq!(segs[0].text, "danger");
    }

    #[test]
    fn bare_color_code_resets_to_no_color() {
        let segs = parse_mirc("\u{3}4red\u{3}plain");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].style.fg, Some(4));
        assert_eq!(segs[1].style.fg, None);
        assert_eq!(segs[1].text, "plain");
    }

    #[test]
    fn bold_toggles_and_survives_across_a_color_change() {
        let segs = parse_mirc("\u{2}bold\u{3}4still bold and red");
        assert_eq!(segs.len(), 2);
        assert!(segs[0].style.bold);
        assert!(segs[1].style.bold);
        assert_eq!(segs[1].style.fg, Some(4));
    }

    #[test]
    fn reset_clears_every_attribute() {
        let segs = parse_mirc("\u{2}\u{3}4,1bold red\u{f}plain again");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].style, MircStyle::default());
    }

    #[test]
    fn a_digit_right_after_text_is_not_swallowed_as_a_color_code() {
        // No \x03 preceding it -- "2026" is just ordinary text.
        let segs = parse_mirc("countdown to 2026");
        assert_eq!(segs[0].text, "countdown to 2026");
    }

    #[test]
    fn color_digits_cap_at_two_so_trailing_digits_stay_in_the_text() {
        let segs = parse_mirc("\u{3}12345");
        assert_eq!(segs[0].style.fg, Some(12));
        assert_eq!(segs[0].text, "345");
    }
}
