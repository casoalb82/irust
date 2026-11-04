
use std::time::Duration;

/// One currently-recognized netsplit, grouped by server pair --
/// `ClientState::recent_netsplits`'s own return type, `/netsplit`'s
/// data source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetsplitSummary {
    pub server_a: String,
    pub server_b: String,
    pub nick_count: usize,
    pub since: Duration,
}

pub fn is_split_reason(reason: &str) -> bool {
    let mut parts = reason.split(' ');
    let (Some(a), Some(b)) = (parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    !a.is_empty() && !b.is_empty() && a.contains('.') && b.contains('.')
}

pub const NETSPLIT_FLUSH_DELAY: Duration = Duration::from_secs(2);

pub const NETSPLIT_REJOIN_WINDOW: Duration = Duration::from_secs(30 * 60);

/// Which side of a split a collapsed line is for -- each has its own
/// irssi setting with slightly different rules.
#[derive(Clone, Copy)]
pub enum Side {
    /// `netsplit_max_nicks` (default 10), capped by
    /// `netsplit_nicks_hide_threshold` (default 15); 0 = list everyone
    /// (`fe-netsplit.c`: only truncates when max > 0).
    Quits,
    /// `netjoin_max_nicks` (default 10); no "0 = unlimited" special case
    /// in `fe-netjoin.c`, so 0 shows only the "(+N more)" count.
    Joins,
}

fn limit(side: Side) -> Option<usize> {
    use irust_config::settings_net::int_of;
    match side {
        Side::Quits => {
            let max = int_of("netsplit_max_nicks");
            let threshold = int_of("netsplit_nicks_hide_threshold");
            let effective = max.min(threshold);
            (effective > 0).then_some(effective as usize)
        }
        Side::Joins => Some(int_of("netjoin_max_nicks").max(0) as usize),
    }
}

/// Formats a nick list for a collapsed netsplit line, truncating past the
/// side's configured maximum -- shared by both the quit and join side of
/// `ClientState::flush_pending_netsplits`.
pub fn format_nick_list(nicks: &[String], side: Side) -> String {
    format_with_limit(nicks, limit(side))
}

fn format_with_limit(nicks: &[String], limit: Option<usize>) -> String {
    match limit {
        Some(max) if nicks.len() > max => {
            format!("{} (+{} more)", nicks[..max].join(", "), nicks.len() - max)
        }
        _ => nicks.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_a_real_hub_leaf_pair() {
        assert!(is_split_reason("hub.example.net leaf.example.net"));
    }

    #[test]
    fn rejects_a_normal_quit_message() {
        assert!(!is_split_reason("Client Quit"));
        assert!(!is_split_reason("Ping timeout: 240 seconds"));
        assert!(!is_split_reason("bye everyone"));
    }

    #[test]
    fn rejects_a_single_word() {
        assert!(!is_split_reason("hub.example.net"));
    }

    #[test]
    fn rejects_three_or_more_words_even_if_first_two_look_serverish() {
        assert!(!is_split_reason("hub.example.net leaf.example.net extra"));
    }

    #[test]
    fn rejects_when_either_half_has_no_dot() {
        assert!(!is_split_reason("hub.example.net leaf"));
        assert!(!is_split_reason("hub leaf.example.net"));
    }

    #[test]
    fn rejects_empty_reason() {
        assert!(!is_split_reason(""));
    }

    #[test]
    fn format_nick_list_joins_short_lists_plainly() {
        let nicks: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(format_with_limit(&nicks, Some(10)), "a, b, c");
    }

    #[test]
    fn format_nick_list_truncates_past_the_max() {
        let nicks: Vec<String> = (1..=12).map(|i| format!("n{i}")).collect();
        assert_eq!(
            format_with_limit(&nicks, Some(10)),
            "n1, n2, n3, n4, n5, n6, n7, n8, n9, n10 (+2 more)"
        );
    }

    #[test]
    fn no_limit_lists_everyone_and_zero_join_limit_shows_only_the_count() {
        let nicks: Vec<String> = (1..=3).map(|i| format!("n{i}")).collect();
        assert_eq!(format_with_limit(&nicks, None), "n1, n2, n3");
        assert_eq!(format_with_limit(&nicks, Some(0)), " (+3 more)");
    }
}
