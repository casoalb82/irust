//! Splitting an over-long outgoing message into wire-sized lines, with
//! irssi's `split_line_start` / `split_line_end` / `split_line_on_space`
//! settings (`irc/core/irc-servers.c` `split_line`).

use irust_config::settings_net::{bool_of, str_of};

/// Splits `text` into pieces of at most `limit` characters. With
/// `on_space` the cut goes at the last whitespace inside the window
/// (whitespace around the cut is dropped) unless the window has none;
/// without it every piece is a hard `limit`-character slice.
fn split_plain(text: &str, limit: usize, on_space: bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut pos = 0;
    while chars.len() - pos > limit {
        let window_end = pos + limit;
        let mut split_at = window_end;
        if on_space {
            while split_at > pos && !chars[split_at].is_whitespace() {
                split_at -= 1;
            }
            if split_at == pos {
                split_at = window_end; // no whitespace anywhere in this window
            }
        }
        let piece: String = chars[pos..split_at].iter().collect();
        let piece = if on_space { piece.trim().to_string() } else { piece };
        if !piece.is_empty() {
            chunks.push(piece);
        }
        pos = split_at;
        if on_space {
            while pos < chars.len() && chars[pos].is_whitespace() {
                pos += 1;
            }
        }
    }
    let last: String = chars[pos..].iter().collect();
    let last = if on_space { last.trim().to_string() } else { last };
    if !last.is_empty() {
        chunks.push(last);
    }
    chunks
}

/// The full irssi pipeline for one message, using the live settings.
pub fn split_for_wire(text: &str, limit: usize) -> Vec<String> {
    split_with(text, limit, &str_of("split_line_start"), &str_of("split_line_end"), bool_of("split_line_on_space"))
}

fn split_with(text: &str, limit: usize, start: &str, end: &str, on_space: bool) -> Vec<String> {
    let (start_len, end_len) = (start.chars().count(), end.chars().count());
    // irssi refuses to send anything when start+end leave no room; here the
    // affixes are simply ignored in that case rather than dropping the message.
    let (start, end, len) = if start_len + end_len >= limit {
        ("", "", limit)
    } else {
        (start, end, limit - start_len - end_len)
    };
    let mut lines = split_plain(text, len, on_space);
    let mut i = 0;
    while i < lines.len() {
        if i != 0 && !start.is_empty() {
            lines[i] = format!("{start}{}", lines[i]);
        }
        if i + 1 < lines.len() && !end.is_empty() {
            if i + 2 == lines.len() && lines[i + 1].chars().count() <= end_len {
                // Next-to-last line: the last one is no longer than the suffix,
                // so append it here instead of breaking the line for it.
                let last = lines.pop().unwrap_or_default();
                lines[i].push_str(&last);
                break;
            }
            lines[i].push_str(end);
        }
        i += 1;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_untouched_even_with_affixes() {
        assert_eq!(split_with("hey there", 400, ">", "…", true), vec!["hey there".to_string()]);
    }

    #[test]
    fn words_stay_whole_and_nothing_is_lost_by_default() {
        let long = "word ".repeat(150);
        let chunks = split_with(&long, 400, "", "", true);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= 400 && c.split(' ').all(|w| w == "word")));
        assert_eq!(chunks.join(" ").split(' ').count(), long.trim().split(' ').count());
    }

    #[test]
    fn a_single_long_word_is_hard_split() {
        let chunks = split_with(&"a".repeat(1000), 400, "", "", true);
        assert_eq!(chunks.iter().map(|c| c.chars().count()).collect::<Vec<_>>(), vec![400, 400, 200]);
    }

    #[test]
    fn on_space_off_slices_at_exactly_the_limit_without_trimming() {
        let chunks = split_with(&format!("{} {}", "a".repeat(9), "b".repeat(9)), 10, "", "", false);
        assert_eq!(chunks, vec!["aaaaaaaaa ".to_string(), "bbbbbbbbb".to_string()]);
    }

    #[test]
    fn start_goes_on_continuation_lines_and_end_on_all_but_the_last() {
        let chunks = split_with(&"a".repeat(30), 12, "+", "-", false);
        // 12 - 1 - 1 = 10 chars of text per line.
        assert_eq!(chunks, vec!["aaaaaaaaaa-".to_string(), "+aaaaaaaaaa-".to_string(), "+aaaaaaaaaa".to_string()]);
    }

    #[test]
    fn a_short_last_line_is_merged_into_the_previous_one() {
        // 14 chars at 16 - 3 = 13 per line: pieces of 13 and 1. The last one is
        // no longer than the 3-char suffix, so it is appended instead of
        // producing a second line (and no suffix is added).
        assert_eq!(split_with(&"a".repeat(14), 16, "", "...", false), vec!["a".repeat(14)]);
        // A longer tail is not merged: the first line gets the suffix.
        assert_eq!(split_with(&"a".repeat(17), 16, "", "...", false), vec![format!("{}...", "a".repeat(13)), "aaaa".to_string()]);
    }

    #[test]
    fn affixes_that_leave_no_room_are_ignored() {
        let chunks = split_with(&"a".repeat(15), 10, "12345", "67890", false);
        assert_eq!(chunks, vec!["a".repeat(10), "a".repeat(5)]);
    }
}
