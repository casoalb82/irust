//! "Is this channel message addressed to us?" -- irssi's
//! `core/nicklist.c` `nick_match_msg` (message starts with our nick, with
//! some fuzziness) and `nick_match_msg_everywhere` (our nick as a whole word
//! anywhere), picked by `/set hilight_nick_matches` and
//! `/set hilight_nick_matches_everywhere`.

use irust_config::lookandfeel;

fn alnum_high(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b >= 128
}

/// Port of `nick_match_msg`. `channel_has_nick` answers irssi's
/// `nick_nfind(channel, word)`: when the match needed fuzziness, another
/// exact nick in the channel matching the addressed word wins and the
/// message is not for us.
pub fn nick_match_msg(msg: &str, nick: &str, channel_has_nick: &dyn Fn(&str) -> bool) -> bool {
    let m = msg.as_bytes();
    let n = nick.as_bytes();
    if n.is_empty() {
        return false;
    }
    // identical match: the message starts with the nick and the next char
    // is not part of a word
    if m.len() >= n.len() && m[..n.len()].eq_ignore_ascii_case(n) && !alnum_high(m.get(n.len()).copied().unwrap_or(0))
    {
        return true;
    }

    let mut mi = 0;
    let mut ni;
    let mut msgstart;
    let mut fullmatch;
    loop {
        ni = 0;
        msgstart = mi;
        fullmatch = true;
        // match the alphanumeric parts of the nick
        while ni < n.len() && mi < m.len() {
            if n[ni].eq_ignore_ascii_case(&m[mi]) {
                mi += 1;
            } else if m[mi].is_ascii_alphanumeric() && !n[ni].is_ascii_alphanumeric() {
                // a strange char in the nick that the message left out
                fullmatch = false;
            } else {
                break;
            }
            ni += 1;
        }
        let next = m.get(mi).copied().unwrap_or(0);
        if mi != msgstart && !alnum_high(next) {
            if ni < n.len() {
                // drop the rest of the non-alphanumeric chars of the nick
                fullmatch = false;
                while ni < n.len() && !n[ni].is_ascii_alphanumeric() {
                    ni += 1;
                }
            }
            if ni >= n.len() {
                break; // match
            }
        }
        // no match: maybe a message to several people ("nick1,nick2: text")
        while mi < m.len() && m[mi] != b' ' && m[mi] != b',' {
            mi += 1;
        }
        if m.get(mi) != Some(&b',') {
            return false;
        }
        mi += 1;
    }

    if fullmatch {
        return true;
    }
    !channel_has_nick(&String::from_utf8_lossy(&m[msgstart..mi]))
}

/// `nick_match_msg_everywhere` (`stristr_full`): the nick as a whole word
/// anywhere in the message, case-insensitive.
pub fn nick_match_msg_everywhere(msg: &str, nick: &str) -> bool {
    crate::highlight::mentions_nick(msg, nick)
}

/// Whether a public message/notice counts as being for us, per the two
/// `hilight_nick_matches*` settings.
pub fn for_me(msg: &str, our_nick: &str, channel_has_nick: &dyn Fn(&str) -> bool) -> bool {
    if !lookandfeel::get_bool("hilight_nick_matches") {
        false
    } else if lookandfeel::get_bool("hilight_nick_matches_everywhere") {
        nick_match_msg_everywhere(msg, our_nick)
    } else {
        nick_match_msg(msg, our_nick, channel_has_nick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn a_message_starting_with_the_nick_matches() {
        assert!(nick_match_msg("abcX: hello", "abcX", &none));
        assert!(nick_match_msg("ABCX, hello", "abcX", &none));
        assert!(nick_match_msg("abcX", "abcX", &none), "the nick alone");
        assert!(nick_match_msg("abcX hello", "abcX", &none));
    }

    #[test]
    fn the_nick_later_in_the_message_does_not_match_without_everywhere() {
        assert!(!nick_match_msg("hello abcX", "abcX", &none));
        assert!(!nick_match_msg("thanks abcX!", "abcX", &none));
        assert!(nick_match_msg_everywhere("hello abcX", "abcX"));
        assert!(nick_match_msg_everywhere("thanks abcX!", "abcX"));
    }

    #[test]
    fn a_longer_word_starting_with_the_nick_is_not_a_match() {
        assert!(!nick_match_msg("abcXtreme: hi", "abcX", &none));
        assert!(!nick_match_msg_everywhere("abcXtreme hi", "abcX"));
    }

    #[test]
    fn a_message_to_several_people_matches_any_of_them() {
        assert!(nick_match_msg("bob,abcX: hi both", "abcX", &none));
        assert!(!nick_match_msg("bob,carol: hi both", "abcX", &none));
    }

    #[test]
    fn fuzzy_match_may_omit_trailing_symbols_of_the_nick() {
        // "abc:" addresses the nick "abc_" (only symbols were left out)...
        assert!(nick_match_msg("abc: hi", "abc_", &none));
        // ...but not one whose remainder has letters in it
        assert!(!nick_match_msg("abc: hi", "abc|away", &none));
        // ...and not when "abc" is itself another nick in the channel
        assert!(!nick_match_msg("abc: hi", "abc_", &|w| w == "abc"));
        // an exact match never asks the channel
        assert!(nick_match_msg("abc_: hi", "abc_", &|_| true));
    }

    #[test]
    fn empty_nick_never_matches() {
        assert!(!nick_match_msg("hi", "", &none));
    }
}
