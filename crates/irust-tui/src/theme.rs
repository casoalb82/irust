
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColors {
    pub statusbar_bg: Color,
    pub statusbar_fg: Color,
    pub timestamp: Color,
    pub join: Color,
    pub leave: Color,
    pub notice: Color,
}

const CLASSIC: ThemeColors = ThemeColors {
    statusbar_bg: Color::Blue,
    statusbar_fg: Color::White,
    timestamp: Color::Indexed(244),
    join: Color::Green,
    leave: Color::Red,
    notice: Color::Magenta,
};

const VIVID: ThemeColors = ThemeColors {
    statusbar_bg: Color::Cyan,
    statusbar_fg: Color::Black,
    timestamp: Color::Blue,
    join: Color::LightGreen,
    leave: Color::Indexed(203),
    notice: Color::LightMagenta,
};

const MONO: ThemeColors = ThemeColors {
    statusbar_bg: Color::Indexed(238),
    statusbar_fg: Color::White,
    timestamp: Color::Indexed(244),
    join: Color::Gray,
    leave: Color::Gray,
    notice: Color::Gray,
};

pub fn current() -> ThemeColors {
    match irust_config::settings::theme().as_str() {
        "vivid" => VIVID,
        "mono" => MONO,
        "classic" => CLASSIC,
        name => irust_config::load_custom_themes()
            .get(name)
            .map(from_custom)
            .unwrap_or(CLASSIC),
    }
}

fn from_custom(custom: &irust_config::CustomTheme) -> ThemeColors {
    let color = |raw: &Option<String>, fallback: Color| {
        raw.as_deref().and_then(|s| s.parse::<Color>().ok()).unwrap_or(fallback)
    };
    ThemeColors {
        statusbar_bg: color(&custom.statusbar_bg, CLASSIC.statusbar_bg),
        statusbar_fg: color(&custom.statusbar_fg, CLASSIC.statusbar_fg),
        timestamp: color(&custom.timestamp, CLASSIC.timestamp),
        join: color(&custom.join, CLASSIC.join),
        leave: color(&custom.leave, CLASSIC.leave),
        notice: color(&custom.notice, CLASSIC.notice),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_matches_irusts_original_hardcoded_colors() {
        assert_eq!(CLASSIC.statusbar_bg, Color::Blue);
        assert_eq!(CLASSIC.statusbar_fg, Color::White);
        assert_eq!(CLASSIC.timestamp, Color::Indexed(244));
        assert_eq!(CLASSIC.join, Color::Green);
        assert_eq!(CLASSIC.leave, Color::Red);
        assert_eq!(CLASSIC.notice, Color::Magenta);
    }

    #[test]
    fn the_three_presets_are_actually_distinct() {
        assert_ne!(CLASSIC, VIVID);
        assert_ne!(CLASSIC, MONO);
        assert_ne!(VIVID, MONO);
    }

    #[test]
    fn custom_theme_overrides_only_the_fields_it_sets() {
        let custom = irust_config::CustomTheme {
            leave: Some("#ff5500".to_string()),
            ..Default::default()
        };
        let colors = from_custom(&custom);
        assert_eq!(colors.leave, Color::Rgb(0xff, 0x55, 0x00));
        // Every other field falls back to classic's own value, not
        // some blank/default -- a custom theme overriding one color
        // shouldn't turn every other line invisible/black.
        assert_eq!(colors.statusbar_bg, CLASSIC.statusbar_bg);
        assert_eq!(colors.statusbar_fg, CLASSIC.statusbar_fg);
        assert_eq!(colors.timestamp, CLASSIC.timestamp);
        assert_eq!(colors.join, CLASSIC.join);
        assert_eq!(colors.notice, CLASSIC.notice);
    }

    #[test]
    fn custom_theme_accepts_named_and_indexed_colors() {
        let custom = irust_config::CustomTheme {
            statusbar_bg: Some("cyan".to_string()),
            join: Some("203".to_string()),
            ..Default::default()
        };
        let colors = from_custom(&custom);
        assert_eq!(colors.statusbar_bg, Color::Cyan);
        assert_eq!(colors.join, Color::Indexed(203));
    }

    #[test]
    fn custom_theme_falls_back_to_classic_on_an_unparseable_color() {
        let custom = irust_config::CustomTheme {
            notice: Some("not-a-real-color".to_string()),
            ..Default::default()
        };
        let colors = from_custom(&custom);
        assert_eq!(colors.notice, CLASSIC.notice);
    }
}
