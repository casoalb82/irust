use crate::state::ClientState;

/// How long a JOIN stays eligible to claim a service's on-join NOTICE --
/// see `ClientState::pending_join_notices`' own doc comment for the full
/// reasoning. Generous enough to absorb normal services round-trip
/// latency, short enough to keep cross-channel misattribution rare
/// during a fast autojoin burst.
const SERVICE_NOTICE_JOIN_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

pub const SERVICE_NICKS: &[&str] = &[
    "nickserv",
    "k9",
    "os",
    "chatworld",
    "global",
    "memoserv",
    "infoserv",
    "saslserv",
    "groupserv",
    "hostserv",
    "botserv",
    "gameserv",
    "rpgserv",
    "helpserv",
    "statserv",
    "alis",
    "cncheck",
];

pub fn is_service_nick(nick: &str) -> bool {
    SERVICE_NICKS.iter().any(|s| s.eq_ignore_ascii_case(nick))
}

pub fn resolve_service_alias(word: &str) -> Option<&'static str> {
    match word.to_lowercase().as_str() {
        "cw" => Some("ChatWorld"),
        "ns" => Some("NickServ"),
        "ms" => Some("MemoServ"),
        _ => None,
    }
}

pub fn route_service_notice(state: &mut ClientState, sender_nick: &str, text: &str) {
    let color_index = state.nick_colors.resolve(sender_nick);
    let buffer = route_service_buffer(state, sender_nick);
    buffer.push_chat(sender_nick.to_string(), color_index, None, text.to_string(), true);
}

pub fn route_service_buffer<'a>(state: &'a mut ClientState, sender_nick: &str) -> &'a mut crate::buffer::Buffer {
    let already_exists = state.query_buffers.contains_key(&sender_nick.to_lowercase());
    let now = std::time::Instant::now();
    state
        .pending_join_notices
        .retain(|(_, joined_at)| now.duration_since(*joined_at) < SERVICE_NOTICE_JOIN_WINDOW);
    let recent_join_channel = state.pending_join_notices.front().map(|(key, _)| key.clone());
    irust_config::debug_log::line(
        "SVC",
        sender_nick,
        &format!(
            "already_exists={already_exists} active_buffer={:?} recent_join_channel={recent_join_channel:?}",
            state.active_buffer
        ),
    );
    if let Some(key) = recent_join_channel {
        if state.channels.contains_key(&key) {
            return &mut state.channels.get_mut(&key).unwrap().buffer;
        }
    }
    if already_exists {
        state.get_or_create_query_buffer(sender_nick, sender_nick)
    } else {
        state.current_buffer_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::handle_message;
    use crate::state::BufferId;
    use irust_proto::parse;

    #[test]
    fn on_join_notice_routes_to_the_just_joined_channel_not_the_active_buffer() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        handle_message(&mut st, &parse(":abcX!abcX@host JOIN #newchan").unwrap());
        // Simulate the active buffer having moved on (a later JOIN in
        // the same burst, or the user just looking elsewhere) before
        // K9's reply to *this* JOIN actually arrives.
        st.active_buffer = BufferId::Status;

        let before = st.channel("#newchan").unwrap().buffer.lines.len();
        route_service_notice(&mut st, "K9", "Welcome to #newchan!");

        assert_eq!(st.status.lines.len(), 0, "must not land in Status");
        assert_eq!(st.channel("#newchan").unwrap().buffer.lines.len(), before + 1);
    }

    /// K9's `ONJOIN` can be configured with several lines -- all of them
    /// must land in the same channel, so the pending-join entry must not
    /// be consumed after the first match.
    #[test]
    fn multiple_onjoin_lines_for_the_same_channel_all_land_there() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        handle_message(&mut st, &parse(":abcX!abcX@host JOIN #newchan").unwrap());
        st.active_buffer = BufferId::Status;

        let before = st.channel("#newchan").unwrap().buffer.lines.len();
        route_service_notice(&mut st, "K9", "line one");
        route_service_notice(&mut st, "K9", "line two");

        assert_eq!(st.channel("#newchan").unwrap().buffer.lines.len(), before + 2);
        assert_eq!(st.status.lines.len(), 0);
    }

    /// Once a pending join ages out of `SERVICE_NOTICE_JOIN_WINDOW`, a
    /// service notice must fall back to the active buffer again -- an
    /// unrelated NOTICE minutes later shouldn't still get attributed to
    /// a join from ages ago.
    #[test]
    fn an_expired_pending_join_falls_back_to_the_active_buffer() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        handle_message(&mut st, &parse(":abcX!abcX@host JOIN #newchan").unwrap());
        // Back-date the just-pushed entry past the window instead of
        // actually sleeping in the test.
        st.pending_join_notices.clear();
        st.pending_join_notices.push_back((
            "#newchan".to_string(),
            std::time::Instant::now() - SERVICE_NOTICE_JOIN_WINDOW - std::time::Duration::from_secs(1),
        ));
        st.active_buffer = BufferId::Status;
        let before = st.channel("#newchan").unwrap().buffer.lines.len();

        route_service_notice(&mut st, "K9", "unrelated, much later");

        assert_eq!(st.channel("#newchan").unwrap().buffer.lines.len(), before);
        assert_eq!(st.status.lines.len(), 1);
    }

    #[test]
    fn recognizes_the_full_ported_service_list_case_insensitively() {
        assert!(is_service_nick("K9"));
        assert!(is_service_nick("k9"));
        assert!(is_service_nick("ChatWorld"));
        assert!(is_service_nick("NickServ"));
        assert!(!is_service_nick("SomeRandomUser"));
    }

    #[test]
    fn with_no_query_already_open_it_lands_in_the_active_buffer() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        route_service_notice(&mut st, "K9", "Welcome back.");
        assert!(!st.query_buffers.contains_key("k9"), "must not auto-create a query");
        let buf = &st.channel("#irust-test").unwrap().buffer;
        assert_eq!(buf.lines.len(), 1);
    }

    #[test]
    fn a_second_notice_with_still_no_query_lands_in_the_active_buffer_too() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        route_service_notice(&mut st, "K9", "first");
        let color_first = match &st.channel("#irust-test").unwrap().buffer.lines[0] {
            crate::buffer::BufferLine::Chat { color_index, .. } => *color_index,
            _ => panic!("expected chat line"),
        };
        route_service_notice(&mut st, "K9", "second");
        let buf = &st.channel("#irust-test").unwrap().buffer;
        assert_eq!(buf.lines.len(), 2);
        let color_second = match &buf.lines[1] {
            crate::buffer::BufferLine::Chat { color_index, .. } => *color_index,
            _ => panic!("expected chat line"),
        };
        assert_eq!(color_first, color_second);
    }

    #[test]
    fn an_already_open_query_is_reused_regardless_of_the_active_buffer() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.get_or_create_query_buffer("k9", "K9");
        route_service_notice(&mut st, "K9", "hi from k9");
        assert_eq!(st.query_buffers["k9"].lines.len(), 1);
        // and it must not also have leaked into the active channel
        assert!(st.channel("#irust-test").unwrap().buffer.lines.is_empty());
    }

    #[test]
    fn a_recent_join_wins_over_an_already_open_query_for_the_same_service() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.get_or_create_query_buffer("k9", "K9");
        handle_message(&mut st, &parse(":abcX!abcX@host JOIN #newchan").unwrap());
        let before = st.channel("#newchan").unwrap().buffer.lines.len();

        route_service_notice(&mut st, "K9", "Welcome to #newchan!");

        assert_eq!(
            st.channel("#newchan").unwrap().buffer.lines.len(),
            before + 1,
            "onjoin welcome must land in the just-joined channel"
        );
        assert!(
            st.query_buffers["k9"].lines.is_empty(),
            "must not leak into the unrelated pre-existing K9 query"
        );
    }

    #[test]
    fn different_services_each_get_their_own_already_open_query() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.get_or_create_query_buffer("k9", "K9");
        st.get_or_create_query_buffer("chatworld", "ChatWorld");
        route_service_notice(&mut st, "K9", "hi from k9");
        route_service_notice(&mut st, "ChatWorld", "hi from cw");
        assert_eq!(st.query_buffers["k9"].lines.len(), 1);
        assert_eq!(st.query_buffers["chatworld"].lines.len(), 1);
    }
}
