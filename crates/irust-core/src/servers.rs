use std::collections::HashMap;

use crate::state::ClientState;

/// One entry of [`Servers::global_windows`]: a window plus the tag of
/// the server it belongs to.
#[derive(Clone)]
pub struct GlobalWindow {
    pub tag: String,
    pub summary: crate::state::BufferSummary,
}

#[derive(Default)]
pub struct Servers {
    states: HashMap<String, ClientState>,
    /// Insertion order, for stable `/server`-less display and cycling.
    order: Vec<String>,
    active: Option<String>,
    pending_messages: Vec<(bool, String)>,
}

impl Servers {
    pub fn push_pending_message(&mut self, warning: bool, message: String) {
        self.pending_messages.push((warning, message));
    }

    pub fn pending_messages(&self) -> &[(bool, String)] {
        &self.pending_messages
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn active_tag(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn active_state(&self) -> Option<&ClientState> {
        self.active.as_ref().and_then(|tag| self.states.get(tag))
    }

    pub fn active_state_mut(&mut self) -> Option<&mut ClientState> {
        let tag = self.active.clone()?;
        self.states.get_mut(&tag)
    }

    pub fn state(&self, tag: &str) -> Option<&ClientState> {
        self.states.get(tag)
    }

    pub fn state_mut(&mut self, tag: &str) -> Option<&mut ClientState> {
        self.states.get_mut(tag)
    }

    pub fn contains(&self, tag: &str) -> bool {
        self.states.contains_key(tag)
    }

    /// Every known server tag, in the order they were added.
    pub fn tags(&self) -> &[String] {
        &self.order
    }

    /// Adds a newly connected server. `make_active` switches the UI to
    /// it immediately (what `/connect` wants); startup auto-connects add
    /// every server up front and pick the active one separately once all
    /// of them are in.
    pub fn add(&mut self, tag: impl Into<String>, state: ClientState, make_active: bool) {
        let tag = tag.into();
        if !self.states.contains_key(&tag) {
            self.order.push(tag.clone());
        }
        self.states.insert(tag.clone(), state);
        if make_active || self.active.is_none() {
            self.active = Some(tag);
        }
    }

    /// Removes a server (`/disconnect`). Picks a new active server if
    /// the removed one was active; `None` is left active only when this
    /// was the last server standing.
    pub fn remove(&mut self, tag: &str) -> bool {
        if self.states.remove(tag).is_none() {
            return false;
        }
        self.order.retain(|t| t != tag);
        if self.active.as_deref() == Some(tag) {
            self.active = self.order.first().cloned();
        }
        true
    }

    /// `/server <name>` -- `false` if no such server is connected.
    pub fn switch_active(&mut self, tag: &str) -> bool {
        if !self.states.contains_key(tag) {
            return false;
        }
        self.active = Some(tag.to_string());
        true
    }

    pub fn global_windows(&self) -> Vec<GlobalWindow> {
        self.order
            .iter()
            .filter_map(|tag| self.states.get(tag).map(|st| (tag, st)))
            .flat_map(|(tag, st)| {
                st.buffer_summaries().into_iter().map(move |summary| GlobalWindow {
                    tag: tag.clone(),
                    summary,
                })
            })
            .collect()
    }

    /// Position in [`global_windows`](Self::global_windows) (0-based) of
    /// the currently focused window, if any server is active.
    pub fn active_global_index(&self) -> Option<usize> {
        let active_tag = self.active.as_deref()?;
        let active_id = &self.states.get(active_tag)?.active_buffer;
        self.global_windows()
            .iter()
            .position(|w| w.tag == active_tag && &w.summary.id == active_id)
    }

    /// Focuses the `index`-th window (1-based) of the global list,
    /// switching the active server along with it. `false` (a no-op) if
    /// `index` is 0 or past the end.
    pub fn goto_global_index(&mut self, index: usize) -> bool {
        let Some(pos) = index.checked_sub(1) else { return false };
        let Some(win) = self.global_windows().into_iter().nth(pos) else {
            return false;
        };
        self.focus_window(&win.tag, win.summary.id);
        true
    }

    /// `Alt-Left`/`Alt-Right` across servers: steps through the global
    /// window list, wrapping, skipping `/window stick`ed windows the
    /// same way the per-server `cycle_buffer` does. A no-op with fewer
    /// than two windows.
    pub fn cycle_global(&mut self, forward: bool) {
        let wins = self.global_windows();
        let len = wins.len();
        if len <= 1 {
            return;
        }
        let current = self.active_global_index().unwrap_or(0);
        let mut pos = current;
        for _ in 0..len {
            pos = if forward { (pos + 1) % len } else { (pos + len - 1) % len };
            let sticky = self
                .states
                .get(&wins[pos].tag)
                .is_some_and(|st| st.sticky.contains(&wins[pos].summary.id));
            if pos == current || !sticky {
                break;
            }
        }
        let win = wins[pos].clone();
        self.focus_window(&win.tag, win.summary.id);
    }

    /// Makes `(tag, id)` the focused window: switches the active server
    /// and that server's `active_buffer`, resetting scroll to the live
    /// tail only if the focus actually moved (same as `goto_buffer_index`).
    fn focus_window(&mut self, tag: &str, id: crate::state::BufferId) {
        let Some(st) = self.states.get_mut(tag) else { return };
        if st.active_buffer != id {
            st.active_buffer = id;
            st.active_scroll_offset = 0;
        }
        self.active = Some(tag.to_string());
    }

    /// Moves the active server forward (or, if `!forward`, backward)
    /// through `tags()`, wrapping around. A no-op with zero or one
    /// server connected.
    pub fn cycle_server(&mut self, forward: bool) {
        let len = self.order.len();
        if len <= 1 {
            return;
        }
        let Some(active) = &self.active else { return };
        let current_pos = self.order.iter().position(|t| t == active).unwrap_or(0);
        let next_pos = if forward {
            (current_pos + 1) % len
        } else {
            (current_pos + len - 1) % len
        };
        self.active = Some(self.order[next_pos].clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(nick: &str) -> ClientState {
        ClientState::new(nick, "#test", 11)
    }

    #[test]
    fn starts_empty_with_no_active_server() {
        let servers = Servers::empty();
        assert!(servers.active_tag().is_none());
        assert!(servers.active_state().is_none());
        assert!(servers.tags().is_empty());
    }

    #[test]
    fn the_first_server_added_becomes_active_even_without_make_active() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        assert_eq!(servers.active_tag(), Some("chatnet"));
    }

    #[test]
    fn a_later_server_only_becomes_active_if_requested() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        servers.add("libera", state("irust-abcX"), false);
        assert_eq!(servers.active_tag(), Some("chatnet"));

        servers.add("third", state("x"), true);
        assert_eq!(servers.active_tag(), Some("third"));
    }

    #[test]
    fn removing_the_active_server_falls_back_to_another_one() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        servers.add("libera", state("irust-abcX"), false);
        assert!(servers.remove("chatnet"));
        assert_eq!(servers.active_tag(), Some("libera"));
    }

    #[test]
    fn removing_the_last_server_leaves_nothing_active() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        assert!(servers.remove("chatnet"));
        assert!(servers.active_tag().is_none());
    }

    #[test]
    fn removing_an_unknown_tag_is_a_no_op() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        assert!(!servers.remove("nope"));
        assert_eq!(servers.active_tag(), Some("chatnet"));
    }

    #[test]
    fn switch_active_requires_an_existing_server() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        servers.add("libera", state("irust-abcX"), false);
        assert!(!servers.switch_active("nope"));
        assert_eq!(servers.active_tag(), Some("chatnet"));
        assert!(servers.switch_active("libera"));
        assert_eq!(servers.active_tag(), Some("libera"));
    }

    #[test]
    fn cycle_server_wraps_around_in_insertion_order() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        servers.add("libera", state("irust-abcX"), false);
        servers.add("third", state("x"), false);
        assert_eq!(servers.active_tag(), Some("chatnet"));

        servers.cycle_server(true);
        assert_eq!(servers.active_tag(), Some("libera"));
        servers.cycle_server(true);
        assert_eq!(servers.active_tag(), Some("third"));
        servers.cycle_server(true);
        assert_eq!(servers.active_tag(), Some("chatnet"), "wraps");

        servers.cycle_server(false);
        assert_eq!(servers.active_tag(), Some("third"), "backward wraps too");
    }

    #[test]
    fn cycle_server_with_one_or_zero_servers_is_a_no_op() {
        let mut servers = Servers::empty();
        servers.cycle_server(true);
        assert!(servers.active_tag().is_none());

        servers.add("chatnet", state("abcX"), false);
        servers.cycle_server(true);
        assert_eq!(servers.active_tag(), Some("chatnet"));
    }

    #[test]
    fn global_windows_concatenate_every_servers_buffers_in_order() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        servers.add("libera", state("abcX"), false);
        let tags: Vec<String> = servers.global_windows().into_iter().map(|w| w.tag).collect();
        assert_eq!(tags, vec!["chatnet".to_string(), "chatnet".to_string(), "libera".to_string(), "libera".to_string()]);
    }

    #[test]
    fn goto_global_index_switches_server_and_buffer() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        servers.add("libera", state("abcX"), false);
        assert_eq!(servers.active_global_index(), Some(1));
        assert!(servers.goto_global_index(4));
        assert_eq!(servers.active_tag(), Some("libera"));
        assert_eq!(servers.active_global_index(), Some(3));
        assert!(servers.goto_global_index(1));
        assert_eq!(servers.active_tag(), Some("chatnet"));
        assert!(!servers.goto_global_index(0));
        assert!(!servers.goto_global_index(99));
    }

    #[test]
    fn cycle_global_crosses_servers_and_wraps() {
        let mut servers = Servers::empty();
        servers.add("chatnet", state("abcX"), false);
        servers.add("libera", state("abcX"), false);
        servers.goto_global_index(2);
        servers.cycle_global(true);
        assert_eq!(servers.active_tag(), Some("libera"));
        servers.cycle_global(true);
        servers.cycle_global(true);
        assert_eq!(servers.active_tag(), Some("chatnet"), "wraps");
        servers.cycle_global(false);
        assert_eq!(servers.active_tag(), Some("libera"));
    }
}
