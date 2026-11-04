
use irust_config::lookandfeel;

fn high_alnum(b: u8) -> bool {
    b >= 128 || b.is_ascii_alphanumeric()
}

fn nick_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"`-_[]{}|\\^".contains(&b)
}

/// `is_nick` answers "is this word a nick in the channel" (a `_nick_` is
/// never emphasised); pass `|_| false` outside a channel.
pub fn expand_emphasis(text: String, is_nick: &dyn Fn(&str) -> bool) -> String {
    if !lookandfeel::get_bool("emphasis") {
        return text;
    }
    let italics = lookandfeel::get_bool("emphasis_italics");
    let multiword = lookandfeel::get_bool("emphasis_multiword");
    let replace = lookandfeel::get_bool("emphasis_replace");

    let mut s: Vec<u8> = text.into_bytes();
    let mut pos = 0;
    while pos < s.len() {
        let marker = s[pos];
        let kind: u8 = match marker {
            b'*' => 2,
            b'/' if italics => 29,
            b'_' => 31,
            _ => {
                pos += 1;
                continue;
            }
        };
        let at = |s: &[u8], i: usize| s.get(i).copied().unwrap_or(0);
        let bgn = pos;
        // the opening marker must start a word...
        if (bgn > 0 && s[bgn - 1] != b' ') || !high_alnum(at(&s, bgn + 1)) {
            pos += 1;
            continue;
        }
        // ...and its match must end one
        let Some(rel) = s[bgn + 1..].iter().position(|&c| c == marker) else {
            pos += 1;
            continue;
        };
        let end = bgn + 1 + rel;
        let after = at(&s, end + 1);
        if !high_alnum(s[end - 1])
            || high_alnum(after)
            || after == kind
            || after == b'*'
            || after == b'_'
            // don't emphasise common paths like /.../.X
            || (kind == 29 && after.is_ascii_punctuation() && high_alnum(at(&s, end + 2)))
        {
            pos += 1;
            continue;
        }
        // _nick_ (or _nick_^) in a channel is a nick, not emphasis
        let mut end2 = end;
        while nick_char(at(&s, end2 + 1)) {
            end2 += 1;
        }
        let word = |to: usize| String::from_utf8_lossy(&s[bgn..=to.min(s.len() - 1)]).into_owned();
        if is_nick(&word(end)) || is_nick(&word(end2)) {
            pos += 1;
            continue;
        }
        // *word*, not *several words*, unless emphasis_multiword
        if !multiword && !s[bgn + 1..end].iter().all(|&c| high_alnum(c)) {
            pos += 1;
            continue;
        }
        if replace {
            s[bgn] = kind;
            s[end] = kind;
            pos = end + 1;
        } else {
            s.insert(bgn, kind);
            // the closing marker moved one to the right
            s.insert(end + 2, kind);
            pos = end + 4;
        }
    }
    String::from_utf8_lossy(&s).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lookandfeel::with_test_overrides;

    fn on(text: &str) -> String {
        with_test_overrides(&[("emphasis", "on")], || expand_emphasis(text.to_string(), &|_| false))
    }

    #[test]
    fn off_by_default_leaves_text_alone() {
        with_test_overrides(&[], || assert_eq!(expand_emphasis("*hi*".to_string(), &|_| false), "*hi*"));
    }

    #[test]
    fn bold_and_underline_wrap_the_marked_word_and_keep_the_markers() {
        assert_eq!(on("say *hello* now"), "say \u{2}*hello*\u{2} now");
        assert_eq!(on("an _underlined_ word"), "an \u{1f}_underlined_\u{1f} word");
    }

    #[test]
    fn a_marker_must_start_and_end_a_word() {
        assert_eq!(on("2*3*4"), "2*3*4");
        assert_eq!(on("*hello"), "*hello");
        assert_eq!(on("*hello*world"), "*hello*world");
        assert_eq!(on("* spaced *"), "* spaced *");
    }

    #[test]
    fn several_words_need_emphasis_multiword() {
        assert_eq!(on("*two words*"), "*two words*");
        let multi = with_test_overrides(&[("emphasis", "on"), ("emphasis_multiword", "on")], || {
            expand_emphasis("*two words*".to_string(), &|_| false)
        });
        assert_eq!(multi, "\u{2}*two words*\u{2}");
    }

    #[test]
    fn italics_are_opt_in_and_skip_paths() {
        assert_eq!(on("a /slanted/ b"), "a /slanted/ b");
        let italic = |t: &str| {
            with_test_overrides(&[("emphasis", "on"), ("emphasis_italics", "on")], || {
                expand_emphasis(t.to_string(), &|_| false)
            })
        };
        assert_eq!(italic("a /slanted/ b"), "a \u{1d}/slanted/\u{1d} b");
        assert_eq!(italic("see /usr/lib/x.so"), "see /usr/lib/x.so", "a path is not italics");
    }

    #[test]
    fn replace_swaps_the_markers_for_the_codes() {
        let out = with_test_overrides(&[("emphasis", "on"), ("emphasis_replace", "on")], || {
            expand_emphasis("say *hello* now".to_string(), &|_| false)
        });
        assert_eq!(out, "say \u{2}hello\u{2} now");
    }

    #[test]
    fn a_nick_written_with_underscores_is_not_emphasised() {
        let out = with_test_overrides(&[("emphasis", "on")], || {
            expand_emphasis("hi _foo_ there".to_string(), &|w| w == "_foo_")
        });
        assert_eq!(out, "hi _foo_ there");
        assert_eq!(on("hi _foo_ there"), "hi \u{1f}_foo_\u{1f} there", "without that nick it is emphasis");
    }

    #[test]
    fn several_emphasised_words_and_utf8_survive() {
        assert_eq!(on("*a* and *b*"), "\u{2}*a*\u{2} and \u{2}*b*\u{2}");
        assert_eq!(on("caffè *ottimo* già"), "caffè \u{2}*ottimo*\u{2} già");
    }
}
