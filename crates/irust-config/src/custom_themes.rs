use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct CustomTheme {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statusbar_bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statusbar_fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leave: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,

    // Format templates -- the wording half of a theme, see
    // `theme_formats.rs`'s own doc comment for the full design and
    // every `$placeholder` each of these accepts. Same "override only
    // what you want different" rule as the colors above: any left
    // unset here fall back to that format's own built-in default text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_join: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_join_self: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_part: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_part_self: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_quit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_kick: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_kick_self: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_nick: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_nick_self: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_mode_channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_mode_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_chghost: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_account_login: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_account_logout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_setname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_invite: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_invite_notify: Option<String>,

    // DCC transfer + WHOIS formats -- the same "still fixed text in
    // client.rs"/`dcc_transfer.rs` gap the theme system otherwise
    // already closed for join/part/quit/etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_dcc_send_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_dcc_get_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_dcc_transfer_complete: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_dcc_transfer_cancelled: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_dcc_transfer_failed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_whois_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_whois_header: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_whois_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_whowas_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_whois_no_such_nick: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_whowas_no_such_nick: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
struct CustomThemesFile {
    #[serde(default)]
    themes: HashMap<String, CustomTheme>,
}

const CUSTOM_THEMES_FILE: &str = "custom_themes.toml";

pub fn load_custom_themes() -> HashMap<String, CustomTheme> {
    crate::storage::load_toml::<CustomThemesFile>(CUSTOM_THEMES_FILE)
        .themes
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect()
}

/// Every custom theme name currently defined -- `Settings::set`'s
/// "theme" arm accepts these alongside `THEME_NAMES` so `/set theme
/// <custom-name>` doesn't get rejected as unrecognized.
pub fn custom_theme_names() -> Vec<String> {
    load_custom_themes().into_keys().collect()
}

pub fn save_custom_themes(themes: &HashMap<String, CustomTheme>) -> std::io::Result<()> {
    let file = CustomThemesFile { themes: themes.clone() };
    crate::storage::save_toml(CUSTOM_THEMES_FILE, &file, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_with_one_theme_parses_and_lowercases_the_name() {
        let parsed: CustomThemesFile = toml::from_str(
            r##"
            [themes.Sunset]
            statusbar_bg = "203"
            leave = "#ff5500"
            "##,
        )
        .unwrap();
        assert!(parsed.themes.contains_key("Sunset"));
        let theme = &parsed.themes["Sunset"];
        assert_eq!(theme.statusbar_bg.as_deref(), Some("203"));
        assert_eq!(theme.leave.as_deref(), Some("#ff5500"));
        assert_eq!(theme.statusbar_fg, None, "unset fields stay None, not defaulted here");
    }

    #[test]
    fn empty_file_has_no_themes() {
        let parsed: CustomThemesFile = toml::from_str("").unwrap();
        assert!(parsed.themes.is_empty());
    }

    #[test]
    fn save_custom_themes_toml_shape_round_trips() {
        let mut themes = HashMap::new();
        themes.insert(
            "sunset".to_string(),
            CustomTheme {
                format_join: Some("$nick arrived!".to_string()),
                ..Default::default()
            },
        );
        let file = CustomThemesFile { themes: themes.clone() };
        let toml_str = toml::to_string_pretty(&file).unwrap();
        let parsed: CustomThemesFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.themes, themes);
    }
}
