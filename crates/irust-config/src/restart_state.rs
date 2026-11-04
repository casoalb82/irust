
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct RestartNetwork {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct RestartStateFile {
    #[serde(default)]
    networks: Vec<RestartNetwork>,
}

const RESTART_STATE_FILE: &str = "restart-state.toml";

pub fn save_restart_state(networks: Vec<RestartNetwork>) -> std::io::Result<()> {
    crate::storage::save_toml(RESTART_STATE_FILE, &RestartStateFile { networks }, false)
}

pub fn take_restart_state() -> Vec<RestartNetwork> {
    let file: RestartStateFile = crate::storage::load_toml(RESTART_STATE_FILE);
    if !file.networks.is_empty() {
        if let Some(path) = crate::storage::data_file_path(RESTART_STATE_FILE) {
            let _ = std::fs::remove_file(path);
        }
    }
    file.networks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_parses_to_no_networks() {
        let parsed: RestartStateFile = toml::from_str("").unwrap();
        assert!(parsed.networks.is_empty());
    }

    #[test]
    fn networks_round_trip_with_and_without_nick() {
        let toml_str = r##"
            [[networks]]
            tag = "chatnet"

            [[networks]]
            tag = "efnet"
            nick = "abcX2"
        "##;
        let parsed: RestartStateFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            parsed.networks,
            vec![
                RestartNetwork { tag: "chatnet".to_string(), nick: None },
                RestartNetwork { tag: "efnet".to_string(), nick: Some("abcX2".to_string()) },
            ]
        );
    }
}
