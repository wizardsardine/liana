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
