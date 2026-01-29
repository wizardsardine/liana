# Liana Business Theme - Detailed Implementation Plan

## Overview
Create a **light theme** for liana-business using **cyan-blue** (`#00BFFF`) as the primary accent color, matching the lianawallet.com/business branding.

---

## 1. Color Definitions

### File: `liana-ui/src/color.rs`

Add the following new color constants:

```rust
// =============================================================================
// BUSINESS THEME COLORS (Light Mode with Cyan-Blue accent)
// =============================================================================

// Primary accent: Cyan-Blue from lianawallet.com/business (HSL 196, 100%, 50%)
pub const BUSINESS_BLUE: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0xBF as f32 / 255.0,
    0xFF as f32 / 255.0,
);  // #00BFFF

// Darker variant for hover states (HSL 196, 100%, 40%)
pub const BUSINESS_BLUE_DARK: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0x99 as f32 / 255.0,
    0xCC as f32 / 255.0,
);  // #0099CC

// Transparent variant for highlights
pub const TRANSPARENT_BUSINESS_BLUE: Color = Color::from_rgba(
    0x00 as f32 / 255.0,
    0xBF as f32 / 255.0,
    0xFF as f32 / 255.0,
    0.15,
);

// Light theme backgrounds
pub const LIGHT_BG: Color = Color::from_rgb(
    0xFF as f32 / 255.0,
    0xFF as f32 / 255.0,
    0xFF as f32 / 255.0,
);  // #FFFFFF - main background

pub const LIGHT_BG_SECONDARY: Color = Color::from_rgb(
    0xF9 as f32 / 255.0,
    0xF9 as f32 / 255.0,
    0xF9 as f32 / 255.0,
);  // #F9F9F9 - cards, panels

pub const LIGHT_BG_TERTIARY: Color = Color::from_rgb(
    0xF0 as f32 / 255.0,
    0xF0 as f32 / 255.0,
    0xF0 as f32 / 255.0,
);  // #F0F0F0 - disabled states

// Light theme text colors
pub const DARK_TEXT_PRIMARY: Color = Color::from_rgb(
    0x1A as f32 / 255.0,
    0x1A as f32 / 255.0,
    0x1A as f32 / 255.0,
);  // #1A1A1A - primary text

pub const DARK_TEXT_SECONDARY: Color = Color::from_rgb(
    0x6B as f32 / 255.0,
    0x6B as f32 / 255.0,
    0x6B as f32 / 255.0,
);  // #6B6B6B - secondary text

pub const DARK_TEXT_TERTIARY: Color = Color::from_rgb(
    0x9E as f32 / 255.0,
    0x9E as f32 / 255.0,
    0x9E as f32 / 255.0,
);  // #9E9E9E - placeholders

// Light theme borders
pub const LIGHT_BORDER: Color = Color::from_rgb(
    0xD9 as f32 / 255.0,
    0xD9 as f32 / 255.0,
    0xD9 as f32 / 255.0,
);  // #D9D9D9 - borders

pub const LIGHT_BORDER_STRONG: Color = Color::from_rgb(
    0xB3 as f32 / 255.0,
    0xB3 as f32 / 255.0,
    0xB3 as f32 / 255.0,
);  // #B3B3B3 - stronger borders
```

---

## 2. Business Palette Implementation

### File: `liana-ui/src/theme/palette.rs`

Add a new method `Palette::business()` after the `Default` implementation:

```rust
impl Palette {
    /// Business theme: Light mode with cyan-blue accent
    pub fn business() -> Self {
        Self {
            // ... detailed below
        }
    }
}
```

### 2.1 General Section
```rust
general: General {
    background: color::LIGHT_BG,           // #FFFFFF (was LIGHT_BLACK #141414)
    foreground: color::LIGHT_BG_SECONDARY, // #F9F9F9 (was BLACK #000000)
    scrollable: color::LIGHT_BORDER,       // #D9D9D9 (was GREY_7 #3F3F3F)
},
```

### 2.2 Text Section
```rust
text: Text {
    primary: color::DARK_TEXT_PRIMARY,     // #1A1A1A (was WHITE)
    secondary: color::DARK_TEXT_SECONDARY, // #6B6B6B (was GREY_2 #CCCCCC)
    warning: color::ORANGE,                // Keep #FFA700
    success: color::GREEN,                 // Keep #00FF66
    error: color::RED,                     // Keep #E24E1B
},
```

### 2.3 Buttons Section

#### 2.3.1 Primary Button (main CTA)
```rust
primary: Button {
    active: ButtonPalette {
        background: color::BUSINESS_BLUE,      // #00BFFF (was GREEN)
        text: color::WHITE,                    // White on blue
        border: color::BUSINESS_BLUE.into(),
    },
    hovered: ButtonPalette {
        background: color::BUSINESS_BLUE_DARK, // #0099CC (darker on hover)
        text: color::WHITE,
        border: color::BUSINESS_BLUE_DARK.into(),
    },
    pressed: Some(ButtonPalette {
        background: color::BUSINESS_BLUE_DARK,
        text: color::WHITE,
        border: color::BUSINESS_BLUE_DARK.into(),
    }),
    disabled: Some(ButtonPalette {
        background: color::LIGHT_BG_TERTIARY,  // #F0F0F0
        text: color::DARK_TEXT_TERTIARY,       // #9E9E9E
        border: color::LIGHT_BORDER.into(),
    }),
},
```

#### 2.3.2 Secondary Button
```rust
secondary: Button {
    active: ButtonPalette {
        background: color::LIGHT_BG_SECONDARY, // #F9F9F9
        text: color::DARK_TEXT_SECONDARY,      // #6B6B6B
        border: color::LIGHT_BORDER.into(),    // #D9D9D9
    },
    hovered: ButtonPalette {
        background: color::LIGHT_BG_SECONDARY,
        text: color::BUSINESS_BLUE,            // Blue text on hover
        border: color::BUSINESS_BLUE.into(),
    },
    pressed: Some(ButtonPalette {
        background: color::LIGHT_BG_SECONDARY,
        text: color::BUSINESS_BLUE,
        border: color::BUSINESS_BLUE.into(),
    }),
    disabled: Some(ButtonPalette {
        background: color::LIGHT_BG_TERTIARY,
        text: color::DARK_TEXT_TERTIARY,
        border: color::LIGHT_BORDER.into(),
    }),
},
```

#### 2.3.3 Destructive Button
```rust
destructive: Button {
    active: ButtonPalette {
        background: color::LIGHT_BG_SECONDARY,
        text: color::RED,
        border: color::RED.into(),
    },
    hovered: ButtonPalette {
        background: color::RED,
        text: color::WHITE,
        border: color::RED.into(),
    },
    pressed: Some(ButtonPalette {
        background: color::RED,
        text: color::WHITE,
        border: color::RED.into(),
    }),
    disabled: Some(ButtonPalette {
        background: color::LIGHT_BG_TERTIARY,
        text: color::RED,
        border: color::RED.into(),
    }),
},
```

#### 2.3.4 Transparent Button
```rust
transparent: Button {
    active: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_SECONDARY,
        border: None,
    },
    hovered: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_PRIMARY,
        border: None,
    },
    pressed: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_PRIMARY,
        border: None,
    }),
    disabled: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_TERTIARY,
        border: None,
    }),
},
```

#### 2.3.5 Transparent Border Button
```rust
transparent_border: Button {
    active: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_SECONDARY,
        border: color::TRANSPARENT.into(),
    },
    hovered: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::BUSINESS_BLUE,
        border: color::BUSINESS_BLUE.into(),
    },
    pressed: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::BUSINESS_BLUE,
        border: color::BUSINESS_BLUE.into(),
    }),
    disabled: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_TERTIARY,
        border: color::TRANSPARENT.into(),
    }),
},
```

#### 2.3.6 Container Button
```rust
container: Button {
    active: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_SECONDARY,
        border: None,
    },
    hovered: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_PRIMARY,
        border: None,
    },
    pressed: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_PRIMARY,
        border: None,
    }),
    disabled: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_TERTIARY,
        border: None,
    }),
},
```

#### 2.3.7 Container Border Button
```rust
container_border: Button {
    active: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_SECONDARY,
        border: color::TRANSPARENT.into(),
    },
    hovered: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::BUSINESS_BLUE,
        border: color::BUSINESS_BLUE.into(),
    },
    pressed: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::BUSINESS_BLUE,
        border: color::BUSINESS_BLUE.into(),
    }),
    disabled: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_TERTIARY,
        border: color::TRANSPARENT.into(),
    }),
},
```

#### 2.3.8 Menu Button
```rust
menu: Button {
    active: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_PRIMARY,
        border: color::TRANSPARENT.into(),
    },
    hovered: ButtonPalette {
        background: color::LIGHT_BG_SECONDARY,
        text: color::DARK_TEXT_PRIMARY,
        border: color::TRANSPARENT.into(),
    },
    pressed: Some(ButtonPalette {
        background: color::LIGHT_BG_SECONDARY,
        text: color::DARK_TEXT_PRIMARY,
        border: color::TRANSPARENT.into(),
    }),
    disabled: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_TERTIARY,
        border: color::TRANSPARENT.into(),
    }),
},
```

#### 2.3.9 Tab Button
```rust
tab: Button {
    active: ButtonPalette {
        background: color::LIGHT_BG_SECONDARY,
        text: color::DARK_TEXT_SECONDARY,
        border: color::LIGHT_BORDER.into(),
    },
    hovered: ButtonPalette {
        background: color::LIGHT_BG_SECONDARY,
        text: color::BUSINESS_BLUE,
        border: color::BUSINESS_BLUE.into(),
    },
    pressed: Some(ButtonPalette {
        background: color::LIGHT_BG,
        text: color::BUSINESS_BLUE,
        border: color::BUSINESS_BLUE.into(),
    }),
    disabled: Some(ButtonPalette {
        background: color::LIGHT_BG_TERTIARY,
        text: color::DARK_TEXT_TERTIARY,
        border: color::LIGHT_BORDER.into(),
    }),
},
```

#### 2.3.10 Link Button
```rust
link: Button {
    active: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_SECONDARY,
        border: color::TRANSPARENT.into(),
    },
    hovered: ButtonPalette {
        background: color::TRANSPARENT,
        text: color::BUSINESS_BLUE,
        border: color::TRANSPARENT.into(),
    },
    pressed: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::BUSINESS_BLUE,
        border: color::TRANSPARENT.into(),
    }),
    disabled: Some(ButtonPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_TERTIARY,
        border: color::TRANSPARENT.into(),
    }),
},
```

### 2.4 Cards Section
```rust
cards: Cards {
    simple: ContainerPalette {
        background: color::LIGHT_BG_SECONDARY,  // #F9F9F9 (was GREY_6)
        text: None,
        border: Some(color::TRANSPARENT),
    },
    modal: ContainerPalette {
        background: color::LIGHT_BG,            // White (was LIGHT_BLACK)
        text: None,
        border: color::LIGHT_BORDER.into(),     // Add subtle border for light theme
    },
    border: ContainerPalette {
        background: color::TRANSPARENT,
        text: None,
        border: color::LIGHT_BORDER.into(),     // #D9D9D9 (was GREY_7)
    },
    invalid: ContainerPalette {
        background: color::LIGHT_BG,
        text: color::RED.into(),
        border: color::RED.into(),
    },
    warning: ContainerPalette {
        background: color::LIGHT_BG,
        text: color::ORANGE.into(),
        border: color::ORANGE.into(),
    },
    error: ContainerPalette {
        background: color::LIGHT_BG,
        text: color::RED.into(),
        border: color::RED.into(),
    },
},
```

### 2.5 Banners Section
```rust
banners: Banners {
    network: ContainerPalette {
        background: color::BUSINESS_BLUE,       // Use brand blue (was BLUE)
        text: color::WHITE.into(),              // White text (was LIGHT_BLACK)
        border: None,
    },
    warning: ContainerPalette {
        background: color::ORANGE,
        text: color::WHITE.into(),              // White text (was LIGHT_BLACK)
        border: None,
    },
},
```

### 2.6 Badges Section
```rust
badges: Badges {
    simple: ContainerPalette {
        background: color::LIGHT_BG_TERTIARY,   // #F0F0F0 (was GREY_4)
        text: None,
        border: color::TRANSPARENT.into(),
    },
    bitcoin: ContainerPalette {
        background: color::ORANGE,
        text: color::WHITE.into(),
        border: color::TRANSPARENT.into(),
    },
},
```

### 2.7 Pills Section
```rust
pills: Pills {
    primary: ContainerPalette {
        background: color::BUSINESS_BLUE,       // Brand blue (was GREEN)
        text: color::WHITE.into(),              // White (was LIGHT_BLACK)
        border: color::TRANSPARENT.into(),
    },
    simple: ContainerPalette {
        background: color::TRANSPARENT,
        text: color::DARK_TEXT_SECONDARY.into(), // #6B6B6B (was GREY_3)
        border: color::LIGHT_BORDER.into(),      // #D9D9D9 (was GREY_3)
    },
    warning: ContainerPalette {
        background: color::TRANSPARENT,
        text: color::RED.into(),
        border: color::RED.into(),
    },
    success: ContainerPalette {
        background: color::GREEN,
        text: color::DARK_TEXT_PRIMARY.into(),   // Dark text on green
        border: color::GREEN.into(),
    },
},
```

### 2.8 Notifications Section
```rust
notifications: Notifications {
    pending: ContainerPalette {
        background: color::BUSINESS_BLUE,       // Brand blue (was GREEN)
        text: color::WHITE.into(),
        border: Some(color::BUSINESS_BLUE),
    },
    error: ContainerPalette {
        background: color::ORANGE,
        text: color::WHITE.into(),
        border: Some(color::ORANGE),
    },
},
```

### 2.9 Text Inputs Section
```rust
text_inputs: TextInputs {
    primary: TextInput {
        active: TextInputPalette {
            background: color::LIGHT_BG,             // White background
            icon: color::DARK_TEXT_TERTIARY,         // #9E9E9E
            placeholder: color::DARK_TEXT_TERTIARY,  // #9E9E9E (was GREY_7)
            value: color::DARK_TEXT_PRIMARY,         // #1A1A1A (was GREY_2)
            selection: color::BUSINESS_BLUE,         // Brand blue (was GREEN)
            border: Some(color::LIGHT_BORDER),       // #D9D9D9 (was GREY_7)
        },
        disabled: TextInputPalette {
            background: color::LIGHT_BG_TERTIARY,    // #F0F0F0
            icon: color::DARK_TEXT_TERTIARY,
            placeholder: color::DARK_TEXT_TERTIARY,
            value: color::DARK_TEXT_SECONDARY,
            selection: color::BUSINESS_BLUE,
            border: Some(color::LIGHT_BORDER),
        },
    },
    invalid: TextInput {
        active: TextInputPalette {
            background: color::LIGHT_BG,
            icon: color::DARK_TEXT_TERTIARY,
            placeholder: color::DARK_TEXT_TERTIARY,
            value: color::DARK_TEXT_PRIMARY,
            selection: color::BUSINESS_BLUE,
            border: Some(color::RED),
        },
        disabled: TextInputPalette {
            background: color::LIGHT_BG_TERTIARY,
            icon: color::DARK_TEXT_TERTIARY,
            placeholder: color::DARK_TEXT_TERTIARY,
            value: color::TRANSPARENT,
            selection: color::BUSINESS_BLUE,
            border: Some(color::RED),
        },
    },
},
```

### 2.10 Checkboxes Section
```rust
checkboxes: Checkboxes {
    icon: color::BUSINESS_BLUE,                // Brand blue (was GREEN)
    text: color::DARK_TEXT_PRIMARY,            // #1A1A1A (was GREY_2)
    background: color::LIGHT_BG_SECONDARY,     // #F9F9F9 (was GREY_4)
    border: Some(color::LIGHT_BORDER),         // #D9D9D9 (was GREY_4)
},
```

### 2.11 Radio Buttons Section
```rust
radio_buttons: RadioButtons {
    dot: color::BUSINESS_BLUE,                 // Brand blue (was GREEN)
    text: color::DARK_TEXT_PRIMARY,            // #1A1A1A (was GREY_2)
    border: color::LIGHT_BORDER,               // #D9D9D9 (was GREY_7)
},
```

### 2.12 Sliders Section
```rust
sliders: Sliders {
    background: color::BUSINESS_BLUE,          // Brand blue (was GREEN)
    border: color::BUSINESS_BLUE,
    rail_border: None,
    rail_backgrounds: (color::BUSINESS_BLUE, color::LIGHT_BORDER), // (filled, empty)
},
```

### 2.13 Progress Bars Section
```rust
progress_bars: ProgressBars {
    bar: color::BUSINESS_BLUE,                 // Brand blue (was GREEN)
    border: color::TRANSPARENT.into(),
    background: color::LIGHT_BG_TERTIARY,      // #F0F0F0 (was GREY_6)
},
```

### 2.14 Rule (Divider)
```rust
rule: color::LIGHT_BORDER,                     // #D9D9D9 (was GREY_1)
```

### 2.15 Pane Grid Section
```rust
pane_grid: PaneGrid {
    background: color::LIGHT_BG_SECONDARY,     // #F9F9F9 (was BLACK)
    highlight_border: color::BUSINESS_BLUE,    // Brand blue (was GREEN)
    highlight_background: color::TRANSPARENT_BUSINESS_BLUE, // Transparent blue
    picked_split: color::BUSINESS_BLUE,        // Brand blue (was GREEN)
    hovered_split: color::BUSINESS_BLUE,       // Brand blue (was GREEN)
},
```

### 2.16 Togglers Section
```rust
togglers: Togglers {
    on: Toggler {
        background: color::BUSINESS_BLUE,      // Brand blue (was GREEN)
        background_border: color::BUSINESS_BLUE,
        foreground: color::WHITE,
        foreground_border: color::WHITE,
    },
    off: Toggler {
        background: color::LIGHT_BORDER,       // #D9D9D9 (was GREY_2)
        background_border: color::LIGHT_BORDER,
        foreground: color::WHITE,
        foreground_border: color::WHITE,
    },
},
```

---

## 3. Theme Constructor

### File: `liana-ui/src/theme/mod.rs`

Add constructor method:

```rust
impl Theme {
    /// Creates the Liana Business theme (light mode with cyan-blue accent)
    pub fn business() -> Self {
        Self {
            colors: palette::Palette::business(),
        }
    }
}
```

---

## 4. Apply Theme to liana-business

### File: `liana-business/src/main.rs`

**Line 57** - Change from:
```rust
.theme(|_| theme::Theme::default())
```

To:
```rust
.theme(|_| theme::Theme::business())
```

---

## 5. Update Hardcoded Colors in Business Views

### File: `liana-business/business-installer/src/views/template_builder/template_visualization.rs`

#### 5.1 Update PRIMARY_COLOR (line 92)
```rust
// From:
const PRIMARY_COLOR: &str = "#32cd32"; // Green

// To:
const PRIMARY_COLOR: &str = "#00BFFF"; // Business Blue
```

#### 5.2 Update get_secondary_color function (lines 106-141)
Change the color gradient from green→blue to blue→purple:

```rust
fn get_secondary_color(index: usize, total_count: usize) -> String {
    if total_count == 0 {
        return "#00BFFF".to_string(); // Default to business blue
    }

    let factor = if total_count == 1 {
        0.5
    } else {
        index as f32 / (total_count - 1) as f32
    };

    // Start color (business blue): RGB(0, 191, 255) = #00BFFF
    // End color (purple): RGB(128, 0, 255) = #8000FF
    let start_r = 0.0;
    let start_g = 191.0;
    let start_b = 255.0;

    let end_r = 128.0;
    let end_g = 0.0;
    let end_b = 255.0;

    let r = (start_r + (end_r - start_r) * factor) as u8;
    let g = (start_g + (end_g - start_g) * factor) as u8;
    let b = (start_b + (end_b - start_b) * factor) as u8;

    format!("#{:02x}{:02x}{:02x}", r, g, b)
}
```

#### 5.3 Update bordered_button_style function (lines 28-74)
Change `green_border` to use blue:

```rust
fn bordered_button_style(status: Status, radius: f32) -> Style {
    let grey_border = color::LIGHT_BORDER;      // Was GREY_7
    let accent_border = color::BUSINESS_BLUE;   // Was GREEN

    match status {
        Status::Active => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::DARK_TEXT_SECONDARY,  // Was GREY_2
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: grey_border,
            },
            ..Default::default()
        },
        Status::Hovered => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::BUSINESS_BLUE,        // Was GREEN
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: accent_border,
            },
            ..Default::default()
        },
        // ... similar updates for Pressed and Disabled
    }
}
```

#### 5.4 Update path_card_container_style function (lines 77-89)
```rust
fn path_card_container_style(_theme: &Theme) -> iced::widget::container::Style {
    use iced::widget::container;
    container::Style {
        text_color: Some(color::DARK_TEXT_SECONDARY),  // Was GREY_2
        background: Some(Background::Color(color::TRANSPARENT)),
        border: Border {
            radius: 25.0.into(),
            width: 1.0,
            color: color::LIGHT_BORDER,                // Was GREY_7
        },
        ..Default::default()
    }
}
```

---

## Summary of Files to Modify

| File | Lines Changed | Description |
|------|---------------|-------------|
| `liana-ui/src/color.rs` | +40 lines | Add 12 new color constants |
| `liana-ui/src/theme/palette.rs` | +420 lines | Add `Palette::business()` method |
| `liana-ui/src/theme/mod.rs` | +8 lines | Add `Theme::business()` constructor |
| `liana-business/src/main.rs` | 1 line | Change theme from default() to business() |
| `liana-business/.../template_visualization.rs` | ~50 lines | Update hardcoded colors |

---

## Color Transformation Summary

| Element | Dark Theme (Current) | Light Business Theme |
|---------|---------------------|---------------------|
| **Backgrounds** | | |
| Main background | `LIGHT_BLACK` #141414 | `LIGHT_BG` #FFFFFF |
| Card/Panel background | `BLACK` #000000 | `LIGHT_BG_SECONDARY` #F9F9F9 |
| Disabled background | `GREY_6` #202020 | `LIGHT_BG_TERTIARY` #F0F0F0 |
| **Text** | | |
| Primary text | `WHITE` #FFFFFF | `DARK_TEXT_PRIMARY` #1A1A1A |
| Secondary text | `GREY_2` #CCCCCC | `DARK_TEXT_SECONDARY` #6B6B6B |
| Placeholder text | `GREY_7` #3F3F3F | `DARK_TEXT_TERTIARY` #9E9E9E |
| **Accent** | | |
| Primary accent | `GREEN` #00FF66 | `BUSINESS_BLUE` #00BFFF |
| Accent hover | `GREEN` #00FF66 | `BUSINESS_BLUE_DARK` #0099CC |
| Accent transparent | `TRANSPARENT_GREEN` | `TRANSPARENT_BUSINESS_BLUE` |
| **Borders** | | |
| Default border | `GREY_7` #3F3F3F | `LIGHT_BORDER` #D9D9D9 |
| Strong border | `GREY_4` #424242 | `LIGHT_BORDER_STRONG` #B3B3B3 |
| **Semantic (unchanged)** | | |
| Error | `RED` #E24E1B | `RED` #E24E1B |
| Warning | `ORANGE` #FFA700 | `ORANGE` #FFA700 |
| Success | `GREEN` #00FF66 | `GREEN` #00FF66 |
