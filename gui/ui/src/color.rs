use iced::Color;

pub const BLACK: Color = iced::Color::BLACK;

pub const LIGHT_BLACK: Color = Color::from_rgb(
    0x14 as f32 / 255.0,
    0x14 as f32 / 255.0,
    0x14 as f32 / 255.0,
);

pub const GREEN: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0xFF as f32 / 255.0,
    0x66 as f32 / 255.0,
);

pub const DARK_GREY: Color = Color::from_rgb(
    0x55 as f32 / 255.0,
    0x55 as f32 / 255.0,
    0x55 as f32 / 255.0,
);

pub const GREY: Color = Color::from_rgb(
    0xCC as f32 / 255.0,
    0xCC as f32 / 255.0,
    0xCC as f32 / 255.0,
);

pub const LIGHT_GREY: Color = Color::from_rgb(
    0xE6 as f32 / 255.0,
    0xE6 as f32 / 255.0,
    0xE6 as f32 / 255.0,
);

pub const RED: Color = Color::from_rgb(
    0xF0 as f32 / 255.0,
    0x43 as f32 / 255.0,
    0x59 as f32 / 255.0,
);

pub const ORANGE: Color =
    Color::from_rgb(0xFF as f32 / 255.0, 0xa7 as f32 / 255.0, 0x0 as f32 / 255.0);

pub mod dark {
    use iced::Color;
    pub const BLACK: Color = iced::Color::BLACK;
    pub const LIGHT_BLACK: Color = Color::from_rgb(
        0x14 as f32 / 255.0,
        0x14 as f32 / 255.0,
        0x14 as f32 / 255.0,
    );
    pub const GREY_5: Color = Color::from_rgb(
        0x27 as f32 / 255.0,
        0x27 as f32 / 255.0,
        0x27 as f32 / 255.0,
    );
    pub const GREY_4: Color = Color::from_rgb(
        0x42 as f32 / 255.0,
        0x42 as f32 / 255.0,
        0x42 as f32 / 255.0,
    );
    pub const GREY_3: Color = Color::from_rgb(
        0x71 as f32 / 255.0,
        0x71 as f32 / 255.0,
        0x71 as f32 / 255.0,
    );
    pub const GREY_2: Color = Color::from_rgb(
        0xCC as f32 / 255.0,
        0xCC as f32 / 255.0,
        0xCC as f32 / 255.0,
    );
    pub const GREY_1: Color = Color::from_rgb(
        0xE6 as f32 / 255.0,
        0xE6 as f32 / 255.0,
        0xE6 as f32 / 255.0,
    );
    pub const WHITE: Color = iced::Color::WHITE;
    pub const GREEN: Color = Color::from_rgb(
        0x00 as f32 / 255.0,
        0xFF as f32 / 255.0,
        0x66 as f32 / 255.0,
    );
}

pub mod legacy {
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
}
