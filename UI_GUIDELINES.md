# Liana UI Guidelines

UI conventions for the whole repo. Both `liana-gui` and `liana-business/business-installer` are Iced apps
built on the shared **`liana-ui`** crate. `liana-ui` is the design system: reference it, do not hand-roll or
re-define widgets, colors, or layout primitives. If something is missing, add it to `liana-ui`, not to a
consumer. This doc points at liana-ui and the app scaffolds rather than copying code, so it does not drift.

Paths are under `liana-ui/src/` unless noted.

## Core idioms

- **Layout macros.** Build a static child set with the `row!` / `column!` macros (re-exported via
  `liana_ui::widget::*`). They accept `Option<Element>` args at any position, and `None` is dropped, so a
  fixed set of children lives inside the macro: `row![a, b, opt_c]`, never `row![].push(a).push_maybe(opt_c)`
  and never `Row::new()` / `Column::new()` for a fixed set. Only genuinely dynamic building uses `.push`: a
  loop reassigning `x = x.push(...)`, a `.fold(col, |c, x| c.push(x))`, or `Vec::push`. Flatten deep view
  bodies into named `let` bindings first.
- **Spacing.** Use the `SpaceExt` helpers (`widget/mod.rs`): `Space::with_width(100)`,
  `Space::with_height(50)`, `Space::fill_width()`, `Space::fill_height()`. Never
  `Space::new().width(Length::Fixed(100.0))` or `.width(Length::Fill)`.
- **Lengths.** Do not spell raw `Length::Fixed(..)` / `Length::Fill` in new view code unless the API
  requires a `Length` value and there is no existing helper or named constant. Prefer direct numeric
  arguments when the method accepts `impl Into<Length>` (`.width(LOGIN_WIDTH)`, not
  `.width(Length::Fixed(LOGIN_WIDTH))`), shared width enums (`BtnWidth`, `EntryWidth`, `ModalWidth`,
  `PillWidth`), and named `Length` constants for reusable dimensions. For `Space`, always use `SpaceExt`.
- **Typography.** Use the `text::new::*` roles (`component/text/new.rs`): display `d2/d3/d4`, headings
  `h1..h3` (plus `_semi`), body `b1..b5` (plus `_medium` / `_bold`), `caption` / `small_caption`. Do not size
  text by hand.
- **Styling and color.** Style through the theme, never a hardcoded hex or `Color`:
  `theme::text::{primary,secondary,tertiary,...}`, `theme::card::*`, `theme::button::*`, `theme::pill::*`,
  `theme::badge::*`, `theme::container::*`. Applied as a closure: `.style(theme::text::secondary)`.
- **Message generic.** Extracted components are generic over the message type:
  `fn x<'a, M: Clone + 'static>(..) -> Element<'a, M>` (iced widgets are invariant in `'a`).

## Component reference (`component/<name>.rs`)

Use these; do not build equivalents.

- **button**: consumer code uses semantic helpers only: `btn_save`, `btn_cancel`, `btn_ok`,
  `btn_reload`, feature-specific `btn_*` helpers, `list_entry(content, accent, width, msg)`, `icon_btn`,
  `btn_copy/edit/remove/delete`, `subtle_link`. `primary/secondary/tertiary/destructive/flat/transparent`,
  `btn_primary`, `btn_secondary`, `btn_tertiary`, `btn_destructive`, `btn_flat`, and generic
  `btn_*(icon, label, width, msg)` constructors are liana-ui internals. If a consumer needs a new button,
  add a named helper in `component::button` first, then use that helper. Widths: `BtnWidth`, `EntryWidth`.
- **text::new**: the typography roles above.
- **card**: `simple`, `modal`, `invalid`, `soft_warning`, `success`, `flat`, `section`, `warning`, `info`,
  `error`, `list_entry`.
- **pill**: consumer code uses semantic helpers only: status / key / lifecycle pills, `key_kind`, `path_*`,
  `fingerprint`, `coin_sequence`, feature-specific helpers. `pill`, `pill_with_icon`, `compact_pill`,
  `compact_metric`, and direct `PillWidth` + `theme::pill::*` composition are liana-ui internals. If a
  consumer needs a new pill, add a named helper in `component::pill` first, then use that helper.
- **list**: `list_entry_row(tile, body, trailing, accent, width, msg)` and the `entry_*` constructors
  (`entry_wallet/key/path/set_key/organization/register/device_list/action`, `account_entry`,
  `entry_paste_xpub`); helpers `entry_chevron`, `breadcrumb_chevron`, `key_count`, `see_more`. Status enums
  `Entry*`, `DeviceStatus`.
- **modal**: `modal_view(title, back, close, width, content)` (and `modal_view_with_theme`); width
  `ModalWidth`.
- **badge**: icon badges, `tile(Tile::..)`, `avatar(initials)`, `coin()`.
- **combobox**: `combobox`, `editable_combobox`, `email_entry`; `State`, `Tag`.
- **form**: `Form::new / new_disabled / new_trimmed / new_amount_btc`, `.label`, `.padding`; `Value`;
  `FormSize`.
- **amount**: `amount(..)`, `amount_with_font(..)`, `amount_with_fiat(..)`.
- **tab**: `tab_header(items, active, on_select)`; `Dot`.
- **widget traits** (`widget/mod.rs`): `RowExt` / `ColumnExt::push_maybe` (dynamic building only), `SpaceExt`.

## Per-app layout

Each app owns its page scaffolding on top of the components above. Read the real helpers, do not restate them:

- **liana-gui**: sidebar, dashboard, and menu scaffolding in `liana-gui/src/app/view/mod.rs`.
- **liana-business**: `layout()`, `layout_with_scrollable_list()`, `menu_entry()`, and the breadcrumb +
  step-dot header in `liana-business/business-installer/src/views/mod.rs`.

## liana-business specifics

Business-only concerns live in the code; read them there rather than duplicating: the org/wallet breadcrumb
flow and step counter (`business-installer/src/views/mod.rs`), modal priority
(`business-installer/src/state/...` plus `liana_ui::component::modal`), role-gated UI (`UserRole` from
`liana_connect::ws_business`), and the xpub-entry state machine (`business-installer/src/state/views/xpub`).
liana-gui has none of these.
