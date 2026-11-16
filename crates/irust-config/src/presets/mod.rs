pub mod chatnet;
pub mod libera;

use crate::network::NetworkPreset;

/// Every network the client ships knowing about. Adding another preset
/// means writing one more file here and pushing it onto this list, no
/// other code changes.
pub fn builtin_presets() -> Vec<NetworkPreset> {
    vec![chatnet::preset(), libera::preset()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatnet_is_a_builtin_preset() {
        let presets = builtin_presets();
        assert!(presets.iter().any(|p| p.id == "chatnet"));
    }

    #[test]
    fn chatnet_preset_is_ready_to_connect() {
        let p = chatnet::preset();
        assert_eq!(p.servers.len(), 1);
        assert_eq!(p.servers[0].host, "irc.chatnet.gg");
        assert_eq!(p.servers[0].port, Some(6697));
        assert!(p.tls);
        assert_eq!(p.autojoin, vec!["#ChatNet".to_string()]);
    }
}
