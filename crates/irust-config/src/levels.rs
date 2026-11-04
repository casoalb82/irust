//! Message levels -- a port of irssi's `core/levels.c` / `levels.h`.
//!
//! A level set is a bitmask; settings such as `activity_msg_level` or
//! `beep_msg_level` are stored as the space-separated text form
//! (`"PUBLIC NOTICES"`, `"ALL -CRAP"`) and parsed with [`level2bits`].

pub const CRAP: u32 = 0x0000001;
pub const MSGS: u32 = 0x0000002;
pub const PUBLIC: u32 = 0x0000004;
pub const NOTICES: u32 = 0x0000008;
pub const PUBNOTICES: u32 = 0x0000010;
pub const SNOTES: u32 = 0x0000020;
pub const CTCPS: u32 = 0x0000040;
pub const ACTIONS: u32 = 0x0000080;
pub const JOINS: u32 = 0x0000100;
pub const PARTS: u32 = 0x0000200;
pub const QUITS: u32 = 0x0000400;
pub const KICKS: u32 = 0x0000800;
pub const MODES: u32 = 0x0001000;
pub const TOPICS: u32 = 0x0002000;
pub const WALLOPS: u32 = 0x0004000;
pub const INVITES: u32 = 0x0008000;
pub const NICKS: u32 = 0x0010000;
pub const DCC: u32 = 0x0020000;
pub const DCCMSGS: u32 = 0x0040000;
pub const CLIENTNOTICE: u32 = 0x0080000;
pub const CLIENTCRAP: u32 = 0x0100000;
pub const CLIENTERROR: u32 = 0x0200000;
pub const HILIGHT: u32 = 0x0400000;
pub const ALL: u32 = 0x07fffff;
pub const NOHILIGHT: u32 = 0x1000000;
pub const NO_ACT: u32 = 0x2000000;
pub const NEVER: u32 = 0x4000000;
pub const HIDDEN: u32 = 0x10000000;

/// irssi's own name table, index == bit position. Note the spelling
/// `PUBLICS` (irssi accepts the prefix `PUBLIC` through its partial match).
const NAMES: [&str; 23] = [
    "CRAP",
    "MSGS",
    "PUBLICS",
    "NOTICES",
    "PUBNOTICES",
    "SNOTES",
    "CTCPS",
    "ACTIONS",
    "JOINS",
    "PARTS",
    "QUITS",
    "KICKS",
    "MODES",
    "TOPICS",
    "WALLOPS",
    "INVITES",
    "NICKS",
    "DCC",
    "DCCMSGS",
    "CLIENTNOTICES",
    "CLIENTCRAP",
    "CLIENTERRORS",
    "HILIGHTS",
];

fn level_get(name: &str) -> u32 {
    if name.eq_ignore_ascii_case("ALL") || name == "*" {
        return ALL;
    }
    if name.eq_ignore_ascii_case("NEVER") {
        return NEVER;
    }
    if name.eq_ignore_ascii_case("NO_ACT") {
        return NO_ACT;
    }
    if name.eq_ignore_ascii_case("NOHILIGHT") {
        return NOHILIGHT;
    }
    if name.eq_ignore_ascii_case("HIDDEN") {
        return HIDDEN;
    }
    if name.is_empty() {
        return 0;
    }
    let lower = name.to_ascii_lowercase();
    let mut matched = 0;
    for (n, full) in NAMES.iter().enumerate() {
        if full.to_ascii_lowercase().starts_with(&lower) {
            if full.len() == name.len() {
                return 1 << n;
            }
            if matched > 0 {
                return 0;
            }
            matched = 1 << n;
        }
    }
    matched
}

/// `level2bits`: `"PUBLIC NOTICES"`, `"ALL -CRAP"`, `"+MSGS"`. Returns the
/// bitmask and whether any word was not a known level (irssi's `errorp`).
pub fn level2bits_checked(level: &str) -> (u32, bool) {
    let mut ret = 0;
    let mut error = false;
    for word in level.split(' ').filter(|w| !w.is_empty()) {
        let negative = word.starts_with('-');
        let name = word.strip_prefix(['-', '+']).unwrap_or(word);
        let single = level_get(name);
        if single != 0 {
            if negative {
                ret &= !single;
            } else {
                ret |= single;
            }
        } else {
            error = true;
        }
    }
    (ret, error)
}

pub fn level2bits(level: &str) -> u32 {
    level2bits_checked(level).0
}

/// `settings_get_level_negative`: the levels a level list explicitly
/// subtracts (`"ALL -CRAP"` -> `CRAP`). irssi's beep check uses it as an
/// ignore list on top of the positive levels.
pub fn level2bits_negative(level: &str) -> u32 {
    let everything = ALL | NEVER | NO_ACT | NOHILIGHT | HIDDEN;
    let mut ret = everything;
    for word in level.split(' ').filter(|w| !w.is_empty()) {
        let negative = word.starts_with('-');
        let single = level_get(word.strip_prefix(['-', '+']).unwrap_or(word));
        if negative {
            ret &= !single;
        } else {
            ret |= single;
        }
    }
    ret ^ everything
}

/// `combine_level`: applies `src` on top of `dest` -- `NONE` clears,
/// `-x` removes, `^x` toggles, anything else adds. Used by irssi wherever
/// a level setting is layered over another (`/window level +msgs`).
pub fn combine_level(mut dest: u32, src: &str) -> u32 {
    for item in src.split(' ') {
        let name = item.strip_prefix(['+', '-', '^']).unwrap_or(item);
        let level = level_get(name);
        if name.eq_ignore_ascii_case("NONE") {
            dest = 0;
        } else if item.starts_with('-') {
            dest &= !level;
        } else if item.starts_with('^') {
            dest ^= level;
        } else {
            dest |= level;
        }
    }
    dest
}

/// `bits2level`: the canonical text form.
pub fn bits2level(bits: u32) -> String {
    if bits == 0 {
        return String::new();
    }
    let mut parts: Vec<&str> = Vec::new();
    if bits & NEVER != 0 {
        parts.push("NEVER");
    }
    if bits & NO_ACT != 0 {
        parts.push("NO_ACT");
    }
    if bits & ALL == ALL {
        parts.push("ALL");
    } else {
        for (n, name) in NAMES.iter().enumerate() {
            if bits & (1 << n) != 0 {
                parts.push(name);
            }
        }
    }
    if bits & NOHILIGHT != 0 {
        parts.push("NOHILIGHT");
    }
    if bits & HIDDEN != 0 {
        parts.push("HIDDEN");
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irssi_default_activity_levels_parse_with_the_short_public_name() {
        // `activity_msg_level` defaults to "PUBLIC NOTICES" in irssi; the
        // table entry is "PUBLICS", reached through the unique-prefix rule.
        assert_eq!(level2bits("PUBLIC NOTICES"), PUBLIC | NOTICES);
        assert_eq!(level2bits("MSGS DCCMSGS"), MSGS | DCCMSGS);
    }

    #[test]
    fn a_full_match_beats_a_longer_name_sharing_its_prefix() {
        assert_eq!(level2bits("DCC"), DCC, "DCC is a prefix of DCCMSGS but matches fully");
        assert_eq!(level2bits("PUBN"), PUBNOTICES);
    }

    #[test]
    fn an_ambiguous_prefix_is_rejected() {
        // "CLIENT" prefixes CLIENTNOTICES, CLIENTCRAP and CLIENTERRORS.
        assert_eq!(level2bits_checked("CLIENT"), (0, true));
        // "P" prefixes PUBLICS, PUBNOTICES and PARTS.
        assert_eq!(level2bits_checked("P"), (0, true));
    }

    #[test]
    fn plus_minus_all_and_star() {
        assert_eq!(level2bits("ALL -CRAP"), ALL & !CRAP);
        assert_eq!(level2bits("* -msgs"), ALL & !MSGS);
        assert_eq!(level2bits("+msgs +joins"), MSGS | JOINS);
        assert_eq!(level2bits("msgs -msgs"), 0);
    }

    #[test]
    fn unknown_words_are_flagged_but_the_rest_still_applies() {
        assert_eq!(level2bits_checked("msgs bogus"), (MSGS, true));
        assert_eq!(level2bits_checked(""), (0, false));
    }

    #[test]
    fn special_flags_parse_case_insensitively() {
        assert_eq!(level2bits("never"), NEVER);
        assert_eq!(level2bits("No_Act"), NO_ACT);
        assert_eq!(level2bits("nohilight hidden"), NOHILIGHT | HIDDEN);
    }

    #[test]
    fn bits2level_round_trips_and_uses_irssi_order() {
        assert_eq!(bits2level(0), "");
        assert_eq!(bits2level(PUBLIC | NOTICES), "PUBLICS NOTICES");
        assert_eq!(bits2level(ALL), "ALL");
        assert_eq!(bits2level(NEVER | MSGS | HIDDEN), "NEVER MSGS HIDDEN");
        let text = bits2level(MSGS | ACTIONS | JOINS);
        assert_eq!(level2bits(&text), MSGS | ACTIONS | JOINS);
    }

    #[test]
    fn negative_levels_are_the_explicitly_subtracted_ones() {
        assert_eq!(level2bits_negative("ALL -CRAP"), CRAP);
        assert_eq!(level2bits_negative("MSGS PUBLIC"), 0);
        assert_eq!(level2bits_negative("ALL -JOINS -PARTS"), JOINS | PARTS);
        assert_eq!(level2bits_negative(""), 0);
    }

    #[test]
    fn combine_level_matches_irssi_none_minus_and_toggle() {
        assert_eq!(combine_level(MSGS | JOINS, "NONE"), 0);
        assert_eq!(combine_level(MSGS | JOINS, "-joins"), MSGS);
        assert_eq!(combine_level(MSGS, "^msgs ^parts"), PARTS);
        assert_eq!(combine_level(MSGS, "+notices"), MSGS | NOTICES);
    }
}
