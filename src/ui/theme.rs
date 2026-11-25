//! Color theme for Euler UI elements.

use crossterm::style::Color;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Theme {
    pub dir_color: Color,
    pub file_color: Color,
    pub symlink_color: Color,
    pub hidden_color: Color,
    pub line_number_color: Color,
    pub cursor_line_bg: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub border_color: Color,

    // Diff colors
    pub create_color: Color,
    pub rename_color: Color,
    pub move_color: Color,
    pub delete_color: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            dir_color: Color::Cyan,
            file_color: Color::White,
            symlink_color: Color::Magenta,
            hidden_color: Color::DarkGrey,
            line_number_color: Color::DarkGrey,
            cursor_line_bg: Color::DarkGrey,
            status_bg: Color::Blue,
            status_fg: Color::Black,
            border_color: Color::Yellow,

            create_color: Color::Green,
            rename_color: Color::Yellow,
            move_color: Color::Cyan,
            delete_color: Color::Red,
        }
    }
}
