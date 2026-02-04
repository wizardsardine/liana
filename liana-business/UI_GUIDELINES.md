# Liana Business UI Guidelines

This document describes UI patterns, components, and conventions.

## Component Library

We use `liana_ui` components from the shared UI library:

```rust
use liana_ui::{
    component::{button, text, form, card},
    widget::*,
    theme,
    icon,
};
```

### Available Components

```
+---------------------+-------------------------------+
| Component           | Usage                         |
+---------------------+-------------------------------+
| button::primary     | Main action buttons           |
| button::secondary   | Alternative actions           |
| button::transparent | Navigation, subtle actions    |
| text::h1/h2/h3      | Headings                      |
| text::p1_regular    | Body text                     |
| text::caption       | Small text, badges            |
| form::Value         | Form field state              |
| card::simple        | Card containers               |
| icon::*             | Icon components               |
+---------------------+-------------------------------+
```

### Theme Styles

```rust
// Container styles
theme::container::background
theme::card::simple

// Button styles
theme::button::container_border

// Pill styles (for badges)
liana_ui::theme::pill::success   // Green
liana_ui::theme::pill::warning   // Yellow/amber
liana_ui::theme::pill::info      // Blue
```

## Page Layout

Use `layout()` or `layout_with_scrollable_list()` helpers in `views/mod.rs`:

```rust
pub fn my_view(state: &State) -> Element<'_, Message> {
    let breadcrumb = vec![
        "Organization".to_string(),
        "Wallet".to_string(),
        "Current View".to_string(),
    ];

    layout(
        (current_step, total_steps),  // Progress (0, 0) to hide
        Some("user@email.com"),       // Email display
        Some("WS Admin"),           // Role badge (None to hide)
        &breadcrumb,
        content,
        true,                         // padding_left: center content
        Some(Message::NavigateBack),  // Previous button
    )
}
```

### Layout Structure

```
+----------------------------------------------------------+
|                                    [user@email.com]      |
+----------------------------------------------------------+
|                                                          |
| [< Previous]   Org > Wallet > View               [1 | 3] |
|                                                          |
+----------------------------------------------------------+
|                                                          |
|                     [ Content Area ]                     |
|                                                          |
+----------------------------------------------------------+
```

### Breadcrumb Patterns

```
+------------------+------------------------------------------+
| View             | Breadcrumb                               |
+------------------+------------------------------------------+
| Login            | ["Login"]                                |
| Org Select       | ["Organizations"]                        |
| Wallet Select    | [org_name, "Wallets"]                    |
| Template Builder | [org_name, wallet_name, "Template"]      |
| Keys Management  | [org_name, wallet_name, "Keys"]          |
| Set Keys (Xpub)  | [org_name, wallet_name, "Set Keys"]      |
+------------------+------------------------------------------+
```

## Menu Entries

Use `menu_entry()` for clickable cards (org/wallet selection):

```rust
menu_entry(
    text::h3("Organization Name").into(),
    Some(Message::OrgSelected(org_id)),
)
```

Dimensions: 500px width × 80px height, bordered card style.

## Card Entries

Use `card_entry()` for cards with grey background and shadow (keys, paths, xpub):

```rust
card_entry(
    content.into(),
    Some(Message::KeyEdit(key_id)),  // None for read-only
    600.0,  // width
)
```

Features:
- Grey background (`LIGHT_BG_SECONDARY`)
- Shadow effect
- Blue border on hover (via `theme::button::container_border`)
- Pass `None` as message for read-only display

## Status Badges

### Wallet Status

```rust
// Draft
Container::new(text::caption("Draft"))
    .style(liana_ui::theme::pill::warning)

// Validated / Set Keys
Container::new(text::caption("Set Keys"))
    .style(liana_ui::theme::pill::warning)

// Final / Active
Container::new(text::caption("Active"))
    .style(liana_ui::theme::pill::success)
```

### Xpub Population Status

```rust
// Set (populated)
Container::new(text::caption("Set"))
    .style(liana_ui::theme::pill::success)

// Not Set (missing)
Container::new(text::caption("Not Set"))
    .style(liana_ui::theme::pill::warning)
```

## Modal Pattern

### Modal State Structure

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

```rust
fn on_key_edit(&mut self, key_id: u8) {
    if let Some(key) = self.app.keys.get(&key_id) {
        self.views.keys.edit_key = Some(EditKeyModalState {
            key_id,
            alias: key.alias.clone(),
            // ...
        });
    }
}
```

### Rendering Modals

Modal priority in `views/modals/mod.rs`:

```rust
pub fn render_modals(state: &State) -> Option<Element<'_, Message>> {
    // 1. Warning modal (highest priority)
    if let Some(warning) = &state.views.modals.warning {
        return Some(warning_modal(warning));
    }
    // 2. Conflict modal
    if let Some(conflict) = &state.views.modals.conflict {
        return Some(conflict_modal(conflict));
    }
    // 3. Xpub modal
    if let Some(_) = &state.views.xpub.modal {
        return Some(xpub::render_modal(state)?);
    }
    // 4. Key edit modal
    if let Some(edit_key) = &state.views.keys.edit_key {
        return Some(key_modal(edit_key, &state.app.keys));
    }
    // 5. Path edit modal
    if let Some(edit_path) = &state.views.paths.edit_path {
        return Some(path_modal(edit_path, &state.app));
    }
    None
}
```

### Modal Overlay

```rust
if let Some(modal) = modals::render_modals(self) {
    Modal::new(content, modal)
        .on_blur(Some(cancel_message))
        .into()
} else {
    content
}
```

### Closing a Modal

```rust
fn on_key_cancel_modal(&mut self) {
    self.views.keys.edit_key = None;
}
```

## SelectKeySource-Style Modal (Xpub Entry)

Two-step pattern with collapsible "Other options":

```rust
pub struct XpubEntryModalState {
    pub key_id: u8,
    pub xpub_source: XpubSource,
    pub xpub_input: String,
    pub hw_devices: Vec<...>,
    pub validation_error: Option<String>,
    pub processing: bool,
    pub options_collapsed: bool,  // Collapsible state
    pub modal_step: ModalStep,    // Select or Details
}

pub enum ModalStep {
    Select,   // Device list
    Details,  // Account picker + fetch
}
```

### Collapsible Section

```rust
fn render_other_options(modal: &XpubEntryModalState) -> Element<'_, Msg> {
    let header_text = if modal.options_collapsed {
        "Other options ▼"
    } else {
        "Other options ▲"
    };

    let mut content = Column::new();
    content = content.push(
        button::transparent(None, header_text)
            .on_press(Msg::XpubToggleOptions)
    );

    if !modal.options_collapsed {
        content = content
            .push(button::secondary(Some(icon::import_icon()), "Import file")
                .on_press(Msg::XpubLoadFromFile))
            .push(button::secondary(Some(icon::clipboard_icon()), "Paste")
                .on_press(Msg::XpubPaste));
    }

    content.into()
}
```

## Form Handling

### Form State

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

### Navigation

```rust
button::transparent(Some(icon::previous_icon()), "Previous")
    .on_press(Message::NavigateBack)
```

### Disabled Buttons

Omit `.on_press()`:

```rust
let mut btn = button::primary(None, "Continue");
if form_is_valid {
    btn = btn.on_press(Message::Continue);
}
```

## Spacing and Layout

### Common Spacing

```rust
Space::with_height(Length::Fixed(100.0))  // Large gap
Space::with_width(Length::Fill)           // Flexible fill
Space::with_width(Length::FillPortion(2)) // Proportional
```

### Column Layout

```rust
Column::new()
    .spacing(10)
    .push(text::h3("Title"))
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

## Scrollable Lists

Use `layout_with_scrollable_list()` for views with lists:

```rust
layout_with_scrollable_list(
    /* layout params */,
    list_content,   // Scrollable list
    Some(footer),   // Optional footer (buttons)
)
```

This keeps header and footer fixed while making only the list scrollable.

## Icons

Available icons from `liana_ui::icon`:

```
+--------------------+-----------------+
| Icon               | Usage           |
+--------------------+-----------------+
| previous_icon()    | Back navigation |
| plus_icon()        | Add action      |
| trash_icon()       | Delete action   |
| pencil_icon()      | Edit action     |
| import_icon()      | File import     |
| clipboard_icon()   | Paste           |
| clock_icon()       | Waiting/time    |
+--------------------+-----------------+
```

## Role-Based UI

### Filtering Content

```rust
let user_role = &state.app.current_user_role;

let filtered_keys: Vec<_> = state.app.keys.iter()
    .filter(|(_, key)| {
        match user_role {
            Some(UserRole::Participant) => {
                key.email.to_lowercase() == current_email.to_lowercase()
            }
            _ => true,  // WS Admin/Wallet Manager see all
        }
    })
    .collect();
```

### Role Badge

```rust
let role_badge = if matches!(user_role, Some(UserRole::WS Admin)) {
    Some("WS Admin")
} else {
    None
};
```

### Role-Based Actions

```rust
// Only WS Admin can edit paths
let is_editable = matches!(user_role, Some(UserRole::WS Admin));

let path_card = if is_editable {
    button::transparent(None, path_content)
        .on_press(Msg::TemplateEditPath(...))
} else {
    Container::new(path_content)  // No click handler
};
```
