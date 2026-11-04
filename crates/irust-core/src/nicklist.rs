use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixTable(Vec<(char, char)>);

impl PrefixTable {
    pub fn standard() -> Self {
        PrefixTable(vec![('q', '~'), ('a', '&'), ('o', '@'), ('h', '%'), ('v', '+')])
    }

    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.strip_prefix('(')?;
        let (letters, symbols) = rest.split_once(')')?;
        let letters: Vec<char> = letters.chars().collect();
        let symbols: Vec<char> = symbols.chars().collect();
        if letters.is_empty() || letters.len() != symbols.len() {
            return None;
        }
        let pairs: Vec<(char, char)> = letters.into_iter().zip(symbols).take(8).collect();
        Some(PrefixTable(pairs))
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn position_of_symbol(&self, symbol: char) -> Option<usize> {
        self.0.iter().position(|&(_, s)| s == symbol)
    }

    fn symbol_at(&self, index: usize) -> Option<char> {
        self.0.get(index).map(|&(_, s)| s)
    }

    fn has_letter(&self, letter: char) -> bool {
        self.0.iter().any(|&(l, _)| l == letter)
    }

    pub fn is_symbol(&self, c: char) -> bool {
        self.0.iter().any(|&(_, s)| s == c)
    }

    fn letter_to_symbol(&self, letter: char) -> Option<char> {
        self.0.iter().find(|&&(l, _)| l == letter).map(|&(_, s)| s)
    }

    fn position_of_letter(&self, letter: char) -> Option<usize> {
        self.0.iter().position(|&(l, _)| l == letter)
    }
}

impl Default for PrefixTable {
    fn default() -> Self {
        Self::standard()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChanModesTable {
    type_a: String,
    type_b: String,
    type_c: String,
}

impl ChanModesTable {
    /// What solanum -- and virtually every real ircd -- actually
    /// reports (`CHANMODES=beI,k,l,imnpst`); the default until a real
    /// one is learned.
    pub fn standard() -> Self {
        ChanModesTable {
            type_a: "beI".to_string(),
            type_b: "k".to_string(),
            type_c: "l".to_string(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let mut groups = value.split(',');
        let type_a = groups.next()?.to_string();
        let type_b = groups.next()?.to_string();
        let type_c = groups.next()?.to_string();
        let _type_d = groups.next()?;
        if groups.next().is_some() {
            return None;
        }
        Some(ChanModesTable { type_a, type_b, type_c })
    }

    fn takes_param(&self, letter: char, adding: bool) -> bool {
        self.type_a.contains(letter) || self.type_b.contains(letter) || (adding && self.type_c.contains(letter))
    }
}

impl Default for ChanModesTable {
    fn default() -> Self {
        Self::standard()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank(u8);

pub struct Entry {
    pub nick: String,
    pub rank: Rank,
    pub away: bool,
}

#[derive(Clone, Default)]
struct NickInfo {
    held: u8,
    away: bool,
    user: Option<String>,
    host: Option<String>,
}

#[derive(Default)]
pub struct Nicklist {
    entries: BTreeMap<String, NickInfo>,
    prefix_table: PrefixTable,
    chanmodes_table: ChanModesTable,
}

impl Nicklist {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn status_counts(&self) -> (usize, usize, usize, usize) {
        let op_idx = self.prefix_table.position_of_letter('o');
        let halfop_idx = self.prefix_table.position_of_letter('h');
        let voice_idx = self.prefix_table.position_of_letter('v');
        let (mut ops, mut halfops, mut voices, mut normal) = (0, 0, 0, 0);
        for info in self.entries.values() {
            let idx = self.rank_from_held(info.held).0 as usize;
            if Some(idx) == op_idx {
                ops += 1;
            } else if Some(idx) == halfop_idx {
                halfops += 1;
            } else if Some(idx) == voice_idx {
                voices += 1;
            } else {
                normal += 1;
            }
        }
        (ops, halfops, voices, normal)
    }

    pub fn contains(&self, nick: &str) -> bool {
        self.entries.contains_key(nick)
    }

    pub fn set_prefix_table(&mut self, table: PrefixTable) {
        self.prefix_table = table;
    }

    /// Learns the server's real `CHANMODES=` scheme -- same
    /// replace-outright treatment as `set_prefix_table`.
    pub fn set_chanmodes_table(&mut self, table: ChanModesTable) {
        self.chanmodes_table = table;
    }

    /// The display `Rank` for a bitmask of held prefix modes -- highest
    /// bit set wins, "no rank" (one past the last real index) if none
    /// are.
    fn rank_from_held(&self, held: u8) -> Rank {
        (0..self.prefix_table.len())
            .find(|i| held & (1 << i) != 0)
            .map_or(Rank(self.prefix_table.len() as u8), |i| Rank(i as u8))
    }

    pub fn add_from_names_token(&mut self, token: &str) {
        let mut rest = token;
        let mut held = 0u8;
        while let Some(c) = rest.chars().next() {
            let Some(pos) = self.prefix_table.position_of_symbol(c) else {
                break;
            };
            held |= 1 << pos;
            rest = &rest[c.len_utf8()..];
        }
        if rest.is_empty() {
            return;
        }
        let (nick, userhost) = match rest.split_once('!') {
            Some((nick, userhost)) => (nick, Some(userhost)),
            None => (rest, None),
        };
        self.entries.insert(
            nick.to_string(),
            NickInfo {
                held,
                ..Default::default()
            },
        );
        if let Some(userhost) = userhost {
            let (user, host) = userhost.split_once('@').unwrap_or((userhost, ""));
            self.set_userhost(nick, Some(user), Some(host).filter(|h| !h.is_empty()));
        }
    }

    pub fn insert_plain(&mut self, nick: &str) {
        self.entries.entry(nick.to_string()).or_default();
    }

    pub fn remove(&mut self, nick: &str) {
        self.entries.remove(nick);
    }

    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(entry) = self.entries.remove(old) {
            self.entries.insert(new.to_string(), entry);
        }
    }

    /// Sets a nick's away status -- fed by IRCv3 `away-notify` (real-time
    /// changes) and by `RPL_WHOREPLY`'s H/G flag (initial state right
    /// after joining). A no-op if the nick isn't in this channel.
    pub fn set_away(&mut self, nick: &str, away: bool) {
        if let Some(entry) = self.entries.get_mut(nick) {
            entry.away = away;
        }
    }

    pub fn set_userhost(&mut self, nick: &str, user: Option<&str>, host: Option<&str>) {
        let (Some(user), Some(host)) = (user, host) else { return };
        if let Some(entry) = self.entries.get_mut(nick) {
            entry.user = Some(user.to_string());
            entry.host = Some(host.to_string());
        }
    }

    pub fn apply_prefix_mode(&mut self, nick: &str, prefix_char: char, added: bool) {
        let Some(pos) = self.prefix_table.position_of_symbol(prefix_char) else {
            return;
        };
        let Some(existing) = self.entries.get_mut(nick) else {
            return;
        };
        let bit = 1u8 << pos;
        if added {
            existing.held |= bit;
        } else {
            existing.held &= !bit;
        }
    }

    /// Sorted for display: highest rank first, then alphabetically
    /// (case-insensitive).
    pub fn sorted(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> = self
            .entries
            .iter()
            .map(|(n, info)| Entry {
                nick: n.clone(),
                rank: self.rank_from_held(info.held),
                away: info.away,
            })
            .collect();
        v.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| a.nick.to_lowercase().cmp(&b.nick.to_lowercase()))
        });
        v
    }

    pub fn prefix_char(&self, rank: Rank) -> Option<char> {
        self.prefix_table.symbol_at(rank.0 as usize)
    }

    pub fn ban_mask_for(&self, nick: &str) -> Option<String> {
        let entry = self.entries.get(nick)?;
        let (user, host) = (entry.user.as_deref()?, entry.host.as_deref()?);
        Some(crate::bantype::mask(nick, user, host, &irust_config::settings_net::str_of("ban_type")))
    }

    /// `nick`'s known host (cloak), if we've seen its full prefix yet --
    /// e.g. `/ban *`'s bot/staff exclusion needs the raw host, not a
    /// built ban mask.
    pub fn host_for(&self, nick: &str) -> Option<&str> {
        self.entries.get(nick)?.host.as_deref()
    }

    pub fn prefix_for(&self, nick: &str) -> Option<char> {
        let rank = self.rank_from_held(self.entries.get(nick)?.held);
        self.prefix_char(rank)
    }
}

/// Strips a leading ident-flag char (`^~+=-`, irssi's `ishostflag`) off
/// `user`, then generalizes `host` to a domain wildcard the same way
/// irssi's default `ban_type` ("normal") does -- see `domain_mask`.
/// Ported from `irc_get_mask()`/`BAN_TYPE_NORMAL` in irssi's
/// `irc-masks.c`/`bans.c`.
#[cfg(test)]
fn ban_mask(user: &str, host: &str) -> String {
    let user = user.strip_prefix(['^', '~', '+', '=', '-']).unwrap_or(user);
    format!("*!*{user}@{}", domain_mask(host))
}

pub(crate) fn domain_mask(host: &str) -> String {
    if !host.contains('.') {
        return host.to_string();
    }
    let labels: Vec<&str> = host.split('.').collect();
    let is_ipv4 = labels.len() == 4
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()));
    if is_ipv4 {
        let mut labels = labels;
        let last = labels.len() - 1;
        labels[last] = "*";
        return labels.join(".");
    }
    if let Some(first_dot) = host.find('.') {
        if host[first_dot + 1..].contains('.') {
            return format!("*{}", &host[first_dot..]);
        }
    }
    host.to_string()
}

pub fn mode_takes_param(
    letter: char,
    adding: bool,
    prefix_table: &PrefixTable,
    chanmodes_table: &ChanModesTable,
) -> bool {
    prefix_table.has_letter(letter) || chanmodes_table.takes_param(letter, adding)
}

/// `true` if `text` matches `pattern`'s `*`/`?` glob (`*` = any run of
/// characters, `?` = exactly one), case-insensitive -- irssi's own
/// wildcard-nick matching (`nicklist_find_multiple`) for `/op *`-style
/// bulk mode commands. Anchored: the whole nick must match, not a
/// substring.
pub fn matches_glob(pattern: &str, text: &str) -> bool {
    fn go(pattern: &[char], text: &[char]) -> bool {
        match pattern.first() {
            None => text.is_empty(),
            Some('*') => {
                // Skip runs of consecutive '*' cheaply, then try
                // matching the rest against every possible split point.
                let rest = &pattern[1..];
                if rest.is_empty() {
                    return true;
                }
                (0..=text.len()).any(|i| go(rest, &text[i..]))
            }
            Some('?') => !text.is_empty() && go(&pattern[1..], &text[1..]),
            Some(c) => text.first().is_some_and(|t| t.eq_ignore_ascii_case(c)) && go(&pattern[1..], &text[1..]),
        }
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    go(&pattern, &text)
}

pub fn matches_regex_cached(pattern: &str, text: &str) -> bool {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<regex::Regex>>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut cache = cache.lock().unwrap();
    let compiled = cache
        .entry(pattern.to_string())
        .or_insert_with(|| regex::Regex::new(pattern).ok());
    compiled.as_ref().is_some_and(|re| re.is_match(text))
}

pub fn is_valid_regex(pattern: &str) -> bool {
    regex::Regex::new(pattern).is_ok()
}

pub fn wildcard_targets(
    nicklist: &Nicklist,
    pattern: &str,
    own_nick: &str,
    want_op: Option<bool>,
    want_voice: Option<bool>,
) -> Vec<String> {
    let op_rank = nicklist.prefix_table.position_of_symbol('@').map(|i| Rank(i as u8));
    let voice_rank = nicklist.prefix_table.position_of_symbol('+').map(|i| Rank(i as u8));
    nicklist
        .sorted()
        .into_iter()
        .filter(|e| matches_glob(pattern, &e.nick))
        .filter(|e| !e.nick.eq_ignore_ascii_case(own_nick))
        .filter(|e| {
            let is_op = op_rank.is_some_and(|r| e.rank <= r);
            want_op.is_none_or(|want| want == is_op)
        })
        .filter(|e| {
            let is_voice = voice_rank.is_some_and(|r| e.rank == r);
            want_voice.is_none_or(|want| want == is_voice)
        })
        .map(|e| e.nick)
        .collect()
}

pub fn mode_letter_to_prefix(letter: char, prefix_table: &PrefixTable) -> Option<char> {
    prefix_table.letter_to_symbol(letter)
}

pub fn extract_ban_changes(modestring: &str, args: &[String], nicklist: &Nicklist) -> Vec<(bool, String)> {
    let mut changes = Vec::new();
    let mut adding = true;
    let mut arg_iter = args.iter();
    for ch in modestring.chars() {
        match ch {
            '+' => adding = true,
            '-' => adding = false,
            letter => {
                if mode_takes_param(letter, adding, &nicklist.prefix_table, &nicklist.chanmodes_table) {
                    if let Some(arg) = arg_iter.next() {
                        if letter == 'b' {
                            changes.push((adding, arg.clone()));
                        }
                    }
                }
            }
        }
    }
    changes
}

pub fn apply_mode_line(nicklist: &mut Nicklist, modestring: &str, args: &[String]) {
    let prefix_table = nicklist.prefix_table.clone();
    let chanmodes_table = nicklist.chanmodes_table.clone();
    let mut adding = true;
    let mut arg_iter = args.iter();
    for ch in modestring.chars() {
        match ch {
            '+' => adding = true,
            '-' => adding = false,
            letter => {
                if mode_takes_param(letter, adding, &prefix_table, &chanmodes_table) {
                    if let Some(arg) = arg_iter.next() {
                        if let Some(prefix) = mode_letter_to_prefix(letter, &prefix_table) {
                            nicklist.apply_prefix_mode(arg, prefix, adding);
                        }
                    }
                }
            }
        }
    }
}

fn parse_modestring(current: &str, chanmodes_table: &ChanModesTable) -> Vec<(char, Option<String>)> {
    let mut parts = current.split(' ');
    let letters = parts.next().unwrap_or("");
    let mut args = parts.filter(|p| !p.is_empty());
    letters
        .chars()
        .map(|letter| {
            let has_arg = chanmodes_table.type_b.contains(letter) || chanmodes_table.type_c.contains(letter);
            let arg = if has_arg { args.next().map(str::to_string) } else { None };
            (letter, arg)
        })
        .collect()
}

/// Replaces (if adding) or drops (if removing) `letter`'s own entry --
/// shared by every branch of [`apply_channel_modestring`] that decided
/// the letter belongs in the modestring at all.
fn set_channel_mode(modes: &mut Vec<(char, Option<String>)>, letter: char, adding: bool, arg: Option<String>) {
    modes.retain(|&(l, _)| l != letter);
    if adding {
        modes.push((letter, arg));
    }
}

pub fn apply_channel_modestring(current: &str, modestring: &str, args: &[String], nicklist: &Nicklist) -> String {
    let prefix_table = &nicklist.prefix_table;
    let chanmodes_table = &nicklist.chanmodes_table;
    let mut modes = parse_modestring(current, chanmodes_table);
    let mut adding = true;
    let mut arg_iter = args.iter();
    for ch in modestring.chars() {
        match ch {
            '+' => adding = true,
            '-' => adding = false,
            letter => {
                let consumed = if mode_takes_param(letter, adding, prefix_table, chanmodes_table) {
                    arg_iter.next().cloned()
                } else {
                    None
                };
                if prefix_table.has_letter(letter) || chanmodes_table.type_a.contains(letter) {
                    continue;
                }
                set_channel_mode(&mut modes, letter, adding, consumed);
            }
        }
    }

    modes.sort_by_key(|&(letter, _)| letter);
    let letters: String = modes.iter().map(|&(l, _)| l).collect();
    let arglist: Vec<&str> = modes.iter().filter_map(|(_, a)| a.as_deref()).collect();
    if arglist.is_empty() {
        letters
    } else {
        format!("{letters} {}", arglist.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_star_and_question_mark() {
        assert!(matches_glob("*", "anything"));
        assert!(matches_glob("jo*n", "jordan"));
        assert!(matches_glob("jo*n", "join"));
        assert!(!matches_glob("jo*n", "joe"));
        assert!(matches_glob("bo?", "bob"));
        assert!(!matches_glob("bo?", "bobby"));
        assert!(matches_glob("BOB", "bob"), "case-insensitive");
        assert!(!matches_glob("bob", "bobby"), "anchored, not a substring match");
    }

    #[test]
    fn regex_matches_a_real_pattern_against_a_hostmask() {
        assert!(matches_regex_cached(
            r"^Bad.*!.*@spammer\.example$",
            "BadNick!user@spammer.example"
        ));
        assert!(!matches_regex_cached(
            r"^Bad.*!.*@spammer\.example$",
            "GoodNick!user@spammer.example"
        ));
    }

    #[test]
    fn regex_matching_is_not_anchored_by_default_unlike_glob() {
        assert!(matches_regex_cached("Bad", "SomeBadNick"));
    }

    #[test]
    fn an_invalid_regex_never_matches_instead_of_panicking() {
        assert!(!matches_regex_cached("(unterminated", "anything"));
    }

    #[test]
    fn is_valid_regex_accepts_real_patterns_and_rejects_broken_ones() {
        assert!(is_valid_regex(r"^Bad.*\d+$"));
        assert!(!is_valid_regex("(unterminated"));
    }

    fn nicklist_with(entries: &[(&str, Option<char>)]) -> Nicklist {
        let mut nl = Nicklist::default();
        for (nick, prefix) in entries {
            match prefix {
                Some(p) => nl.add_from_names_token(&format!("{p}{nick}")),
                None => nl.add_from_names_token(nick),
            }
        }
        nl
    }

    /// Backs `BufferLine::rank` (see `Buffer::push_chat`'s callers in
    /// `client.rs`/`main.rs`) -- a chat line's rank prefix comes straight
    /// from this lookup at arrival time.
    #[test]
    fn prefix_for_reflects_current_held_rank() {
        let nl = nicklist_with(&[("abcX", Some('@')), ("voiced", Some('+')), ("plain", None)]);
        assert_eq!(nl.prefix_for("abcX"), Some('@'));
        assert_eq!(nl.prefix_for("voiced"), Some('+'));
        assert_eq!(nl.prefix_for("plain"), None);
        assert_eq!(nl.prefix_for("not_in_channel"), None);
    }

    #[test]
    fn wildcard_targets_for_op_skips_already_opped_and_self() {
        let nl = nicklist_with(&[("abcX", None), ("already_op", Some('@')), ("plain", None)]);
        let targets = wildcard_targets(&nl, "*", "abcX", Some(false), None);
        assert_eq!(targets, vec!["plain".to_string()]);
    }

    #[test]
    fn wildcard_targets_for_deop_only_matches_current_ops() {
        let nl = nicklist_with(&[("abcX", None), ("an_op", Some('@')), ("plain", None)]);
        let targets = wildcard_targets(&nl, "*", "abcX", Some(true), None);
        assert_eq!(targets, vec!["an_op".to_string()]);
    }

    #[test]
    fn wildcard_targets_for_voice_skips_ops_and_already_voiced() {
        let nl = nicklist_with(&[
            ("abcX", None),
            ("an_op", Some('@')),
            ("voiced", Some('+')),
            ("plain", None),
        ]);
        let targets = wildcard_targets(&nl, "*", "abcX", Some(false), Some(false));
        assert_eq!(targets, vec!["plain".to_string()]);
    }

    #[test]
    fn wildcard_targets_for_devoice_only_matches_currently_voiced() {
        let nl = nicklist_with(&[("abcX", None), ("voiced", Some('+')), ("plain", None)]);
        let targets = wildcard_targets(&nl, "*", "abcX", None, Some(true));
        assert_eq!(targets, vec!["voiced".to_string()]);
    }

    #[test]
    fn domain_mask_strips_the_leftmost_label_of_a_three_plus_label_host() {
        assert_eq!(domain_mask("dyn123.blah.net"), "*.blah.net");
        assert_eq!(domain_mask("k9.example.net"), "*.example.net");
    }

    #[test]
    fn domain_mask_leaves_a_bare_two_label_domain_unchanged() {
        assert_eq!(domain_mask("example.net"), "example.net");
    }

    #[test]
    fn domain_mask_wildcards_the_last_octet_of_an_ipv4_address() {
        assert_eq!(domain_mask("192.168.1.42"), "192.168.1.*");
    }

    #[test]
    fn domain_mask_leaves_a_dotless_host_like_a_cloak_unchanged() {
        assert_eq!(domain_mask("chatnet/staff/sh"), "chatnet/staff/sh");
    }

    #[test]
    fn ban_mask_strips_an_ident_flag_and_wildcards_the_user_and_domain() {
        assert_eq!(ban_mask("~abcX", "dyn123.blah.net"), "*!*abcX@*.blah.net");
        assert_eq!(ban_mask("abcX", "chatnet/staff/sh"), "*!*abcX@chatnet/staff/sh");
    }

    #[test]
    fn ban_mask_for_is_none_until_a_full_prefix_has_been_seen() {
        let mut nl = Nicklist::default();
        nl.insert_plain("SomeNick");
        assert_eq!(nl.ban_mask_for("SomeNick"), None);

        nl.set_userhost("SomeNick", Some("~someuser"), Some("dyn1.example.net"));
        assert_eq!(
            nl.ban_mask_for("SomeNick"),
            Some("*!*someuser@*.example.net".to_string())
        );
    }

    #[test]
    fn set_userhost_on_an_unknown_nick_is_a_no_op() {
        let mut nl = Nicklist::default();
        nl.set_userhost("Ghost", Some("user"), Some("host.example.com"));
        assert_eq!(nl.ban_mask_for("Ghost"), None);
    }

    #[test]
    fn names_tokens_parse_prefix_and_plain() {
        let mut nl = Nicklist::default();
        nl.add_from_names_token("@Op");
        nl.add_from_names_token("+Voiced");
        nl.add_from_names_token("Plain");

        let sorted = nl.sorted();
        let names: Vec<&str> = sorted.iter().map(|e| e.nick.as_str()).collect();
        assert_eq!(names, vec!["Op", "Voiced", "Plain"]);
    }

    #[test]
    fn names_token_with_multiple_prefix_chars_strips_all_of_them() {
        let mut nl = Nicklist::default();
        nl.add_from_names_token("@+BothOpAndVoice");

        assert_eq!(nl.prefix_for("BothOpAndVoice"), Some('@'), "highest held rank shown");
        assert_eq!(
            nl.sorted()[0].nick,
            "BothOpAndVoice",
            "no leftover + baked into the nick"
        );
    }

    /// `userhost-in-names` cap: a NAMES token may be `nick!user@host`
    /// instead of a bare nick -- pre-populates the host cache right
    /// away, same effect `set_userhost` (fed by `WHO`/JOIN) already has.
    #[test]
    fn names_token_with_userhost_populates_the_host_cache() {
        let mut nl = Nicklist::default();
        nl.add_from_names_token("Plain!~user@some.host.example");

        assert_eq!(nl.sorted()[0].nick, "Plain");
        assert_eq!(nl.host_for("Plain"), Some("some.host.example"));
    }

    #[test]
    fn names_token_combines_multi_prefix_and_userhost() {
        let mut nl = Nicklist::default();
        nl.add_from_names_token("@+Op!~user@host.example");

        assert_eq!(nl.prefix_for("Op"), Some('@'));
        assert_eq!(nl.host_for("Op"), Some("host.example"));
    }

    #[test]
    fn rank_ordering_puts_higher_privilege_first_then_alpha() {
        let mut nl = Nicklist::default();
        nl.add_from_names_token("zeta");
        nl.add_from_names_token("@alpha");
        nl.add_from_names_token("beta");

        let sorted = nl.sorted();
        let names: Vec<&str> = sorted.iter().map(|e| e.nick.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "zeta"]);
    }

    #[test]
    fn mode_promotes_and_demotes() {
        let mut nl = Nicklist::default();
        nl.insert_plain("Nick");

        apply_mode_line(&mut nl, "+o", &["Nick".to_string()]);
        assert_eq!(nl.sorted()[0].nick, "Nick");
        assert_eq!(nl.prefix_char(nl.sorted()[0].rank), Some('@'));

        apply_mode_line(&mut nl, "-o", &["Nick".to_string()]);
        assert_eq!(nl.prefix_char(nl.sorted()[0].rank), None);
    }

    #[test]
    fn removing_op_falls_back_to_a_still_held_voice() {
        let mut nl = Nicklist::default();
        nl.insert_plain("Nick");

        apply_mode_line(&mut nl, "+v", &["Nick".to_string()]);
        apply_mode_line(&mut nl, "+o-v", &["Nick".to_string(), "Nick".to_string()]);
        assert_eq!(nl.prefix_char(nl.sorted()[0].rank), Some('@'));

        apply_mode_line(&mut nl, "+v-o", &["Nick".to_string(), "Nick".to_string()]);
        assert_eq!(nl.prefix_char(nl.sorted()[0].rank), Some('+'));
    }

    #[test]
    fn mode_line_correctly_skips_non_prefix_args() {
        let mut nl = Nicklist::default();
        nl.insert_plain("Alice");
        nl.insert_plain("Bob");

        // ban mask arg must not get mistaken for a nick to promote/demote
        apply_mode_line(
            &mut nl,
            "+ob-v",
            &[
                "Alice".to_string(),
                "*!*@spammer.example".to_string(),
                "Bob".to_string(),
            ],
        );

        let sorted = nl.sorted();
        let alice = sorted.iter().find(|e| e.nick == "Alice").unwrap();
        let bob = sorted.iter().find(|e| e.nick == "Bob").unwrap();
        assert_eq!(nl.prefix_char(alice.rank), Some('@'));
        assert_eq!(nl.prefix_char(bob.rank), None);
    }

    #[test]
    fn extract_ban_changes_finds_the_ban_and_ignores_prefix_letters() {
        let nl = Nicklist::default();
        // Same "+ob-v" shape as the sibling test above -- the ban mask
        // must be picked out correctly even sandwiched between two
        // prefix-mode letters that also consume an arg.
        let changes = extract_ban_changes(
            "+ob-v",
            &[
                "Alice".to_string(),
                "*!*@spammer.example".to_string(),
                "Bob".to_string(),
            ],
            &nl,
        );
        assert_eq!(changes, vec![(true, "*!*@spammer.example".to_string())]);
    }

    #[test]
    fn extract_ban_changes_reports_removal_too() {
        let nl = Nicklist::default();
        let changes = extract_ban_changes("-b", &["*!*@spammer.example".to_string()], &nl);
        assert_eq!(changes, vec![(false, "*!*@spammer.example".to_string())]);
    }

    #[test]
    fn extract_ban_changes_empty_when_no_ban_letter_present() {
        let nl = Nicklist::default();
        let changes = extract_ban_changes("+o", &["Alice".to_string()], &nl);
        assert!(changes.is_empty());
    }

    /// `apply_channel_modestring`'s own tests, using
    /// `Nicklist::default()`'s standard `beI,k,l,imnpst` CHANMODES /
    /// `(qaohv)~&@%+` PREFIX -- same defaults every other test file in
    /// this crate assumes when no `RPL_ISUPPORT` was ever parsed.
    mod channel_modestring {
        use super::*;

        #[test]
        fn a_type_d_flag_is_added_bare() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("", "+n", &[], &nl);
            assert_eq!(modes, "n");
        }

        #[test]
        fn several_type_d_flags_end_up_sorted_regardless_of_input_order() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("", "+ts", &[], &nl);
            assert_eq!(modes, "st");
        }

        #[test]
        fn a_type_d_flag_can_be_removed_again() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("n", "-n", &[], &nl);
            assert_eq!(modes, "");
        }

        #[test]
        fn a_type_b_mode_carries_its_arg_while_set() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("", "+k", &["secretkey".to_string()], &nl);
            assert_eq!(modes, "k secretkey");
        }

        #[test]
        fn removing_a_type_b_mode_drops_the_letter_and_its_arg_entirely() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("k secretkey", "-k", &["whatever".to_string()], &nl);
            assert_eq!(modes, "");
        }

        #[test]
        fn a_type_c_mode_carries_its_arg_only_while_set() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("", "+l", &["50".to_string()], &nl);
            assert_eq!(modes, "l 50");
        }

        #[test]
        fn removing_a_type_c_mode_takes_no_arg_off_the_wire_at_all() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("l 50", "-l", &[], &nl);
            assert_eq!(modes, "");
        }

        #[test]
        fn a_type_a_list_mode_never_appears_but_still_consumes_its_arg() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("", "+bn", &["BadBot!*@*".to_string()], &nl);
            assert_eq!(modes, "n");
        }

        #[test]
        fn a_user_prefix_rank_never_appears_but_still_consumes_its_arg() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("", "+on", &["SomeNick".to_string()], &nl);
            assert_eq!(modes, "n");
        }

        #[test]
        fn mixed_types_in_one_line_sort_letters_with_args_in_matching_order() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("", "+ntkl", &["secretkey".to_string(), "50".to_string()], &nl);
            // Sorted: k, l, n, t -- args follow in that same order (only
            // k and l carry one).
            assert_eq!(modes, "klnt secretkey 50");
        }

        #[test]
        fn building_incrementally_across_several_mode_lines_matches_one_combined_line() {
            let nl = Nicklist::default();
            let mut modes = String::new();
            modes = apply_channel_modestring(&modes, "+n", &[], &nl);
            modes = apply_channel_modestring(&modes, "+k", &["secretkey".to_string()], &nl);
            modes = apply_channel_modestring(&modes, "+l", &["50".to_string()], &nl);
            assert_eq!(modes, "kln secretkey 50");

            modes = apply_channel_modestring(&modes, "-k", &["secretkey".to_string()], &nl);
            assert_eq!(modes, "ln 50");
        }

        #[test]
        fn an_unclassified_letter_is_treated_as_a_bare_type_d_flag() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("", "+Z", &[], &nl);
            assert_eq!(modes, "Z");
        }

        #[test]
        fn setting_an_already_set_type_c_mode_again_replaces_its_arg_not_duplicates_it() {
            let nl = Nicklist::default();
            let modes = apply_channel_modestring("l 50", "+l", &["100".to_string()], &nl);
            assert_eq!(modes, "l 100");
        }
    }

    #[test]
    fn rename_preserves_rank() {
        let mut nl = Nicklist::default();
        nl.add_from_names_token("@OldNick");
        nl.rename("OldNick", "NewNick");

        let sorted = nl.sorted();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].nick, "NewNick");
        assert_eq!(nl.prefix_char(sorted[0].rank), Some('@'));
    }

    #[test]
    fn remove_drops_the_entry() {
        let mut nl = Nicklist::default();
        nl.insert_plain("Gone");
        nl.remove("Gone");
        assert!(nl.is_empty());
    }

    #[test]
    fn new_entries_start_not_away() {
        let mut nl = Nicklist::default();
        nl.insert_plain("Nick");
        assert!(!nl.sorted()[0].away);
    }

    #[test]
    fn set_away_marks_and_unmarks_an_entry() {
        let mut nl = Nicklist::default();
        nl.insert_plain("Nick");
        nl.set_away("Nick", true);
        assert!(nl.sorted()[0].away);
        nl.set_away("Nick", false);
        assert!(!nl.sorted()[0].away);
    }

    #[test]
    fn set_away_on_an_unknown_nick_is_a_no_op() {
        let mut nl = Nicklist::default();
        nl.set_away("Ghost", true);
        assert!(nl.is_empty());
    }

    #[test]
    fn rename_preserves_away_status() {
        let mut nl = Nicklist::default();
        nl.insert_plain("OldNick");
        nl.set_away("OldNick", true);
        nl.rename("OldNick", "NewNick");
        assert!(nl.sorted()[0].away);
    }

    #[test]
    fn prefix_table_parses_the_standard_isupport_value() {
        let table = PrefixTable::parse("(qaohv)~&@%+").unwrap();
        assert_eq!(table, PrefixTable::standard());
    }

    #[test]
    fn prefix_table_parse_rejects_malformed_values() {
        assert!(PrefixTable::parse("no parens here").is_none());
        assert!(PrefixTable::parse("(ov)@").is_none(), "mismatched letter/symbol counts");
        assert!(PrefixTable::parse("()").is_none(), "empty is not a valid table");
    }

    /// Proves `set_prefix_table` actually REPLACES the default rather
    /// than merging with it: on a server reporting only `(ov)@+`, a
    /// letter the standard scheme has (`h`) no longer resolves to
    /// anything at all.
    #[test]
    fn a_narrower_server_reported_table_replaces_the_default() {
        let mut nl = Nicklist::default();
        nl.insert_plain("Nick");
        nl.set_prefix_table(PrefixTable::parse("(ov)@+").unwrap());

        apply_mode_line(&mut nl, "+o", &["Nick".to_string()]);
        assert_eq!(
            nl.prefix_for("Nick"),
            Some('@'),
            "o is still a real prefix mode on this table"
        );

        apply_mode_line(&mut nl, "-o+h", &["Nick".to_string(), "Nick".to_string()]);
        assert_eq!(
            nl.prefix_for("Nick"),
            None,
            "h isn't a prefix letter on this narrower table, so +h must be a no-op"
        );
    }

    #[test]
    fn chanmodes_table_parses_the_standard_isupport_value() {
        let table = ChanModesTable::parse("beI,k,l,imnpst").unwrap();
        assert_eq!(table, ChanModesTable::standard());
    }

    #[test]
    fn chanmodes_table_parse_rejects_malformed_values() {
        assert!(
            ChanModesTable::parse("beI,k,l").is_none(),
            "only 3 groups, needs exactly 4"
        );
        assert!(
            ChanModesTable::parse("beI,k,l,imnpst,extra").is_none(),
            "5 groups, needs exactly 4"
        );
    }

    #[test]
    fn a_narrower_chanmodes_table_replaces_the_default() {
        let mut nl = Nicklist::default();
        nl.insert_plain("Nick");
        nl.set_chanmodes_table(ChanModesTable::parse("eb,k,l,imnpst").unwrap());

        // Only ONE arg for both letters -- if `I` wrongly ate it for
        // itself (the default table's behavior), `o` would find the
        // iterator empty and never apply anything at all.
        apply_mode_line(&mut nl, "+Io", &["Nick".to_string()]);
        assert_eq!(
            nl.prefix_for("Nick"),
            Some('@'),
            "I takes no arg on this table, so the one arg must still land on o"
        );
    }
}
