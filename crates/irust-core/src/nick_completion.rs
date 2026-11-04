
use irust_config::settings_history_log as cfg;

/// One entry of irssi's `lastmsgs` list: a nick that recently spoke (or
/// joined). `own > 0` marks nicks the user addressed / who addressed the
/// user, which sort ahead of the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastMsg {
    pub nick: String,
    pub own: u32,
}

/// irssi's `last_msg_add`: newest first, one entry per nick (case
/// insensitive), at most `max` entries (`max <= 0` keeps nothing). A
/// nick already present moves to the front; `own` resets its counter to
/// `max`, a non-own message ages it by one, and every other entry's
/// counter ages by one on each add.
pub fn last_msg_add(list: &mut Vec<LastMsg>, nick: &str, own: bool, max: i64) {
    if max <= 0 {
        return;
    }
    let max = max as usize;
    let mut rec = match list.iter().position(|r| r.nick.eq_ignore_ascii_case(nick)) {
        Some(i) => {
            let mut rec = list.remove(i);
            if own {
                rec.own = max as u32;
            } else if rec.own > 0 {
                rec.own -= 1;
            }
            rec
        }
        None => {
            while list.len() >= max {
                list.pop();
            }
            LastMsg { nick: nick.to_string(), own: if own { max as u32 } else { 0 } }
        }
    };
    for other in list.iter_mut() {
        if other.own > 0 {
            other.own -= 1;
        }
    }
    rec.nick = nick.to_string();
    list.insert(0, rec);
}

/// Renames an entry in place (`sig_nick_changed`).
pub fn last_msg_rename(list: &mut [LastMsg], old: &str, new: &str) {
    if let Some(r) = list.iter_mut().find(|r| r.nick.eq_ignore_ascii_case(old)) {
        r.nick = new.to_string();
    }
}

/// Drops an entry (`sig_nick_removed`).
pub fn last_msg_remove(list: &mut Vec<LastMsg>, nick: &str) {
    list.retain(|r| !r.nick.eq_ignore_ascii_case(nick));
}

fn is_alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// irssi's `isalnumhigh`: alphanumeric or a non-ASCII byte.
fn is_alnum_high(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || !c.is_ascii())
}

/// irssi's `nick_match_msg` (`core/nicklist.c`): does `msg` start by
/// addressing `nick` ("nick: hi", "n1ck, hi", "a,nick: hi")? A fuzzy
/// match (punctuation in the nick skipped) only counts when no other
/// nick in the channel is an exact match for the addressed word
/// (`other_exact`).
pub fn msg_addresses_nick(msg: &str, nick: &str, other_exact: impl Fn(&str) -> bool) -> bool {
    let m: Vec<char> = msg.chars().collect();
    let n: Vec<char> = nick.chars().collect();
    let len = n.len();
    if m.len() >= len
        && m[..len].iter().zip(&n).all(|(a, b)| a.eq_ignore_ascii_case(b))
        && !is_alnum_high(m.get(len).copied())
    {
        return true;
    }
    let mut msg_i = 0usize;
    loop {
        let mut nick_i = 0usize;
        let msgstart = msg_i;
        let mut fullmatch = true;
        while nick_i < n.len() && msg_i < m.len() {
            if n[nick_i].eq_ignore_ascii_case(&m[msg_i]) {
                msg_i += 1;
            } else if is_alnum(m[msg_i]) && !is_alnum(n[nick_i]) {
                fullmatch = false;
            } else {
                break;
            }
            nick_i += 1;
        }
        if msg_i != msgstart && !is_alnum_high(m.get(msg_i).copied()) {
            if nick_i < n.len() {
                fullmatch = false;
                while nick_i < n.len() && !is_alnum(n[nick_i]) {
                    nick_i += 1;
                }
            }
            if nick_i >= n.len() {
                return if fullmatch {
                    true
                } else {
                    let word: String = m[msgstart..msg_i].iter().collect();
                    !other_exact(&word)
                };
            }
        }
        // Not this recipient: skip to the next one in "nick1,nick2: text".
        while msg_i < m.len() && m[msg_i] != ' ' && m[msg_i] != ',' {
            msg_i += 1;
        }
        if m.get(msg_i) != Some(&',') {
            return false;
        }
        msg_i += 1;
    }
}

/// irssi's `only_nicks`: is the text before the word only earlier
/// completed nicks ("foo: bar:"), so the completion char may be appended?
pub fn only_nicks(linestart: &str, completion_char: &str) -> bool {
    if linestart.is_empty() {
        return true;
    }
    let mut cc_chars = completion_char.chars();
    let (Some(cc), None) = (cc_chars.next(), cc_chars.next()) else {
        return false;
    };
    if cc == ' ' {
        return false;
    }
    let b: Vec<char> = linestart.chars().collect();
    for i in 1..b.len() {
        if b[i] == ' ' && b[i - 1] != cc && b[i - 1] != ' ' {
            return false;
        }
    }
    for c in b.iter().rev() {
        if *c == ' ' {
            continue;
        }
        return *c == cc;
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchCase {
    Never,
    Always,
    Auto,
}

/// The `completion_*` settings this module reads, snapshotted once per Tab.
#[derive(Debug, Clone)]
pub struct Options {
    pub lowercase: bool,
    pub strict: bool,
    pub match_case: MatchCase,
}

impl Options {
    pub fn from_settings() -> Self {
        Self {
            lowercase: cfg::get_bool("completion_nicks_lowercase"),
            strict: cfg::get_bool("completion_strict"),
            match_case: match cfg::get_str("completion_nicks_match_case").as_str() {
                "never" => MatchCase::Never,
                "always" => MatchCase::Always,
                _ => MatchCase::Auto,
            },
        }
    }
}

fn starts_with(hay: &str, needle: &str, match_case: bool) -> bool {
    if match_case {
        hay.starts_with(needle)
    } else {
        hay.len() >= needle.len()
            && hay.is_char_boundary(needle.len())
            && hay[..needle.len()].eq_ignore_ascii_case(needle)
    }
}

fn contains_icase(list: &[String], s: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(s))
}

fn finish(nick: &str, suffix: Option<&str>, lowercase: bool) -> String {
    let s = format!("{nick}{}", suffix.unwrap_or(""));
    if lowercase {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

pub fn channel_nicks(
    nicks: &[String],
    own_nick: &str,
    last_msgs: &[LastMsg],
    word: &str,
    suffix: Option<&str>,
    opts: &Options,
) -> Vec<String> {
    if word.is_empty() {
        return Vec::new();
    }
    let suffix = suffix.filter(|s| !s.is_empty());
    let match_case = opts.match_case == MatchCase::Always
        || (opts.match_case == MatchCase::Auto && word.bytes().any(|b| b.is_ascii_uppercase()));

    let mut own_list = Vec::new();
    let mut list: Vec<String> = Vec::new();
    for rec in last_msgs {
        if starts_with(&rec.nick, word, match_case) {
            let known = if match_case { list.contains(&rec.nick) } else { contains_icase(&list, &rec.nick) };
            if !known {
                let s = finish(&rec.nick, suffix, opts.lowercase);
                if rec.own > 0 {
                    own_list.push(s);
                } else {
                    list.push(s);
                }
            }
        }
    }
    own_list.append(&mut list);
    let mut list = own_list;

    for nick in nicks {
        if starts_with(nick, word, match_case) && !nick.eq_ignore_ascii_case(own_nick) {
            let s = finish(nick, suffix, opts.lowercase);
            if !contains_icase(&list, &s) {
                list.push(s);
            }
        }
    }

    if !opts.strict {
        let mut loose: Vec<String> = Vec::new();
        for nick in nicks {
            let cleaned: String = nick.chars().filter(|c| is_alnum(*c)).collect();
            if starts_with(&cleaned, word, match_case) {
                let s = finish(nick, suffix, opts.lowercase);
                if !contains_icase(&loose, &s) {
                    loose.push(s);
                }
            }
        }
        list.extend(loose);
    }

    // `completion_joinlist`: keep the first of any case-insensitive duplicate.
    let mut out: Vec<String> = Vec::new();
    for s in list {
        if !contains_icase(&out, &s) {
            out.push(s);
        }
    }
    out
}

/// irssi's `completion_msg_server` for one list: entries whose nick
/// starts with `word` (everything when `word` is empty), most recent
/// first, each prefixed by `prefix + " "` when given.
pub fn msg_nicks(privates: &[LastMsg], word: &str, prefix: Option<&str>) -> Vec<String> {
    privates
        .iter()
        .filter(|r| word.is_empty() || starts_with(&r.nick, word, false))
        .map(|r| match prefix.filter(|p| !p.is_empty()) {
            Some(p) => format!("{p} {}", r.nick),
            None => r.nick.clone(),
        })
        .collect()
}

/// `completion_keep_word`: the typed word joins the end of the candidate
/// list so cycling comes back to what was typed.
pub fn with_kept_word(mut candidates: Vec<String>, word: &str) -> Vec<String> {
    if cfg::get_bool("completion_keep_word") {
        candidates.push(word.to_string());
    }
    candidates
}

/// Where the word being completed lives.
pub enum Source<'a> {
    Channel { nicks: &'a [String], own_nick: &'a str, last_msgs: &'a [LastMsg] },
    /// A query window: its one other party.
    Query { name: &'a str },
    Status,
}

pub struct TabInput<'a> {
    pub line: &'a str,
    pub cursor: usize,
    pub source: Source<'a>,
    /// Recent private-message partners (`/msg` completion).
    pub privates: &'a [LastMsg],
    pub cmdchars: &'a str,
    pub completion_char: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TabResult {
    /// Byte offset in the line where the completed word starts.
    pub word_start: usize,
    /// Replacement words to cycle through (the typed word last when
    /// `completion_keep_word` is on).
    pub candidates: Vec<String>,
}

fn is_msg_command(linestart: &str, cmdchars: &str) -> bool {
    let first = linestart.split(' ').next().unwrap_or("");
    let mut chars = first.chars();
    match chars.next() {
        Some(c) if cmdchars.contains(c) => {
            matches!(chars.as_str().to_ascii_lowercase().as_str(), "msg" | "query" | "action")
        }
        _ => false,
    }
}

/// irssi's `sig_complete_word` for the nick / `/msg` cases (channel
/// names and `/completion` words are resolved by the caller first).
/// `None` when there is nothing to offer.
pub fn tab_candidates(inp: &TabInput, opts: &Options, empty_line: bool) -> Option<TabResult> {
    let before = inp.line.get(..inp.cursor)?;
    let word_start = before.rfind(' ').map(|i| i + 1).unwrap_or(0);
    let word = &before[word_start..];
    let linestart = before[..word_start].trim_end_matches(' ');

    let mut found: Vec<String> = Vec::new();
    if linestart.is_empty() && word.is_empty() {
        // Tab on an empty line: offer `/msg <recent partner>`.
        if !empty_line {
            return None;
        }
        let prefix = format!("{}msg", inp.cmdchars.chars().next().unwrap_or('/'));
        found = msg_nicks(inp.privates, "", Some(&prefix));
        if found.is_empty() {
            found.push(prefix);
        }
    } else if is_msg_command(linestart, inp.cmdchars) {
        found = msg_nicks(inp.privates, word, None);
        if let Source::Channel { nicks, own_nick, last_msgs } = &inp.source {
            for n in channel_nicks(nicks, own_nick, last_msgs, word, None, opts) {
                if !contains_icase(&found, &n) {
                    found.push(n);
                }
            }
        }
    } else {
        match &inp.source {
            Source::Channel { nicks, own_nick, last_msgs } => {
                let suffix = only_nicks(linestart, inp.completion_char).then_some(inp.completion_char);
                found = channel_nicks(nicks, own_nick, last_msgs, word, suffix, opts);
            }
            Source::Query { name } => {
                if !word.is_empty() && starts_with(name, word, false) {
                    found.push((*name).to_string());
                }
            }
            Source::Status => {}
        }
    }
    if found.is_empty() {
        return None;
    }
    Some(TabResult { word_start, candidates: with_kept_word(found, word) })
}

/// irssi's `auto_complete` (`completion_auto`): when the text to send
/// starts with `<something><completion_char>` and `<something>` is not
/// an exact nick, expand it to the first completion. `None` leaves the
/// line as typed.
pub fn auto_complete(
    line: &str,
    completion_char: &str,
    nicks: &[String],
    own_nick: &str,
    last_msgs: &[LastMsg],
    opts: &Options,
) -> Option<String> {
    let at = line.find(completion_char)?;
    let nick = &line[..at];
    if nick.is_empty() || nicks.iter().any(|n| n.eq_ignore_ascii_case(nick)) {
        return None;
    }
    let comp = channel_nicks(nicks, own_nick, last_msgs, nick, None, opts);
    comp.first().map(|first| format!("{first}{}", &line[at..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options { lowercase: false, strict: false, match_case: MatchCase::Auto }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn last_msg_keeps_newest_first_and_caps() {
        let mut l = Vec::new();
        for n in ["a", "b", "c"] {
            last_msg_add(&mut l, n, false, 2);
        }
        assert_eq!(l.iter().map(|r| r.nick.as_str()).collect::<Vec<_>>(), ["c", "b"]);
        last_msg_add(&mut l, "B", false, 2);
        assert_eq!(l[0].nick, "B", "an existing nick moves to the front, not duplicated");
        assert_eq!(l.len(), 2);
        last_msg_add(&mut l, "x", false, 0);
        assert_eq!(l.len(), 2, "max 0 stores nothing");
    }

    #[test]
    fn own_messages_sort_ahead_and_age_out() {
        let mut l = Vec::new();
        last_msg_add(&mut l, "friend", true, 5);
        last_msg_add(&mut l, "zed", false, 5);
        let got = channel_nicks(&v(&["friend", "zed", "zoe"]), "me", &l, "z", None, &opts());
        assert_eq!(got[0], "zed");
        let got = channel_nicks(&v(&["fa", "fb"]), "me", &{
            let mut m = Vec::new();
            last_msg_add(&mut m, "fa", false, 5);
            last_msg_add(&mut m, "fb", true, 5);
            last_msg_add(&mut m, "fa", false, 5);
            m
        }, "f", None, &opts());
        assert_eq!(got[0], "fb", "an 'own' nick outranks a more recent plain one");
    }

    #[test]
    fn recent_speakers_come_before_the_rest_and_own_nick_is_excluded() {
        let mut l = Vec::new();
        last_msg_add(&mut l, "bob2", false, 50);
        let nicks = v(&["bob1", "bob2", "bob3", "me"]);
        let got = channel_nicks(&nicks, "me", &l, "bob", None, &opts());
        assert_eq!(got, ["bob2", "bob1", "bob3"]);
        let strict = Options { strict: true, ..opts() };
        assert!(channel_nicks(&nicks, "me", &l, "m", None, &strict).is_empty(), "the exact-prefix pass skips your own nick");
        // irssi's non-strict pass (`completion_nicks_nonstrict`) has no
        // own-nick check, so the loose pass can still offer it.
        assert_eq!(channel_nicks(&nicks, "me", &l, "m", None, &opts()), ["me"]);
    }

    #[test]
    fn suffix_lowercase_and_case_matching() {
        let nicks = v(&["Alice", "alfred"]);
        let mut o = opts();
        assert_eq!(channel_nicks(&nicks, "me", &[], "al", Some(":"), &o), ["Alice:", "alfred:"]);
        assert_eq!(channel_nicks(&nicks, "me", &[], "Al", Some(":"), &o), ["Alice:"], "auto: an uppercase letter turns case matching on");
        o.match_case = MatchCase::Never;
        assert_eq!(channel_nicks(&nicks, "me", &[], "Al", None, &o).len(), 2);
        o.match_case = MatchCase::Always;
        assert_eq!(channel_nicks(&nicks, "me", &[], "al", None, &o), ["alfred"]);
        o.match_case = MatchCase::Auto;
        o.lowercase = true;
        assert_eq!(channel_nicks(&nicks, "me", &[], "Al", Some(":"), &o), ["alice:"]);
    }

    #[test]
    fn non_strict_finds_nicks_by_their_alphanumeric_part() {
        let nicks = v(&["_foo_", "bar"]);
        let mut o = opts();
        assert_eq!(channel_nicks(&nicks, "me", &[], "foo", None, &o), ["_foo_"]);
        o.strict = true;
        assert!(channel_nicks(&nicks, "me", &[], "foo", None, &o).is_empty());
    }

    #[test]
    fn empty_word_completes_nothing() {
        assert!(channel_nicks(&v(&["a"]), "me", &[], "", None, &opts()).is_empty());
    }

    #[test]
    fn msg_nicks_list_privates_newest_first_with_optional_prefix() {
        let mut p = Vec::new();
        last_msg_add(&mut p, "alice", true, 10);
        last_msg_add(&mut p, "bob", true, 10);
        assert_eq!(msg_nicks(&p, "", Some("/msg")), ["/msg bob", "/msg alice"]);
        assert_eq!(msg_nicks(&p, "a", None), ["alice"]);
    }

    #[test]
    fn only_nicks_allows_the_completion_char_only_in_a_nick_run() {
        assert!(only_nicks("", ":"));
        assert!(only_nicks("foo:", ":"));
        assert!(only_nicks("foo: bar:", ":"));
        assert!(!only_nicks("hello there", ":"));
        assert!(!only_nicks("foo bar", ":"));
        assert!(!only_nicks("foo:", ", "), "a multi-character completion_char never continues a run");
        assert!(!only_nicks("foo:", " "));
    }

    #[test]
    fn nick_match_msg_matches_addressing() {
        let none = |_: &str| false;
        assert!(msg_addresses_nick("abcX: hello", "abcX", none));
        assert!(msg_addresses_nick("abcx, hello", "abcX", none));
        assert!(!msg_addresses_nick("abcXtra: hello", "abcX", none));
        assert!(!msg_addresses_nick("hello abcX", "abcX", none), "only at the start");
        assert!(msg_addresses_nick("foo: hi", "_foo_", none), "fuzzy: punctuation in the nick is skipped");
        assert!(!msg_addresses_nick("foo: hi", "_foo_", |w| w == "foo"), "unless someone is exactly 'foo'");
        assert!(msg_addresses_nick("bob,abcX: hi", "abcX", none), "one of several recipients");
    }

    fn input<'a>(line: &'a str, nicks: &'a [String], privates: &'a [LastMsg]) -> TabInput<'a> {
        TabInput {
            line,
            cursor: line.len(),
            source: Source::Channel { nicks, own_nick: "me", last_msgs: &[] },
            privates,
            cmdchars: "/",
            completion_char: ":",
        }
    }

    #[test]
    fn tab_on_a_first_word_appends_the_completion_char_and_keeps_the_typed_word() {
        let nicks = v(&["alice", "bob"]);
        let r = tab_candidates(&input("ali", &nicks, &[]), &opts(), true).unwrap();
        assert_eq!(r.word_start, 0);
        assert_eq!(r.candidates, ["alice:", "ali"], "keep_word puts the typed word last");
        // Later in a sentence: no completion char.
        let r = tab_candidates(&input("hi ali", &nicks, &[]), &opts(), true).unwrap();
        assert_eq!(r.word_start, 3);
        assert_eq!(r.candidates[0], "alice");
        // After a finished "nick:" the next nick still takes one.
        let r = tab_candidates(&input("alice: bo", &nicks, &[]), &opts(), true).unwrap();
        assert_eq!(r.candidates[0], "bob:");
    }

    #[test]
    fn tab_on_an_empty_line_offers_msg_with_recent_partners_when_enabled() {
        let mut p = Vec::new();
        last_msg_add(&mut p, "alice", true, 10);
        last_msg_add(&mut p, "bob", true, 10);
        let r = tab_candidates(&input("", &[], &p), &opts(), true).unwrap();
        assert_eq!(r.candidates[0], "/msg bob");
        assert_eq!(r.candidates[1], "/msg alice");
        assert!(tab_candidates(&input("", &[], &p), &opts(), false).is_none(), "completion_empty_line off");
        let r = tab_candidates(&input("", &[], &[]), &opts(), true).unwrap();
        assert_eq!(r.candidates[0], "/msg", "no partners yet: just /msg");
    }

    #[test]
    fn msg_command_completes_partners_then_channel_nicks() {
        let mut p = Vec::new();
        last_msg_add(&mut p, "zed", true, 10);
        let nicks = v(&["zoe"]);
        let r = tab_candidates(&input("/msg z", &nicks, &p), &opts(), true).unwrap();
        assert_eq!(r.candidates[..2], ["zed", "zoe"]);
        let r = tab_candidates(&input("/msg ", &nicks, &p), &opts(), true).unwrap();
        assert_eq!(r.word_start, 5);
        assert_eq!(r.candidates[0], "zed", "empty word after /msg lists partners");
    }

    #[test]
    fn query_completes_the_other_partys_name_only() {
        let mut i = input("bo", &[], &[]);
        i.source = Source::Query { name: "Bobby" };
        assert_eq!(tab_candidates(&i, &opts(), true).unwrap().candidates[0], "Bobby");
        i.line = "zz";
        i.cursor = 2;
        assert!(tab_candidates(&i, &opts(), true).is_none());
    }

    #[test]
    fn auto_complete_expands_a_partial_nick_before_the_completion_char() {
        let nicks = v(&["alice", "bob"]);
        assert_eq!(auto_complete("ali: hi", ":", &nicks, "me", &[], &opts()), Some("alice: hi".to_string()));
        assert_eq!(auto_complete("alice: hi", ":", &nicks, "me", &[], &opts()), None, "exact nick: leave it");
        assert_eq!(auto_complete("nobody: hi", ":", &nicks, "me", &[], &opts()), None);
        assert_eq!(auto_complete("no colon here", ":", &nicks, "me", &[], &opts()), None);
    }
}
