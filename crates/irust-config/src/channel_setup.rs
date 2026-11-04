use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ChannelSetup {
    #[serde(default)]
    pub autojoin: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    pub botmasks: Vec<String>,
    /// The command sent once per join -- addressed to whichever
    /// `botmasks` entry is actually present in the channel (or, when
    /// `botmasks` is empty, sent to the channel itself right away) --
    /// irssi's own `autosendcmd`/`-botcmd`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub autosendcmd: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct ChannelSetupFile {
    #[serde(default)]
    networks: BTreeMap<String, BTreeMap<String, ChannelSetup>>,
}

const CHANNEL_SETUP_FILE: &str = "channels.toml";

fn read_file() -> ChannelSetupFile {
    crate::storage::load_toml(CHANNEL_SETUP_FILE)
}

fn write_file(file: &ChannelSetupFile) -> std::io::Result<()> {
    crate::storage::save_toml(CHANNEL_SETUP_FILE, file, false)
}

/// Every `/channel`-configured entry for one network tag, keyed by
/// lowercased channel name -- `/channel list`'s own data source, and
/// what `main.rs` unions into a fresh connection's autojoin set
/// alongside `config.toml`'s static list and any saved `/layout`.
pub fn load_channel_setups(tag: &str) -> BTreeMap<String, ChannelSetup> {
    read_file().networks.remove(tag).unwrap_or_default()
}

/// One specific channel's entry, if `/channel add`/`modify` has ever
/// been run for it on this network.
pub fn load_channel_setup(tag: &str, channel: &str) -> Option<ChannelSetup> {
    read_file().networks.get(tag)?.get(&channel.to_lowercase()).cloned()
}

/// Every configured entry, across every network -- `/channel list`'s
/// own data source (irssi's own `/channel list` isn't scoped to one
/// network either).
pub fn load_all_channel_setups() -> BTreeMap<String, BTreeMap<String, ChannelSetup>> {
    read_file().networks
}

/// `/channel add|modify` -- overwrites exactly this (network, channel)
/// entry, leaving every other network/channel in the file untouched.
pub fn save_channel_setup(tag: &str, channel: &str, entry: ChannelSetup) -> std::io::Result<()> {
    let mut file = read_file();
    file.networks
        .entry(tag.to_string())
        .or_default()
        .insert(channel.to_lowercase(), entry);
    write_file(&file)
}

/// `/channel remove` -- returns whether an entry actually existed to
/// remove, so the caller can report "wasn't configured" distinctly
/// from "removed".
pub fn remove_channel_setup(tag: &str, channel: &str) -> std::io::Result<bool> {
    let mut file = read_file();
    let removed = file
        .networks
        .get_mut(tag)
        .map(|channels| channels.remove(&channel.to_lowercase()).is_some())
        .unwrap_or(false);
    if removed {
        write_file(&file)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_parses_to_no_networks() {
        let parsed: ChannelSetupFile = toml::from_str("").unwrap();
        assert!(parsed.networks.is_empty());
    }

    #[test]
    fn a_channel_entry_round_trips_through_save_and_load() {
        let toml_str = r##"
            [networks.chatnet."#chatnet"]
            autojoin = true
            password = "secret"
            botmasks = ["K9", "HelperBot"]
            autosendcmd = "identify hunter2"
            "##;
        let parsed: ChannelSetupFile = toml::from_str(toml_str).unwrap();
        let entry = parsed.networks["chatnet"]["#chatnet"].clone();
        assert!(entry.autojoin);
        assert_eq!(entry.password.as_deref(), Some("secret"));
        assert_eq!(entry.botmasks, vec!["K9".to_string(), "HelperBot".to_string()]);
        assert_eq!(entry.autosendcmd, "identify hunter2");
    }

    #[test]
    fn defaults_parse_from_a_bare_table() {
        let toml_str = r##"[networks.chatnet."#chatnet"]"##;
        let parsed: ChannelSetupFile = toml::from_str(toml_str).unwrap();
        let entry = parsed.networks["chatnet"]["#chatnet"].clone();
        assert_eq!(entry, ChannelSetup::default());
        assert!(!entry.autojoin);
        assert!(entry.password.is_none());
        assert!(entry.botmasks.is_empty());
        assert_eq!(entry.autosendcmd, "");
    }
}
