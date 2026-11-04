use crate::network::{NetworkPreset, ServerAddr};

pub fn preset() -> NetworkPreset {
    NetworkPreset {
        id: "libera".to_string(),
        display_name: "Libera Chat".to_string(),
        servers: vec![ServerAddr {
            host: "irc.libera.chat".to_string(),
            port: Some(6697),
        }],
        default_port: 6697,
        tls: true,
        autojoin: vec!["##irust-abcx-scratch".to_string()],
    }
}
