use ratatui::style::Color;

const PALETTE: &[Color] = &[
    Color::Blue,
    Color::Green,
    Color::Red,
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::LightRed,
    Color::LightGreen,
    Color::LightCyan,
    Color::LightMagenta,
    Color::LightYellow,
];

pub fn len() -> usize {
    PALETTE.len()
}

pub fn color_for_index(index: usize) -> Color {
    PALETTE[index % PALETTE.len()]
}

/// A colour from a `hilight_color`-style setting: an irssi format code
/// (`%Y` bright yellow, `%m` dark magenta, ...; lowercase = normal,
/// uppercase = bright), or anything ratatui parses (names, `#rrggbb`,
/// 0-255). `None` for "" or something unparseable, so the caller falls back
/// to its built-in colour.
pub fn from_setting(text: &str) -> Option<Color> {
    let t = text.trim();
    if let Some(code) = t.strip_prefix('%') {
        return Some(match code {
            "k" => Color::Black,
            "K" => Color::DarkGray,
            "r" => Color::Red,
            "R" => Color::LightRed,
            "g" => Color::Green,
            "G" => Color::LightGreen,
            "y" => Color::Yellow,
            "Y" => Color::LightYellow,
            "b" => Color::Blue,
            "B" => Color::LightBlue,
            "m" | "p" => Color::Magenta,
            "M" | "P" => Color::LightMagenta,
            "c" => Color::Cyan,
            "C" => Color::LightCyan,
            "w" => Color::Gray,
            "W" => Color::White,
            _ => return None,
        });
    }
    t.parse::<Color>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irssi_codes_map_lowercase_normal_uppercase_bright() {
        assert_eq!(from_setting("%y"), Some(Color::Yellow));
        assert_eq!(from_setting("%Y"), Some(Color::LightYellow));
        assert_eq!(from_setting("%M"), Some(Color::LightMagenta));
        assert_eq!(from_setting("%K"), Some(Color::DarkGray));
        assert_eq!(from_setting("%Z"), None);
    }

    #[test]
    fn names_hex_and_indexes_go_through_ratatui() {
        assert_eq!(from_setting("red"), Some(Color::Red));
        assert_eq!(from_setting("#ff8800"), Some(Color::Rgb(255, 136, 0)));
        assert_eq!(from_setting("208"), Some(Color::Indexed(208)));
        assert_eq!(from_setting(""), None);
    }
}
