use std::collections::HashMap;
use std::fs;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::network::{NetworkPreset, ServerAddr};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct NetworkOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starttls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_verify: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sasl_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sasl_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,
    /// Path to the PEM private key matching `client_cert`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autojoin: Option<Vec<String>>,
    /// Connect to this network automatically at startup, irssi-style --
    /// defaults to `false`. Networks without this set stay untouched
    /// until a `/connect <name>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoconnect: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default)]
    pub networks: HashMap<String, NetworkOverride>,
}

pub fn save_config(config: &AppConfig) -> std::io::Result<()> {
    let Some(dirs) = ProjectDirs::from("", "", "irust") else {
        return Ok(());
    };
    let path = dirs.config_dir().join("config.toml");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let toml_str =
        toml::to_string_pretty(config).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&path, toml_str)?;
    // `config.toml` can hold real secrets (`sasl_password`/
    // `server_password`/a client-cert path) -- see `crate::perms`'s own
    // doc comment for why this is needed on every save, not just once.
    crate::perms::restrict_to_owner(&path);
    Ok(())
}

/// The result of merging a built-in preset with the user's config
/// overrides -- everything the client actually needs to attempt a
/// connection. `host` is `None` if neither the preset nor the config
/// supplied one (ChatNet's current state: known, not yet configured).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedNetwork {
    pub host: Option<String>,
    pub port: u16,
    pub tls: bool,
    pub starttls: bool,
    pub tls_verify: bool,
    pub tls_fingerprint: Option<String>,
    pub nick: Option<String>,
    pub sasl_account: Option<String>,
    pub sasl_password: Option<String>,
    pub server_password: Option<String>,
    /// Both `Some` together means "use SASL EXTERNAL with this cert" --
    /// see `NetworkOverride::client_cert`'s own doc comment.
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
    pub autojoin: Vec<String>,
    pub autoconnect: bool,
}

pub fn load_config() -> AppConfig {
    let Some(dirs) = ProjectDirs::from("", "", "irust") else {
        return AppConfig::default();
    };
    let path = dirs.config_dir().join("config.toml");
    let Ok(contents) = fs::read_to_string(&path) else {
        return AppConfig::default();
    };
    crate::warnings::parse_toml_or_warn(&contents, &path).unwrap_or_default()
}

/// Merges a built-in preset with the user's override for that preset's
/// `id` (if any). Preset fields are the defaults; anything present in
/// the override wins.
pub fn resolve_network(preset: &NetworkPreset, config: &AppConfig) -> ResolvedNetwork {
    let empty = NetworkOverride::default();
    let over = config.networks.get(&preset.id).unwrap_or(&empty);

    let host = over
        .host
        .clone()
        .or_else(|| preset.servers.first().map(|s| s.host.clone()));
    let port = over.port.unwrap_or_else(|| {
        preset
            .servers
            .first()
            .and_then(|s: &ServerAddr| s.port)
            .unwrap_or(preset.default_port)
    });

    ResolvedNetwork {
        host,
        port,
        tls: over.tls.unwrap_or(preset.tls),
        starttls: over.starttls.unwrap_or(false),
        tls_verify: over.tls_verify.unwrap_or(true),
        tls_fingerprint: over.tls_fingerprint.clone(),
        nick: over.nick.clone(),
        sasl_account: over.sasl_account.clone(),
        sasl_password: over.sasl_password.clone(),
        server_password: over.server_password.clone(),
        client_cert: over.client_cert.clone(),
        client_key: over.client_key.clone(),
        autojoin: over.autojoin.clone().unwrap_or_else(|| preset.autojoin.clone()),
        autoconnect: over.autoconnect.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets;

    #[test]
    fn no_override_leaves_the_preset_unfilled() {
        let preset = presets::chatnet::preset();
        let config = AppConfig::default();
        let resolved = resolve_network(&preset, &config);
        assert_eq!(resolved.host, None);
        assert_eq!(resolved.port, preset.default_port);
    }

    #[test]
    fn override_fills_in_the_real_connection_details() {
        let preset = presets::chatnet::preset();
        let mut config = AppConfig::default();
        config.networks.insert(
            "chatnet".to_string(),
            NetworkOverride {
                host: Some("irc.example.gg".to_string()),
                port: Some(6697),
                sasl_account: Some("abcX".to_string()),
                sasl_password: Some("hunter2".to_string()),
                autojoin: Some(vec!["#chatnet".to_string()]),
                ..Default::default()
            },
        );
        let resolved = resolve_network(&preset, &config);
        assert_eq!(resolved.host.as_deref(), Some("irc.example.gg"));
        assert_eq!(resolved.port, 6697);
        assert!(resolved.tls, "unset tls override keeps the preset's true");
        assert_eq!(resolved.sasl_account.as_deref(), Some("abcX"));
        assert_eq!(resolved.autojoin, vec!["#chatnet".to_string()]);
    }

    #[test]
    fn starttls_defaults_to_false_and_an_override_wins() {
        let preset = presets::chatnet::preset();
        let config = AppConfig::default();
        assert!(
            !resolve_network(&preset, &config).starttls,
            "unset starttls must default to false"
        );

        let mut config = AppConfig::default();
        config.networks.insert(
            "chatnet".to_string(),
            NetworkOverride {
                starttls: Some(true),
                ..Default::default()
            },
        );
        assert!(resolve_network(&preset, &config).starttls);
    }

    #[test]
    fn override_for_a_different_network_id_does_not_leak_in() {
        let preset = presets::chatnet::preset();
        let mut config = AppConfig::default();
        config.networks.insert(
            "some-other-network".to_string(),
            NetworkOverride {
                host: Some("irc.elsewhere.example".to_string()),
                ..Default::default()
            },
        );
        let resolved = resolve_network(&preset, &config);
        assert_eq!(resolved.host, None);
    }

    #[test]
    fn missing_config_file_parses_as_empty_not_an_error() {
        let parsed: AppConfig = toml::from_str("").unwrap();
        assert!(parsed.networks.is_empty());
    }
}
