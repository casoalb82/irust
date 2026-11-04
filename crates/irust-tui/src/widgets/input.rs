use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

fn masked_for_display(input: &str) -> String {
    let mask = |s: &str| "*".repeat(s.chars().count());

    for prefix in ["/oper ", "oper "] {
        if let Some(rest) = strip_prefix_ci(input, prefix) {
            if let Some((user, password)) = rest.split_once(' ') {
                let command = &prefix[..prefix.len() - 1]; // drop the trailing space
                return format!("{command} {user} {}", mask(password));
            }
        }
    }
    for prefix in ["/quote pass ", "quote pass ", "pass "] {
        if let Some(password) = strip_prefix_ci(input, prefix) {
            let command = prefix.trim_end().to_uppercase().replace("QUOTE", "quote");
            return format!("{command} {}", mask(password));
        }
    }
    if let Some(rest) = strip_prefix_ci(input, "/msg ") {
        let mut parts = rest.splitn(3, ' ');
        if let (Some(target), Some(cmd), Some(password)) = (parts.next(), parts.next(), parts.next()) {
            if cmd.eq_ignore_ascii_case("identify") || cmd.eq_ignore_ascii_case("id") {
                return format!("/msg {target} {cmd} {}", mask(password));
            }
        }
    }
    // `/ns id <password>` / `/ns identify <password>` -- ChatNet's own
    // NickServ shorthand (same "and
    // without the slash too" treatment as everything above).
    for prefix in ["/ns ", "ns "] {
        if let Some(rest) = strip_prefix_ci(input, prefix) {
            if let Some((cmd, password)) = rest.split_once(' ') {
                if cmd.eq_ignore_ascii_case("id") || cmd.eq_ignore_ascii_case("identify") {
                    let command = &prefix[..prefix.len() - 1];
                    return format!("{command} {cmd} {}", mask(password));
                }
            }
        }
    }
    if let Some(password) = strip_prefix_ci(input, "identify ") {
        return format!("identify {}", mask(password));
    }
    input.to_string()
}

/// Case-insensitive prefix strip that still returns the ORIGINAL
/// (un-lowercased) remainder -- masking must preserve whatever case the
/// password itself was actually typed in, only the command word's case
/// is meant to be flexible.
fn strip_prefix_ci<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    let head = input.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

pub fn render(frame: &mut Frame, area: Rect, input: &str, cursor: usize) {
    let cursor_chars = input[..cursor.min(input.len())].chars().count();

    let masked = masked_for_display(input);
    let input = masked.as_str();
    let inner_width = area.width.saturating_sub(2) as usize;
    let chars: Vec<char> = input.chars().collect();
    let total = chars.len();
    let cursor_chars = cursor_chars.min(total);
    let start = cursor_chars.saturating_sub(inner_width);
    let end = (start + inner_width).min(total);
    let visible: String = chars[start..end].iter().collect();
    let cursor_x = (cursor_chars - start) as u16;

    // No command-hint title -- irssi's own input line doesn't carry
    // one either.
    let paragraph = Paragraph::new(visible).block(Block::default().borders(Borders::ALL));
    frame.render_widget(paragraph, area);
    frame.set_cursor_position(Position::new(area.x + 1 + cursor_x, area.y + 1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oper_password_is_masked_same_length() {
        let masked = masked_for_display("/oper abcX hunter2");
        assert_eq!(masked, "/oper abcX *******");
        assert_eq!(masked.len(), "/oper abcX hunter2".len());
    }

    #[test]
    fn quote_pass_is_masked_regardless_of_typed_case() {
        assert_eq!(masked_for_display("/quote pass hunter2"), "/quote PASS *******");
        assert_eq!(masked_for_display("/QUOTE PASS hunter2"), "/quote PASS *******");
    }

    #[test]
    fn msg_identify_password_is_masked() {
        let masked = masked_for_display("/msg NickServ IDENTIFY hunter2");
        assert_eq!(masked, "/msg NickServ IDENTIFY *******");
    }

    #[test]
    fn masking_still_works_without_the_leading_slash() {
        assert_eq!(masked_for_display("oper abcX hunter2"), "oper abcX *******");
        assert_eq!(masked_for_display("quote pass hunter2"), "quote PASS *******");
        assert_eq!(masked_for_display("pass hunter2"), "PASS *******");
        assert_eq!(masked_for_display("identify hunter2"), "identify *******");
    }

    /// ChatNet's own NickServ shorthand.
    #[test]
    fn ns_id_password_is_masked_with_and_without_slash() {
        assert_eq!(masked_for_display("/ns id hunter2"), "/ns id *******");
        assert_eq!(masked_for_display("ns id hunter2"), "ns id *******");
        assert_eq!(masked_for_display("/ns identify hunter2"), "/ns identify *******");
        assert_eq!(
            masked_for_display("/msg NickServ id hunter2"),
            "/msg NickServ id *******"
        );
    }

    #[test]
    fn masked_output_always_matches_the_original_length() {
        for input in [
            "/oper abcX hunter2",
            "oper abcX hunter2",
            "/quote pass hunter2",
            "quote pass hunter2",
            "pass hunter2",
            "/msg NickServ IDENTIFY hunter2",
            "identify hunter2",
            "/ns id hunter2",
            "ns id hunter2",
        ] {
            assert_eq!(
                masked_for_display(input).len(),
                input.len(),
                "length mismatch for {input:?}"
            );
        }
    }

    #[test]
    fn ordinary_messages_are_never_masked() {
        assert_eq!(masked_for_display("hey everyone"), "hey everyone");
        assert_eq!(masked_for_display("/msg K9 HELP"), "/msg K9 HELP");
        assert_eq!(masked_for_display("/join #chatnet"), "/join #chatnet");
    }
}
