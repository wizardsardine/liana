#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u16)]
pub enum VSpacing {
    XS = 4,
    S = 8,
    M = 16,
    L = 20,
    XL = 24,
}

impl VSpacing {
    pub const fn pixels(self) -> u32 {
        self as u32
    }
}

impl From<VSpacing> for iced::Pixels {
    fn from(spacing: VSpacing) -> Self {
        spacing.pixels().into()
    }
}

impl From<VSpacing> for iced::Length {
    fn from(spacing: VSpacing) -> Self {
        iced::Length::Fixed(spacing.pixels() as f32)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u16)]
pub enum HSpacing {
    XS = 4,
    S = 5,
    M = 10,
    ML = 12,
    L = 16,
}

impl HSpacing {
    pub const fn pixels(self) -> u32 {
        self as u32
    }
}

impl From<HSpacing> for iced::Pixels {
    fn from(spacing: HSpacing) -> Self {
        spacing.pixels().into()
    }
}

impl From<HSpacing> for iced::Length {
    fn from(spacing: HSpacing) -> Self {
        iced::Length::Fixed(spacing.pixels() as f32)
    }
}
