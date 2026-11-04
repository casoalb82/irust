//! irssi's `channel-rejoin.c`: a join refused with `437`
//! (ERR_UNAVAILRESOURCE -- the channel is temporarily unavailable,
//! typically mid-netsplit) is not reported; the channel goes on a rejoin
//! list and is retried every [`REJOIN_INTERVAL`] until a join succeeds or
//! fails for another reason. Gated by `channels_rejoin_unavailable`.

use std::time::Duration;

/// `REJOIN_TIMEOUT` in `channel-rejoin.c`.
pub const REJOIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq)]
pub struct Rejoin {
    pub channel: String,
    pub key: Option<String>,
    /// A JOIN for it is in flight; if it is still `true` at the next tick
    /// the join failed for some other reason and the entry is dropped
    /// (`server_rejoin_channels`: "we missed the join (failed) message").
    pub joining: bool,
}

/// `JOIN a,b k1,k2` for everything due, marking those as in flight, or
/// `None` when the list is (or becomes) empty. Keys follow irssi's
/// `"x"` placeholder rule for channels without one, and are only sent if
/// at least one channel has one.
pub fn tick(list: &mut Vec<Rejoin>) -> Option<String> {
    list.retain(|r| !r.joining);
    if list.is_empty() {
        return None;
    }
    let mut channels = Vec::new();
    let mut keys = Vec::new();
    let mut use_keys = false;
    for r in list.iter_mut() {
        r.joining = true;
        channels.push(r.channel.clone());
        match &r.key {
            Some(k) => {
                keys.push(k.clone());
                use_keys = true;
            }
            None => keys.push("x".to_string()),
        }
    }
    let mut line = format!("JOIN {}", channels.join(","));
    if use_keys {
        line.push(' ');
        line.push_str(&keys.join(","));
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(c: &str, k: Option<&str>) -> Rejoin {
        Rejoin { channel: c.to_string(), key: k.map(str::to_string), joining: false }
    }

    #[test]
    fn tick_joins_everything_due_with_irssis_key_placeholder() {
        let mut l = vec![rec("#a", None), rec("#b", Some("sekrit"))];
        assert_eq!(tick(&mut l).as_deref(), Some("JOIN #a,#b x,sekrit"));
        assert!(l.iter().all(|r| r.joining));
    }

    #[test]
    fn keyless_list_sends_no_key_parameter() {
        let mut l = vec![rec("#a", None), rec("#b", None)];
        assert_eq!(tick(&mut l).as_deref(), Some("JOIN #a,#b"));
    }

    #[test]
    fn an_entry_still_joining_at_the_next_tick_is_dropped() {
        let mut l = vec![rec("#a", None)];
        assert!(tick(&mut l).is_some());
        assert_eq!(tick(&mut l), None, "the join never resolved: given up");
        assert!(l.is_empty());
    }
}
