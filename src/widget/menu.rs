//based off example from https://github.com/preiter93/tui-widget-list

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};

const INDICATOR: &char = &'*';

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub text: String,
    pub style: Style,
    pub indicated: bool,
    pub selector: &'static char,
}

impl MenuItem {
    pub fn new<T: Into<String>>(text: T, indicated: bool) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
            indicated,
            selector: &' ',
        }
    }
    pub fn new_selected<T: Into<String>>(
        text: T,
        indicated: bool,
        selector: &'static char,
    ) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
            indicated,
            selector,
        }
    }
}

impl Widget for MenuItem {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let indicator: &char = if self.indicated { INDICATOR } else { &' ' };
        Line::default()
            .spans(vec![
                Span::raw(format!("{}{}", self.selector, indicator)),
                Span::styled(self.text, self.style),
            ])
            .render(area, buf);
    }
}
