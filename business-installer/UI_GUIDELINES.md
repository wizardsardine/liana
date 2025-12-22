# Business Installer UI Guidelines

This document describes UI patterns, components, and conventions for the `business-installer` crate.

## Component Library

### Import Pattern

Always use `liana_ui` components instead of raw iced widgets:

```rust
use liana_ui::{
    component::{button, text, form},
    widget::*,
    theme,
    icon,
};
```

### Available Components

```
+----------------------+------------------------------------------+
| Component            | Usage                                    |
+----------------------+------------------------------------------+
| button::primary      | Main action buttons                      |
| button::secondary    | Alternative actions                      |
| button::transparent  | Navigation, subtle actions               |
| text::h1/h2/h3       | Headings                                 |
| text::p1_regular     | Body text                                |
| text::text           | Generic text                             |
| form::Value          | Form field state (value, warning, valid) |
| icon::*              | Icon components                          |
+----------------------+------------------------------------------+
```

### Theme Styles

```rust
// Container styles
theme::container::background    // Main background
theme::card::simple             // Card containers

// Button styles
theme::button::container_border // Bordered button (for cards)

// Text styles
theme::text::success            // Success/positive text
```

## Page Layout

Use the `layout()` helper in `views/mod.rs` for consistent page structure:

```rust
pub fn my_view(state: &State) -> Element<'_, Message> {
    // Build breadcrumb path
    let breadcrumb = vec![
        "Organization".to_string(),
        "Wallet".to_string(),
        "Current View".to_string(),
    ];

    layout(
        (current_step, total_steps),  // Progress indicator (0, 0) to hide
        Some("user@email.com"),       // Email display (None to hide)
        None,                         // Role badge (Some("WS Manager") to show)
        &breadcrumb,                  // Breadcrumb path segments
        content,                      // Main content Element
        true,                         // padding_left: center content
        Some(Message::NavigateBack),  // Previous button action (None to disable)
    )
}
```

### Breadcrumb Navigation

The header displays a breadcrumb path showing the navigation hierarchy:
- All segments use the same font size (h3 style)
- Segments are separated by `>` in secondary/muted style
- Example: `Acme Corp > My Wallet > Template`

Common patterns:
- Login: `["Login"]`
- Org Select: `["Organization"]`
- Wallet Select: `[org_name, "Wallet"]`
- Template Builder: `[org_name, wallet_name, "Template"]`
- Keys Management: `[org_name, wallet_name, "Keys"]`

### Layout Structure

```
+----------------------------------------------------------+
|                                    [user@email.com]      |
+----------------------------------------------------------+
|                                                          |
| [< Previous]   Org > Wallet > Current View     [1 | 3]   |
|                                                          |
+----------------------------------------------------------+
|                                                          |
|                     [ Content Area ]                     |
|                                                          |
+----------------------------------------------------------+
```

## Menu Entries

Use `menu_entry()` for clickable card components (org/wallet selection):

```rust
use crate::views::menu_entry;

menu_entry(
    text::h3("Organization Name").into(),
    Some(Message::OrgSelected(org_id)),
)
```

### Menu Entry Appearance

- Fixed width: 500px
- Fixed height: 80px
- Styled as bordered card
- Centers content vertically and horizontally

## Status Badges

### Xpub Population Status

For displaying whether a key has xpub information:

```rust
fn xpub_status_badge(has_xpub: bool) -> Element<'static, Msg> {
    if has_xpub {
        Container::new(text::caption("✓ Populated"))
            .padding([4, 12])
            .style(liana_ui::theme::pill::success)
    } else {
        Container::new(text::caption("⚠ Missing"))
            .padding([4, 12])
            .style(liana_ui::theme::pill::warning)
    }
}
```

Success (green) for populated xpubs, warning (yellow) for missing xpubs.

### Wallet Status Badges

For displaying wallet status (Draft/Validated/Final):

```rust
// Draft: Yellow warning pill
Container::new(text::caption("Draft"))
    .style(liana_ui::theme::pill::warning)

// Validated: Blue info pill
Container::new(text::caption("Validated"))
    .style(liana_ui::theme::pill::info)

// Final: Green success pill
Container::new(text::caption("Final"))
    .style(liana_ui::theme::pill::success)
```

## Modal Pattern

### Modal State Structure

Define modal state in the appropriate view state file:

```rust
// In state/views/keys/mod.rs
#[derive(Debug, Clone)]
pub struct EditKeyModalState {
    pub key_id: u8,
    pub alias: String,
    pub description: String,
    pub email: String,
    pub key_type: KeyType,
}

#[derive(Debug, Clone, Default)]
pub struct KeysViewState {
    pub edit_key: Option<EditKeyModalState>,
}
```

### Opening a Modal

Set the modal state to `Some(...)`:

```rust
fn on_key_edit(&mut self, key_id: u8) {
    if let Some(key) = self.app.keys.get(&key_id) {
        self.views.keys.edit_key = Some(EditKeyModalState {
            key_id,
            alias: key.alias.clone(),
            description: key.description.clone(),
            email: key.email.clone(),
            key_type: key.key_type,
        });
    }
}
```

### Rendering Modals

Modals are rendered via `modals::render_modals()` in `views/modals/mod.rs`:

```rust
pub fn render_modals(state: &State) -> Option<Element<'_, Message>> {
    // Warning modal has priority (rendered on top)
    if let Some(warning) = &state.views.modals.warning {
        return Some(warning_modal(warning));
    }
    // Conflict modal
    if let Some(conflict) = &state.views.modals.conflict {
        return Some(conflict_modal(conflict));
    }
    // Xpub modal
    if let Some(_) = &state.views.xpub.modal {
        return Some(xpub::render_modal(state)?);
    }
    // Key edit modal
    if let Some(edit_key) = &state.views.keys.edit_key {
        return Some(key_modal(edit_key, &state.app.keys));
    }
    // Path edit modal
    if let Some(edit_path) = &state.views.paths.edit_path {
        return Some(path_modal(edit_path, &state.app));
    }
    None
}
```

### Modal Overlay

In `State::view()`, wrap content with modal overlay:

```rust
if let Some(modal) = modals::render_modals(self) {
    let cancel_msg = if self.views.modals.warning.is_some() {
        Message::WarningCloseModal
    } else if self.views.keys.edit_key.is_some() {
        Message::KeyCancelModal
    } else {
        Message::TemplateCancelPathModal
    };
    Modal::new(content, modal).on_blur(Some(cancel_msg)).into()
} else {
    content
}
```

### Closing a Modal

Set the modal state to `None`:

```rust
fn on_key_cancel_modal(&mut self) {
    self.views.keys.edit_key = None;
}
```

### SelectKeySource-Style Modal Pattern (Xpub Entry)

The xpub entry modal uses the SelectKeySource UX pattern from liana-gui, which prominently displays hardware wallets with a collapsible "Other options" section:

```rust
// State structure
pub struct XpubEntryModalState {
    pub key_id: u8,
    pub xpub_source: XpubSource,  // Current source type
    pub xpub_input: String,
    pub hw_devices: Vec<...>,      // Hardware wallet list
    pub validation_error: Option<String>,
    pub processing: bool,
    pub options_collapsed: bool,   // Whether "Other options" is collapsed
    // ... other fields
}

pub enum XpubSource {
    HardwareWallet,
    LoadFromFile,
}
```

Rendering pattern:

```rust
fn xpub_modal(state: &State, modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    let mut content = Column::new().spacing(15);

    // Header with close button
    content = content.push(header_with_close_button());

    // Hardware wallet section (always visible)
    content = content.push(render_hw_section(state, modal_state));

    // "Other options" collapsible section
    content = content.push(render_other_options(modal_state));

    // Validation error display
    if let Some(error) = &modal_state.validation_error {
        content = content.push(error_text(error));
    }

    // Footer with Save/Clear/Cancel buttons
    content = content.push(render_footer_buttons(modal_state));

    card::modal(content).into()
}

fn render_other_options(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    let mut content = Column::new().spacing(10);

    // Collapsible header
    let header_text = if modal_state.options_collapsed {
        "Other options ▼"
    } else {
        "Other options ▲"
    };
    content = content.push(
        button::transparent(None, header_text)
            .on_press(Msg::XpubToggleOptions)
    );

    // Show file loading when expanded
    if !modal_state.options_collapsed {
        content = content.push(
            button::secondary(Some(icon::import_icon()), "Import extended public key file")
                .on_press(Msg::XpubLoadFromFile)
        );
    }

    content.into()
}
```

Collapsible section pattern:
```rust
Msg::XpubToggleOptions => {
    if let Some(modal) = &mut self.views.xpub.modal {
        modal.options_collapsed = !modal.options_collapsed;
    }
}
```

## Form Handling

### Form State

Use `liana_ui::component::form::Value` for form fields:

```rust
use liana_ui::component::form;

pub struct EmailState {
    pub form: form::Value<'static>,
    pub processing: bool,
}

impl EmailState {
    pub fn new() -> Self {
        Self {
            form: form::Value {
                value: String::new(),
                warning: None,
                valid: true,
            },
            processing: false,
        }
    }
}
```

### Form Validation

Update form state on input changes:

```rust
fn on_update_email(&mut self, email: String) {
    self.email.form.valid = email_address::EmailAddress::parse_with_options(
        &email,
        email_address::Options::default().with_required_tld(),
    ).is_ok();
    self.email.form.warning = (!self.email.form.valid).then_some("Invalid email!");
    self.email.form.value = email;
}
```

### Displaying Warnings

Form warnings are displayed automatically by form components when `warning` is `Some(...)`.

## Button Patterns

### Primary Actions

```rust
button::primary(None, "Save")
    .on_press(Message::KeySave)
    .width(Length::Fixed(200.0))
```

### Secondary Actions

```rust
button::secondary(None, "Cancel")
    .on_press(Message::KeyCancelModal)
```

### Navigation Buttons

```rust
button::transparent(Some(icon::previous_icon()), "Previous")
    .on_press(Message::NavigateBack)
```

### Disabled Buttons

Omit `.on_press()` to disable:

```rust
let mut btn = button::primary(None, "Continue");
if form_is_valid {
    btn = btn.on_press(Message::Continue);
}
```

## Spacing and Layout

### Common Spacing

```rust
Space::with_height(Length::Fixed(100.0))  // Large vertical gap
Space::with_width(Length::Fill)           // Flexible horizontal fill
Space::with_width(Length::FillPortion(2)) // Proportional width
```

### Column Layout

```rust
Column::new()
    .spacing(10)
    .push(text::h3("Title"))
    .push(Space::with_height(20))
    .push(content)
```

### Row Layout

```rust
Row::new()
    .align_y(Alignment::Center)
    .push(Container::new(left).width(Length::FillPortion(2)))
    .push(Container::new(center).width(Length::FillPortion(8)))
    .push(Container::new(right).width(Length::FillPortion(2)))
```

## Scrollable Content

Wrap long content in scrollable:

```rust
use iced::widget::scrollable;

Container::new(scrollable(
    Column::new()
        .push(header)
        .push(content)
))
```

## Icons

Available icons from `liana_ui::icon`:

```rust
icon::previous_icon()  // Back navigation
icon::plus_icon()      // Add action
icon::trash_icon()     // Delete action
icon::pencil_icon()    // Edit action
```

## View State Updates

View-specific state handlers should be methods on the view state struct:

```rust
// In state/views/keys/mod.rs
impl KeysViewState {
    pub fn on_key_cancel_modal(&mut self) {
        self.edit_key = None;
    }

    pub fn on_key_update_alias(&mut self, value: String) {
        if let Some(ref mut edit) = self.edit_key {
            edit.alias = value;
        }
    }
}
```

Called from `State::update()`:

```rust
Msg::KeyCancelModal => self.views.keys.on_key_cancel_modal(),
Msg::KeyUpdateAlias(v) => self.views.keys.on_key_update_alias(v),
```

## Role-Based View Filtering

Filter content based on user role (WSManager, Owner, Participant):

```rust
// Get current user role
let user_role = &state.app.current_user_role;

// Filter keys for Participants
let filtered_keys: Vec<(u8, &Key)> = state.app.keys.iter()
    .filter(|(_id, key)| {
        match user_role.as_ref() {
            Some(UserRole::Participant) => {
                // Participants only see their own keys
                key.email.to_lowercase() == current_user_email.to_lowercase()
            }
            Some(UserRole::WSManager) | Some(UserRole::Owner) | None => {
                // WSManager and Owner see all keys
                true
            }
        }
    })
    .map(|(id, key)| (*id, key))
    .collect();
```

Role badge display:
```rust
let role_badge = if matches!(user_role, Some(UserRole::WSManager)) {
    Some("WS Manager")
} else {
    None
};
```

