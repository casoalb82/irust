use crate::network::NetworkPreset;

pub fn preset() -> NetworkPreset {
    NetworkPreset {
        id: "chatnet".to_string(),
        display_name: "ChatNet".to_string(),
        servers: Vec::new(),
        default_port: 6697,
        tls: true,
        autojoin: Vec::new(),
    }
}
