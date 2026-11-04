use std::collections::HashMap;

fn hash_nick(nick: &str, palette_len: usize) -> usize {
    let mut hash: u32 = 0;
    for b in nick.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(b as u32);
    }
    (hash as usize) % palette_len
}

pub struct NickColors {
    /// Lowercased nick -> palette index, loaded once from the persisted
    /// override file at startup.
    overrides: HashMap<String, usize>,
    /// Nick (as last seen) -> palette index, in-memory only for this run.
    session_cache: HashMap<String, usize>,
    palette_len: usize,
}

impl NickColors {
    pub fn new(palette_len: usize) -> Self {
        Self::with_overrides(palette_len, HashMap::new())
    }

    pub fn with_overrides(palette_len: usize, overrides: HashMap<String, usize>) -> Self {
        Self {
            overrides,
            session_cache: HashMap::new(),
            palette_len: palette_len.max(1),
        }
    }

    pub fn resolve(&mut self, nick: &str) -> usize {
        if let Some(&idx) = self.overrides.get(&nick.to_lowercase()) {
            return idx % self.palette_len;
        }
        if let Some(&idx) = self.session_cache.get(nick) {
            return idx;
        }
        let idx = hash_nick(nick, self.palette_len);
        self.session_cache.insert(nick.to_string(), idx);
        idx
    }

    /// Carries a nick's session color over to its new name on a `NICK`
    /// change, instead of letting it re-hash to a (likely different)
    /// color under the new name.
    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(idx) = self.session_cache.remove(old) {
            self.session_cache.insert(new.to_string(), idx);
        }
    }

    /// Sets (or replaces) a persisted per-nick override, e.g. from a
    /// `/color <nick> <index>` command. Takes effect immediately --
    /// `resolve` checks overrides before the session cache.
    pub fn set_override(&mut self, nick: &str, index: usize) {
        self.overrides.insert(nick.to_lowercase(), index);
    }

    /// The current override map, for the caller to persist to disk
    /// (`irust-core` stays filesystem-agnostic -- see `main.rs`).
    pub fn overrides(&self) -> &HashMap<String, usize> {
        &self.overrides
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatworld_and_nickserv_no_longer_collide() {
        let mut nc = NickColors::new(11);
        assert_ne!(nc.resolve("ChatWorld"), nc.resolve("NickServ"));
    }

    #[test]
    fn same_nick_always_resolves_to_the_same_color() {
        let mut nc = NickColors::new(11);
        let a = nc.resolve("K9");
        let b = nc.resolve("K9");
        assert_eq!(a, b);
    }

    #[test]
    fn override_takes_priority_over_hash_and_is_case_insensitive() {
        let mut overrides = HashMap::new();
        overrides.insert("k9".to_string(), 7);
        let mut nc = NickColors::with_overrides(11, overrides);
        assert_eq!(nc.resolve("K9"), 7);
        assert_eq!(nc.resolve("k9"), 7);
    }

    #[test]
    fn set_override_takes_effect_immediately_and_is_case_insensitive() {
        let mut nc = NickColors::new(11);
        let before = nc.resolve("K9");
        nc.set_override("K9", (before + 1) % 11);
        assert_eq!(nc.resolve("k9"), (before + 1) % 11);
        assert_eq!(nc.overrides().get("k9"), Some(&((before + 1) % 11)));
    }

    #[test]
    fn nick_change_keeps_the_session_color() {
        let mut nc = NickColors::new(11);
        let before = nc.resolve("OldNick");
        nc.rename("OldNick", "NewNick");
        assert_eq!(nc.resolve("NewNick"), before);
    }

    #[test]
    fn out_of_range_override_index_is_wrapped_to_the_palette() {
        let mut overrides = HashMap::new();
        overrides.insert("weird".to_string(), 999);
        let mut nc = NickColors::with_overrides(11, overrides);
        assert!(nc.resolve("weird") < 11);
    }
}
