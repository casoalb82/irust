pub mod activity;
pub mod bantype;
pub mod beep;
pub mod buffer;
pub mod client;
pub mod command_history;
pub mod ctcp_reply;
pub mod dcc;
pub mod dcc_transfer;
pub mod emphasis;
pub mod eval;
pub mod flood;
pub mod highlight;
pub mod input;
pub mod log_format;
pub mod keybind;
pub mod line_edit;
pub mod mirc;
pub mod netsplit;
pub mod nick_completion;
pub mod nickcolor;
pub mod nicklist;
pub mod nickmatch;
pub mod rejoin;
pub mod split;
pub mod servers;
pub mod service_notices;
pub mod state;

pub use buffer::{now_hms, now_stamp, today, Buffer, BufferLine};
pub use client::{
    contains_risky_emoji_sequence, handle_message, is_replayed_message, handle_message_with_highlight, is_ignored, is_ignored_reply,
};
pub use eval::{eval_commands, EvalContext};
pub use highlight::mentions_nick;
pub use input::{parse_input, ChannelSetupAction, NetworkExtras, FormatMode, InputAction, LogToggle, RecodeSub};
pub use keybind::{is_valid_action, normalize_key_name, BINDABLE_ACTIONS, DEFAULT_KEYBINDS};
pub use mirc::{parse_mirc, MircStyle, Segment as MircSegment};
pub use nickcolor::NickColors;
pub use nicklist::Nicklist;
pub use servers::{GlobalWindow, Servers};
pub use state::{
    channel_completion_candidates, BufferId, BufferSummary, ChannelState, ClientState, PendingIp, SplitOrientation,
    SplitPane,
};

/// Shared by unit tests in this crate: the `/set` values (`settings_net`,
/// and anything that republishes them via `Settings::set`) are process
/// global, so a test that sets one, or whose output depends on one, holds
/// this guard and starts from irssi's defaults.
#[cfg(test)]
pub(crate) mod test_support {
    pub fn net_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        irust_config::settings_net::NetSettings::default().publish();
        guard
    }

    pub fn net_set(key: &str, value: &str) {
        // One override over the defaults the guard just restored.
        let mut n = irust_config::settings_net::NetSettings::default();
        n.set(key, value).unwrap();
    }
}
