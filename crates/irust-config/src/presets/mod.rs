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
}
