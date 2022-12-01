use iced::Color;

pub const BACKGROUND: Color = Color::from_rgb(
    0xF6 as f32 / 255.0,
    0xF7 as f32 / 255.0,
    0xF8 as f32 / 255.0,
);

pub const BORDER_GREY: Color = Color::from_rgb(
    0xd0 as f32 / 255.0,
    0xd7 as f32 / 255.0,
    0xde as f32 / 255.0,
);

pub const FOREGROUND: Color = Color::WHITE;

pub const PRIMARY: Color = Color::BLACK;

pub const SECONDARY: Color = DARK_GREY;

pub const SUCCESS: Color = Color::from_rgb(
    0x29 as f32 / 255.0,
    0xBC as f32 / 255.0,
    0x97 as f32 / 255.0,
);

#[allow(dead_code)]
pub const SUCCESS_LIGHT: Color = Color::from_rgba(
    0x29 as f32 / 255.0,
    0xBC as f32 / 255.0,
    0x97 as f32 / 255.0,
    0.5f32,
);

pub const ALERT: Color = Color::from_rgb(
    0xF0 as f32 / 255.0,
    0x43 as f32 / 255.0,
    0x59 as f32 / 255.0,
);

pub const ALERT_LIGHT: Color = Color::from_rgba(
    0xF0 as f32 / 255.0,
    0x43 as f32 / 255.0,
    0x59 as f32 / 255.0,
    0.5f32,
);

pub const WARNING: Color =
    Color::from_rgb(0xFF as f32 / 255.0, 0xa7 as f32 / 255.0, 0x0 as f32 / 255.0);

pub const WARNING_LIGHT: Color = Color::from_rgba(
    0xFF as f32 / 255.0,
    0xa7 as f32 / 255.0,
    0x0 as f32 / 255.0,
    0.5f32,
);

pub const CANCEL: Color = Color::from_rgb(
    0x34 as f32 / 255.0,
    0x37 as f32 / 255.0,
    0x3D as f32 / 255.0,
);

pub const INFO: Color = Color::from_rgb(
    0x2A as f32 / 255.0,
    0x98 as f32 / 255.0,
    0xBD as f32 / 255.0,
);

pub const INFO_LIGHT: Color = Color::from_rgba(
    0x2A as f32 / 255.0,
    0x98 as f32 / 255.0,
    0xBD as f32 / 255.0,
    0.5f32,
);

pub const DARK_GREY: Color = Color::from_rgb(
    0x8c as f32 / 255.0,
    0x97 as f32 / 255.0,
    0xa6 as f32 / 255.0,
);
