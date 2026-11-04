fn contains_whole_word(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }

    let text_lower = text.to_lowercase();
    let word_lower = word.to_lowercase();
    let bytes = text_lower.as_bytes();

    let mut start = 0;
    while let Some(rel_pos) = text_lower[start..].find(&word_lower) {
        let abs_pos = start + rel_pos;
        let end = abs_pos + word_lower.len();

        let before_ok = abs_pos == 0 || !bytes[abs_pos - 1].is_ascii_alphanumeric();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }

        start = abs_pos + 1;
    }

    false
}

/// True if `text` mentions `nick` as a whole word, case-insensitive.
pub fn mentions_nick(text: &str, nick: &str) -> bool {
    contains_whole_word(text, nick)
}

pub fn matches_any_hilight(
    text: &str,
    sender_mask: &str,
    channel: Option<&str>,
    entries: &[irust_config::HilightEntry],
) -> bool {
    find_matching_hilight(text, sender_mask, channel, entries).is_some()
}

fn entry_matches(entry: &irust_config::HilightEntry, text: &str, sender_mask: &str, channel: Option<&str>) -> bool {
    let channel_ok = entry.channels.is_empty()
        || channel.is_some_and(|c| entry.channels.iter().any(|allowed| allowed.eq_ignore_ascii_case(c)));
    if !channel_ok {
        return false;
    }
    let target = if entry.is_mask { sender_mask } else { text };
    if entry.is_regex {
        crate::nicklist::matches_regex_cached(&entry.pattern, target)
    } else if entry.is_mask {
        crate::nicklist::matches_glob(&entry.pattern, target)
    } else {
        contains_whole_word(target, &entry.pattern)
    }
}

pub fn find_matching_hilight<'a>(
    text: &str,
    sender_mask: &str,
    channel: Option<&str>,
    entries: &'a [irust_config::HilightEntry],
) -> Option<&'a irust_config::HilightEntry> {
    let mut best: Option<&irust_config::HilightEntry> = None;
    for entry in entries {
        if !entry_matches(entry, text, sender_mask, channel) {
            continue;
        }
        match best {
            Some(b) if entry.priority <= b.priority => {}
            _ => best = Some(entry),
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_mention_is_detected() {
        assert!(mentions_nick("hey abcX how are you", "abcX"));
    }

    fn plain_entry(pattern: &str) -> irust_config::HilightEntry {
        irust_config::HilightEntry {
            pattern: pattern.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn hilight_matches_any_pattern_case_insensitively() {
        let entries = vec![plain_entry("urgent"), plain_entry("outage")];
        assert!(matches_any_hilight(
            "this is URGENT please read",
            "n!u@h",
            Some("#c"),
            &entries
        ));
        assert!(matches_any_hilight("we have an outage", "n!u@h", Some("#c"), &entries));
        assert!(!matches_any_hilight(
            "nothing to see here",
            "n!u@h",
            Some("#c"),
            &entries
        ));
    }

    #[test]
    fn hilight_respects_word_boundaries_like_mentions_nick() {
        let entries = vec![plain_entry("cat")];
        assert!(!matches_any_hilight("category error", "n!u@h", Some("#c"), &entries));
        assert!(matches_any_hilight("my cat is asleep", "n!u@h", Some("#c"), &entries));
    }

    #[test]
    fn hilight_regexp_flag_matches_against_text() {
        let entries = vec![irust_config::HilightEntry {
            pattern: r"err(or)?\s*\d+".to_string(),
            is_regex: true,
            ..Default::default()
        }];
        assert!(matches_any_hilight(
            "got error 42 on the build",
            "n!u@h",
            Some("#c"),
            &entries
        ));
        assert!(!matches_any_hilight("no problems here", "n!u@h", Some("#c"), &entries));
    }

    #[test]
    fn hilight_mask_flag_matches_against_the_sender_not_the_text() {
        let entries = vec![irust_config::HilightEntry {
            pattern: "*!*@spammer.example".to_string(),
            is_mask: true,
            ..Default::default()
        }];
        assert!(matches_any_hilight(
            "totally ordinary message",
            "BadBot!ident@spammer.example",
            Some("#c"),
            &entries
        ));
        assert!(!matches_any_hilight(
            "totally ordinary message",
            "GoodNick!ident@safe.example",
            Some("#c"),
            &entries
        ));
    }

    #[test]
    fn hilight_mask_and_regexp_together_matches_the_sender_as_a_regex() {
        let entries = vec![irust_config::HilightEntry {
            pattern: r"^Bad.*@spammer\.example$".to_string(),
            is_regex: true,
            is_mask: true,
            ..Default::default()
        }];
        assert!(matches_any_hilight(
            "hi",
            "BadBot!ident@spammer.example",
            Some("#c"),
            &entries
        ));
        assert!(!matches_any_hilight(
            "hi",
            "GoodNick!ident@spammer.example",
            Some("#c"),
            &entries
        ));
    }

    #[test]
    fn hilight_channels_flag_restricts_to_named_channels() {
        let entries = vec![irust_config::HilightEntry {
            pattern: "urgent".to_string(),
            channels: vec!["#ops".to_string()],
            ..Default::default()
        }];
        assert!(matches_any_hilight("urgent!", "n!u@h", Some("#ops"), &entries));
        assert!(
            !matches_any_hilight("urgent!", "n!u@h", Some("#random"), &entries),
            "wrong channel"
        );
        assert!(
            !matches_any_hilight("urgent!", "n!u@h", None, &entries),
            "a query is never a scoped channel"
        );
    }

    #[test]
    fn empty_pattern_list_never_matches() {
        assert!(!matches_any_hilight("urgent outage", "n!u@h", Some("#c"), &[]));
    }

    #[test]
    fn find_matching_hilight_returns_the_entry_that_fired() {
        let entries = vec![irust_config::HilightEntry {
            pattern: "urgent".to_string(),
            color: Some("203".to_string()),
            ..Default::default()
        }];
        let found = find_matching_hilight("this is urgent", "n!u@h", Some("#c"), &entries);
        assert_eq!(found.map(|e| e.color.as_deref()), Some(Some("203")));
    }

    #[test]
    fn find_matching_hilight_returns_none_when_nothing_matches() {
        let entries = vec![plain_entry("urgent")];
        assert!(find_matching_hilight("nothing here", "n!u@h", Some("#c"), &entries).is_none());
    }

    #[test]
    fn find_matching_hilight_first_match_wins_when_several_would_match() {
        let entries = vec![
            irust_config::HilightEntry {
                pattern: "urgent".to_string(),
                color: Some("red".to_string()),
                ..Default::default()
            },
            irust_config::HilightEntry {
                pattern: "urgent".to_string(),
                color: Some("blue".to_string()),
                ..Default::default()
            },
        ];
        let found = find_matching_hilight("urgent!", "n!u@h", Some("#c"), &entries);
        assert_eq!(found.and_then(|e| e.color.as_deref()), Some("red"));
    }

    #[test]
    fn find_matching_hilight_prefers_higher_priority_over_list_order() {
        let entries = vec![
            irust_config::HilightEntry {
                pattern: "urgent".to_string(),
                color: Some("red".to_string()),
                ..Default::default()
            },
            irust_config::HilightEntry {
                pattern: "urgent".to_string(),
                color: Some("blue".to_string()),
                priority: 5,
                ..Default::default()
            },
        ];
        let found = find_matching_hilight("urgent!", "n!u@h", Some("#c"), &entries);
        assert_eq!(
            found.and_then(|e| e.color.as_deref()),
            Some("blue"),
            "higher priority beats list order"
        );
    }

    #[test]
    fn find_matching_hilight_still_breaks_equal_priority_ties_by_list_order() {
        let entries = vec![
            irust_config::HilightEntry {
                pattern: "urgent".to_string(),
                color: Some("red".to_string()),
                priority: 5,
                ..Default::default()
            },
            irust_config::HilightEntry {
                pattern: "urgent".to_string(),
                color: Some("blue".to_string()),
                priority: 5,
                ..Default::default()
            },
        ];
        let found = find_matching_hilight("urgent!", "n!u@h", Some("#c"), &entries);
        assert_eq!(
            found.and_then(|e| e.color.as_deref()),
            Some("red"),
            "equal priority still falls back to list order"
        );
    }

    #[test]
    fn find_matching_hilight_a_negative_priority_still_beats_nothing() {
        let entries = vec![irust_config::HilightEntry {
            pattern: "urgent".to_string(),
            priority: -3,
            ..Default::default()
        }];
        assert!(find_matching_hilight("urgent!", "n!u@h", Some("#c"), &entries).is_some());
    }

    #[test]
    fn is_case_insensitive() {
        assert!(mentions_nick("hey ABCX", "abcX"));
        assert!(mentions_nick("hey abcx", "ABCX"));
    }

    #[test]
    fn punctuation_boundaries_still_count() {
        assert!(mentions_nick("abcX: check this out", "abcX"));
        assert!(mentions_nick("thanks, abcX!", "abcX"));
    }

    #[test]
    fn substring_of_a_longer_word_does_not_count() {
        assert!(!mentions_nick("abcXtra is a different nick", "abcX"));
        assert!(!mentions_nick("this is fooabcX", "abcX"));
    }

    #[test]
    fn no_mention_at_all() {
        assert!(!mentions_nick("hello everyone", "abcX"));
    }

    #[test]
    fn empty_nick_never_matches() {
        assert!(!mentions_nick("hello", ""));
    }
}
