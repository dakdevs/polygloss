// path: crates/polygloss-app/src/space.rs

// accept: the token modules themselves
pub mod layout {
    pub const SIDEBAR_WIDTH: f32 = 280.0;
    pub const ALL: &[(&str, &[f32])] = &[("SIDEBAR_WIDTH", &[SIDEBAR_WIDTH])];
}
pub trait TextStyleExt: Styled {
    fn text_style(self, style: text::Style) -> Self {
        self.text_size(px(style.0)).line_height(px(style.1))
    }
}
