use crate::network::{NetworkPreset, ServerAddr};

pub fn preset() -> NetworkPreset {
    NetworkPreset {
        id: "chatnet".to_string(),
        display_name: "ChatNet".to_string(),
        servers: vec![ServerAddr {
            host: "irc.chatnet.gg".to_string(),
            port: Some(6697),
        }],
        default_port: 6697,
        tls: true,
        autojoin: vec!["#ChatNet".to_string()],
    }
}
