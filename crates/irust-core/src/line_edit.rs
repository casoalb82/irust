
/// One char left of `cursor` (a byte offset into `s`), clamped to `0`.
pub fn prev_boundary(s: &str, cursor: usize) -> usize {
    match s[..cursor].chars().next_back() {
        Some(c) => cursor - c.len_utf8(),
        None => 0,
    }
}

/// One char right of `cursor`, clamped to `s.len()`.
pub fn next_boundary(s: &str, cursor: usize) -> usize {
    match s[cursor..].chars().next() {
        Some(c) => cursor + c.len_utf8(),
        None => s.len(),
    }
}

/// Start of the word before `cursor` -- skips any whitespace
/// immediately to the left first, then the run of non-whitespace before
/// that (irssi/readline's own "word" here just means a whitespace-
/// delimited run, not readline's fancier `WORDCHARS`-based
/// punctuation splitting).
pub fn word_left(s: &str, cursor: usize) -> usize {
    let mut i = cursor;
    while i > 0 {
        let p = prev_boundary(s, i);
        if s[p..i].chars().next().is_some_and(char::is_whitespace) {
            i = p;
        } else {
            break;
        }
    }
    while i > 0 {
        let p = prev_boundary(s, i);
        if s[p..i].chars().next().is_some_and(|c| !c.is_whitespace()) {
            i = p;
        } else {
            break;
        }
    }
    i
}

/// End of the word after `cursor` -- mirror of `word_left`: skips
/// whitespace first, then the run of non-whitespace after that.
pub fn word_right(s: &str, cursor: usize) -> usize {
    let len = s.len();
    let mut i = cursor;
    while i < len {
        let n = next_boundary(s, i);
        if s[i..n].chars().next().is_some_and(char::is_whitespace) {
            i = n;
        } else {
            break;
        }
    }
    while i < len {
        let n = next_boundary(s, i);
        if s[i..n].chars().next().is_some_and(|c| !c.is_whitespace()) {
            i = n;
        } else {
            break;
        }
    }
    i
}

/// Inserts one char at `cursor`, returning the cursor position right
/// after it.
pub fn insert_char(s: &mut String, cursor: usize, c: char) -> usize {
    s.insert(cursor, c);
    cursor + c.len_utf8()
}

/// Inserts `text` at `cursor` (used for paste and `yank`), returning
/// the cursor position right after it.
pub fn insert_str(s: &mut String, cursor: usize, text: &str) -> usize {
    s.insert_str(cursor, text);
    cursor + text.len()
}

/// Deletes the char immediately before `cursor` (classic Backspace),
/// returning the new cursor position. A no-op at the start of the line.
pub fn backspace(s: &mut String, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let p = prev_boundary(s, cursor);
    s.drain(p..cursor);
    p
}

/// Deletes the char immediately at `cursor` (forward-delete / the
/// `Delete`/`Canc` key). Cursor position doesn't change -- whatever
/// used to be after the deleted char slides left into its place.
pub fn delete_forward(s: &mut String, cursor: usize) {
    if cursor >= s.len() {
        return;
    }
    let n = next_boundary(s, cursor);
    s.drain(cursor..n);
}

/// `Ctrl-W`: deletes the word immediately before `cursor`, returning
/// `(new_cursor, deleted_text)` -- the deleted text is what the caller
/// stores in the one-slot cutbuffer for a later `yank`.
pub fn delete_word_backward(s: &mut String, cursor: usize) -> (usize, String) {
    let start = word_left(s, cursor);
    let removed = s.drain(start..cursor).collect();
    (start, removed)
}

/// `Ctrl-K`: deletes from `cursor` to the end of the line, returning
/// the deleted text for the cutbuffer. Cursor position is unaffected
/// (it was already at the start of what got removed).
pub fn kill_to_end(s: &mut String, cursor: usize) -> String {
    s.drain(cursor..).collect()
}

/// Deletes from the start of the line up to `cursor`, returning the
/// deleted text for the cutbuffer. The caller is responsible for
/// resetting its own cursor variable to `0` afterwards.
pub fn kill_to_start(s: &mut String, cursor: usize) -> String {
    s.drain(..cursor).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_step_one_char_at_a_time_across_multibyte_text() {
        let s = "ciao à тест";
        let mut c = s.len();
        // Walking backward to 0 must never panic (would, on a naive
        // `cursor - 1`, the moment it hit a multi-byte char).
        let mut count = 0;
        while c > 0 {
            c = prev_boundary(s, c);
            count += 1;
        }
        assert_eq!(count, s.chars().count());
        // And forward again lands back at the end.
        let mut c = 0;
        while c < s.len() {
            c = next_boundary(s, c);
        }
        assert_eq!(c, s.len());
    }

    #[test]
    fn insert_and_backspace_around_a_multibyte_char() {
        let mut s = "café".to_string();
        let cursor = s.len(); // right after the 'é' (2 bytes)
        let cursor = backspace(&mut s, cursor);
        assert_eq!(s, "caf");
        assert_eq!(cursor, "caf".len());
    }

    #[test]
    fn delete_forward_removes_the_char_under_the_cursor_not_before_it() {
        let mut s = "hello".to_string();
        delete_forward(&mut s, 1); // cursor between 'h' and 'e'
        assert_eq!(s, "hllo");
    }

    #[test]
    fn word_left_skips_trailing_space_then_the_word() {
        let s = "hello world ";
        assert_eq!(word_left(s, s.len()), "hello ".len()); // -> start of "world"
        assert_eq!(word_left(s, "hello ".len()), 0); // -> start of "hello"
    }

    #[test]
    fn word_right_skips_the_word_then_trailing_space() {
        let s = "hello world";
        assert_eq!(word_right(s, 0), "hello".len());
        assert_eq!(word_right(s, "hello".len()), s.len());
    }

    #[test]
    fn delete_word_backward_returns_the_removed_word_for_the_cutbuffer() {
        let mut s = "join #chatnet now".to_string();
        let cursor = "join #chatnet".len();
        let (new_cursor, removed) = delete_word_backward(&mut s, cursor);
        assert_eq!(removed, "#chatnet");
        assert_eq!(s, "join  now");
        assert_eq!(new_cursor, "join ".len());
    }

    #[test]
    fn kill_to_end_then_yank_round_trips() {
        let mut s = "hello world".to_string();
        let cursor = "hello".len();
        let killed = kill_to_end(&mut s, cursor);
        assert_eq!(killed, " world");
        assert_eq!(s, "hello");
        let end = s.len();
        let cursor = insert_str(&mut s, end, &killed);
        assert_eq!(s, "hello world");
        assert_eq!(cursor, s.len());
    }

    #[test]
    fn kill_to_start_removes_everything_before_the_cursor() {
        let mut s = "hello world".to_string();
        let cursor = "hello ".len();
        let killed = kill_to_start(&mut s, cursor);
        assert_eq!(killed, "hello ");
        assert_eq!(s, "world");
    }

    #[test]
    fn insert_char_advances_cursor_by_the_chars_utf8_width() {
        let mut s = "caf".to_string();
        let end = s.len();
        let cursor = insert_char(&mut s, end, 'é');
        assert_eq!(s, "café");
        assert_eq!(cursor, s.len());
    }
}
