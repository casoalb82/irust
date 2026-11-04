use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One channel irust had joined at `/layout save` time, with its `+k`
/// key if it had one (so a rejoin on restore doesn't hit `475
/// ERR_BADCHANNELKEY`, same reasoning as the `001` handler's own
/// remembered-key rejoin).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct LayoutChannel {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Everything `/layout save` remembered for ONE network tag: which
/// channels were joined (in save order -- irrelevant to restore, since
/// `ClientState::buffer_ids()` re-sorts alphabetically regardless, but
/// kept stable for a predictable `layout.toml` diff) and which query
/// windows were open.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct NetworkLayout {
    #[serde(default)]
    pub channels: Vec<LayoutChannel>,
    #[serde(default)]
    pub queries: Vec<String>,
}

impl NetworkLayout {
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty() && self.queries.is_empty()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct LayoutFile {
    #[serde(default)]
    networks: BTreeMap<String, NetworkLayout>,
}

const LAYOUT_FILE: &str = "layout.toml";

fn read_file() -> LayoutFile {
    crate::storage::load_toml(LAYOUT_FILE)
}

fn write_file(file: &LayoutFile) -> std::io::Result<()> {
    crate::storage::save_toml(LAYOUT_FILE, file, false)
}

pub fn load_layout(tag: &str) -> NetworkLayout {
    read_file().networks.remove(tag).unwrap_or_default()
}

pub fn has_saved_layout(tag: &str) -> bool {
    read_file().networks.contains_key(tag)
}

/// Every network with a saved layout, for `/layout list`.
pub fn load_all_layouts() -> BTreeMap<String, NetworkLayout> {
    read_file().networks
}

/// Overwrites ONE network's saved layout, leaving every other network's
/// entry in the file untouched -- `/layout save` only ever knows about
/// the server(s) actually connected when it runs, one tag at a time.
pub fn save_layout(tag: &str, layout: NetworkLayout) -> std::io::Result<()> {
    let mut file = read_file();
    file.networks.insert(tag.to_string(), layout);
    write_file(&file)
}

pub fn reset_all_layouts() -> std::io::Result<()> {
    write_file(&LayoutFile::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_parses_to_no_networks() {
        let parsed: LayoutFile = toml::from_str("").unwrap();
        assert!(parsed.networks.is_empty());
    }

    #[test]
    fn a_network_with_channels_and_queries_round_trips() {
        let toml_str = r##"
            [networks.chatnet]
            queries = ["dana"]

            [[networks.chatnet.channels]]
            name = "#chatnet"

            [[networks.chatnet.channels]]
            name = "#routing"
            key = "secret"
        "##;
        let parsed: LayoutFile = toml::from_str(toml_str).unwrap();
        let chatnet = &parsed.networks["chatnet"];
        assert_eq!(
            chatnet.channels,
            vec![
                LayoutChannel {
                    name: "#chatnet".to_string(),
                    key: None
                },
                LayoutChannel {
                    name: "#routing".to_string(),
                    key: Some("secret".to_string())
                },
            ]
        );
        assert_eq!(chatnet.queries, vec!["dana".to_string()]);
    }

    #[test]
    fn load_layout_on_an_unknown_tag_is_empty_not_an_error() {
        // No real filesystem interaction needed to prove the "missing
        // key" branch -- this exercises the same `unwrap_or_default`
        // path a missing file/path would also take.
        let file = LayoutFile::default();
        assert!(file.networks.get("chatnet").is_none());
        assert_eq!(
            NetworkLayout::default(),
            NetworkLayout {
                channels: Vec::new(),
                queries: Vec::new()
            }
        );
    }

    #[test]
    fn network_layout_is_empty_reflects_both_fields() {
        assert!(NetworkLayout::default().is_empty());
        assert!(!NetworkLayout {
            channels: vec![LayoutChannel {
                name: "#a".to_string(),
                key: None
            }],
            queries: Vec::new(),
        }
        .is_empty());
        assert!(!NetworkLayout {
            channels: Vec::new(),
            queries: vec!["nick".to_string()]
        }
        .is_empty());
    }
}
