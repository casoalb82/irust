use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, topic: Option<&str>) {
    let text = match topic {
        Some(t) => format!(" {t}"),
        None => format!(" irust v{}", env!("CARGO_PKG_VERSION")),
    };
    let theme = crate::theme::current();
    let style = Style::default()
        .bg(theme.statusbar_bg)
        .fg(theme.statusbar_fg)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Paragraph::new(Line::styled(text, style)), area);
}
