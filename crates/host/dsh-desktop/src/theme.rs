use zsui::{Color, TextStyle, TextWeight, TextWrap, VerticalAlign};
pub fn rgb(v: u32) -> Color {
    Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}
#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub base: Color,
    pub sidebar: Color,
    pub ink: Color,
    pub secondary: Color,
    pub muted: Color,
    pub accent: Color,
    pub border: Color,
    pub selected: Color,
    pub bubble: Color,
    pub card: Color,
    pub danger: Color,
}
impl Theme {
    pub fn new(dark: bool) -> Self {
        let c = if dark {
            [
                0x1b1b1c, 0x151517, 0xf1f3f5, 0xadb2b8, 0x81858c, 0x679efe, 0x353638, 0x2c2c2e,
                0x283142, 0x232324, 0xf25a5a,
            ]
        } else {
            [
                0xffffff, 0xf9fafb, 0x0f1115, 0x61666b, 0x81858c, 0x4176e6, 0xe5e5e5, 0xedeef0,
                0xedf3fe, 0xf5f6f7, 0xec1313,
            ]
        };
        Self {
            dark,
            base: rgb(c[0]),
            sidebar: rgb(c[1]),
            ink: rgb(c[2]),
            secondary: rgb(c[3]),
            muted: rgb(c[4]),
            accent: rgb(c[5]),
            border: rgb(c[6]),
            selected: rgb(c[7]),
            bubble: rgb(c[8]),
            card: rgb(c[9]),
            danger: rgb(c[10]),
        }
    }
    pub fn text(&self, size: f32, color: Color, bold: bool, mono: bool) -> TextStyle {
        let mut s = TextStyle::line(if mono { "Consolas" } else { "Segoe UI" }, size, color);
        s.line_height = if mono { 22. } else { size * 1.6 };
        s.weight = if bold {
            TextWeight::Semibold
        } else {
            TextWeight::Regular
        };
        s.wrap = TextWrap::Word;
        s.ellipsis = false;
        s.vertical_align = VerticalAlign::Start;
        s
    }
}
