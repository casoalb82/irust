
/// Canonical key-name syntax: one of the named keys below, or
/// `ctrl-<letter>` for a Ctrl-held printable character. Case-
/// insensitive on input; always stored/compared lowercase.
const NAMED_KEYS: &[&str] = &[
    "tab",
    "enter",
    "up",
    "down",
    "left",
    "right",
    "pageup",
    "pagedown",
    "home",
    "end",
    "backspace",
    "delete",
    "esc",
    // Compound arrow combos don't fit the generic `ctrl-<letter>`/
    // `alt-<letter>` parsing below (more than one char after the
    // dash), so they're listed here as their own literal names instead.
    "ctrl-left",
    "ctrl-right",
];

/// Every action `/bind` can point a key at. Deliberately scoped to what
/// `irust-tui/app.rs` already does today -- no arbitrary irssi-style
/// "bind a key to any command" generality.
pub const BINDABLE_ACTIONS: &[&str] = &[
    "tab_complete",
    "submit",
    "history_up",
    "history_down",
    "quit",
    "cycle_next",
    "cycle_prev",
    "server_next",
    "server_prev",
    "page_up",
    "page_down",
    "scroll_home",
    "scroll_end",
    "backspace",
    "nicklist_up",
    "nicklist_down",
    "goto_buffer_1",
    "goto_buffer_2",
    "goto_buffer_3",
    "goto_buffer_4",
    "goto_buffer_5",
    "goto_buffer_6",
    "goto_buffer_7",
    "goto_buffer_8",
    "goto_buffer_9",
    // Cursor-based input-line editing -- irssi parity:
    // before this the input box was append/backspace-only, no way to
    // move the cursor or edit mid-line at all.
    "cursor_left",
    "cursor_right",
    "cursor_word_left",
    "cursor_word_right",
    "cursor_line_start",
    "cursor_line_end",
    "delete_word_backward",
    "kill_to_end",
    "kill_to_start",
    "yank",
    "delete_forward",
];

/// The keys' factory-default actions, exactly matching what
/// `irust-tui/app.rs`'s key handler did before `/bind` existed. A
/// `/bind`-set override for one of these keys takes priority; `/bind
/// -delete <key>` reverts to this.
pub const DEFAULT_KEYBINDS: &[(&str, &str)] = &[
    ("tab", "tab_complete"),
    ("enter", "submit"),
    ("up", "history_up"),
    ("down", "history_down"),
    ("ctrl-c", "quit"),
    ("esc", "quit"),
    ("ctrl-n", "cycle_next"),
    ("ctrl-p", "cycle_prev"),
    // Switch network (each network keeps its own open window). Ctrl-A, which
    // used to be `cursor_line_start` (still bindable: `/bind <key>
    // cursor_line_start`); `server_prev` has no default key.
    ("ctrl-a", "server_next"),
    ("pageup", "page_up"),
    ("pagedown", "page_down"),
    ("home", "scroll_home"),
    ("end", "scroll_end"),
    ("backspace", "backspace"),
    // vim-style up/down, distinct from ctrl-n/ctrl-p (buffer cycling) --
    // added for scrolling the nicklist itself once a channel has more
    // nicks than the sidebar's height.
    ("ctrl-u", "nicklist_up"),
    ("ctrl-d", "nicklist_down"),
    ("alt-1", "goto_buffer_1"),
    ("alt-2", "goto_buffer_2"),
    ("alt-3", "goto_buffer_3"),
    ("alt-4", "goto_buffer_4"),
    ("alt-5", "goto_buffer_5"),
    ("alt-6", "goto_buffer_6"),
    ("alt-7", "goto_buffer_7"),
    ("alt-8", "goto_buffer_8"),
    ("alt-9", "goto_buffer_9"),
    ("left", "cursor_left"),
    ("right", "cursor_right"),
    ("ctrl-left", "cursor_word_left"),
    ("ctrl-right", "cursor_word_right"),
    ("ctrl-e", "cursor_line_end"),
    ("ctrl-w", "delete_word_backward"),
    ("ctrl-k", "kill_to_end"),
    ("ctrl-y", "yank"),
    ("delete", "delete_forward"),
];

pub fn normalize_key_name(input: &str) -> Option<String> {
    let lower = input.to_lowercase().replace('+', "-");
    if NAMED_KEYS.contains(&lower.as_str()) {
        return Some(lower);
    }
    if let Some(rest) = lower.strip_prefix("ctrl-") {
        let mut chars = rest.chars();
        let c = chars.next()?;
        if chars.next().is_none() && c.is_ascii_alphanumeric() {
            return Some(format!("ctrl-{c}"));
        }
    }
    if let Some(rest) = lower.strip_prefix("alt-") {
        let mut chars = rest.chars();
        let c = chars.next()?;
        if chars.next().is_none() && c.is_ascii_alphanumeric() {
            return Some(format!("alt-{c}"));
        }
    }
    None
}

/// `true` if `action` is one `/bind` can actually point a key at.
pub fn is_valid_action(action: &str) -> bool {
    BINDABLE_ACTIONS.contains(&action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_normalize_case_insensitively() {
        assert_eq!(normalize_key_name("PageUp"), Some("pageup".to_string()));
        assert_eq!(normalize_key_name("HOME"), Some("home".to_string()));
    }

    #[test]
    fn ctrl_letter_normalizes_with_either_separator() {
        assert_eq!(normalize_key_name("Ctrl-N"), Some("ctrl-n".to_string()));
        assert_eq!(normalize_key_name("CTRL+n"), Some("ctrl-n".to_string()));
    }

    #[test]
    fn ctrl_digit_is_accepted() {
        assert_eq!(normalize_key_name("ctrl-1"), Some("ctrl-1".to_string()));
    }

    #[test]
    fn unrecognized_syntax_is_none() {
        assert_eq!(normalize_key_name("F1"), None);
        assert_eq!(normalize_key_name("ctrl-"), None);
        assert_eq!(normalize_key_name("ctrl-ab"), None);
        assert_eq!(normalize_key_name(""), None);
    }

    #[test]
    fn action_validity() {
        assert!(is_valid_action("cycle_next"));
        assert!(!is_valid_action("nonexistent_action"));
    }

    #[test]
    fn ctrl_a_switches_network_by_default() {
        assert!(DEFAULT_KEYBINDS.contains(&("ctrl-a", "server_next")));
        assert!(is_valid_action("server_next") && is_valid_action("server_prev"));
        assert_eq!(DEFAULT_KEYBINDS.iter().filter(|(k, _)| *k == "ctrl-a").count(), 1, "no duplicate binding");
    }
}

