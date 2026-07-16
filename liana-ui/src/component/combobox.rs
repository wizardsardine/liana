use std::fmt::Display;

use iced::{
    advanced::{
        layout, overlay,
        widget::{tree, Operation, Tree, Widget},
        Clipboard, Layout, Shell,
    },
    mouse,
    widget::{
        button, column,
        combo_box::{self, ComboBox as IcedComboBox},
        container, mouse_area, row, scrollable,
        text_input::Icon as IcedIcon,
        Space,
    },
    Alignment, Border, Event, Font, Length, Padding, Pixels, Point, Rectangle, Shadow, Size,
    Vector,
};

use crate::{
    color,
    component::{
        badge,
        text::{self, Text},
    },
    icon, theme,
    widget::*,
};

const FIELD_HEIGHT: f32 = 40.0;
const FIELD_PADDING: Padding = Padding {
    top: 9.6,
    right: 16.0,
    bottom: 9.6,
    left: 16.0,
};
const INPUT_SIZE: Pixels = Pixels(16.0);
const MENU_HEIGHT: f32 = 264.0;
const MENU_ROW_PADDING: Padding = Padding {
    top: 9.0,
    right: 14.0,
    bottom: 9.0,
    left: 14.0,
};
const MENU_HEADER_PADDING: Padding = Padding {
    top: 9.0,
    right: 14.0,
    bottom: 5.0,
    left: 14.0,
};
const MENU_SHADOW: Shadow = Shadow {
    color: color::BLACK_15,
    offset: Vector { x: 0.0, y: 4.0 },
    blur_radius: 10.0,
};

#[derive(Debug, Clone)]
pub struct State<T: Display + Clone> {
    combo_box: combo_box::State<T>,
}

impl<T: Display + Clone> State<T> {
    pub fn new(options: Vec<T>) -> Self {
        Self {
            combo_box: combo_box::State::new(options),
        }
    }

    pub fn with_selection(options: Vec<T>, selection: Option<&T>) -> Self {
        Self {
            combo_box: combo_box::State::with_selection(options, selection),
        }
    }

    fn combo_box(&self) -> &combo_box::State<T> {
        &self.combo_box
    }
}

impl<T: Display + Clone> Default for State<T> {
    fn default() -> Self {
        Self {
            combo_box: combo_box::State::new(Vec::new()),
        }
    }
}

pub type Combobox<'a, Message> = Element<'a, Message>;

pub enum MenuEntry<'a, T: Display + Clone, Message> {
    Header(Element<'a, Message>),
    Option {
        value: T,
        body: Element<'a, Message>,
        selected: bool,
    },
    Empty(Element<'a, Message>),
}

pub struct EditableMenuActions<F> {
    pub on_input: Option<F>,
}

/// Trailing state of an [`email_entry`]: an optional "already a signer" note and the
/// selection check, which can appear together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    None,
    Selected,
    AlreadySigner,
    AlreadySignerSelected,
}

/// A member row: an initials avatar, the name over the email, and a trailing [`Tag`]. When
/// `email` is empty the row is a single line (used for emails with no member name).
pub fn email_entry<'a, M: 'a>(avatar: &str, name: &str, email: &str, tag: Tag) -> Element<'a, M> {
    let details = column![
        text::new::b5_medium(name.to_string()),
        (!email.is_empty())
            .then_some(text::new::small_caption(email.to_string()).style(theme::text::secondary))
    ]
    .spacing(2)
    .width(Length::Fill);
    row![badge::avatar(avatar.to_string()), details, trailing(tag)]
        .spacing(11)
        .align_y(Alignment::Center)
        .into()
}

fn trailing<'a, M: 'a>(tag: Tag) -> Element<'a, M> {
    let note = || text::new::small_caption("already a signer").style(theme::text::secondary);
    let check = || icon::check_icon().size(13).style(theme::text::success);
    match tag {
        Tag::None => Space::with_width(Length::Shrink).into(),
        Tag::Selected => check().into(),
        Tag::AlreadySigner => note().into(),
        Tag::AlreadySignerSelected => row![note(), check()]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
    }
}

pub fn combobox<'a, T, Message>(
    state: &'a State<T>,
    placeholder: &'a str,
    selected: Option<T>,
    on_selected: impl Fn(T) -> Message + 'static,
) -> Combobox<'a, Message>
where
    T: Display + Clone + 'static,
    Message: Clone + 'a + 'static,
{
    wrap_combobox(styled_combobox(state, placeholder, selected, on_selected))
}

pub fn editable_combobox<'a, T, Message>(
    state: &'a State<T>,
    placeholder: &'a str,
    selected: Option<T>,
    on_selected: impl Fn(T) -> Message + 'static,
    on_input: impl Fn(String) -> Message + 'static,
    on_close: Message,
) -> Combobox<'a, Message>
where
    T: Display + Clone + 'static,
    Message: Clone + 'a + 'static,
{
    wrap_combobox(
        styled_combobox(state, placeholder, selected, on_selected)
            .on_input(on_input)
            .on_close(on_close),
    )
}

fn styled_combobox<'a, T, Message>(
    state: &'a State<T>,
    placeholder: &'a str,
    selected: Option<T>,
    on_selected: impl Fn(T) -> Message + 'static,
) -> IcedComboBox<'a, T, Message, theme::Theme, Renderer>
where
    T: Display + Clone + 'static,
    Message: Clone + 'a,
{
    IcedComboBox::new(
        state.combo_box(),
        placeholder,
        selected.as_ref(),
        on_selected,
    )
    .width(Length::Fill)
    .padding(FIELD_PADDING)
    .size(INPUT_SIZE)
    .icon(iced_chevron())
    .input_style(theme::combobox::input)
    .menu_style(theme::combobox::menu)
    .menu_height(MENU_HEIGHT)
}

pub fn editable_menu_combobox<'a, T, Message, F>(
    placeholder: &'a str,
    value: String,
    on_selected: impl Fn(T) -> Message + 'static,
    entries: Vec<MenuEntry<'a, T, Message>>,
    actions: EditableMenuActions<F>,
) -> Combobox<'a, Message>
where
    T: Display + Clone + 'static,
    Message: Clone + 'a + 'static,
    F: Fn(String) -> Message + 'static,
{
    EditableMenu::new(
        input(placeholder, value, actions.on_input),
        container(menu(entries, on_selected))
            .padding(Padding::from([4.0, 0.0]))
            .into(),
    )
    .into()
}

fn input<'a, Message, F>(
    placeholder: &'a str,
    value: String,
    on_input: Option<F>,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
    F: Fn(String) -> Message + 'static,
{
    let mut field = crate::widget::text_input::TextInput::new(placeholder, value)
        .width(Length::Fill)
        .padding(FIELD_PADDING)
        .size(INPUT_SIZE.0)
        .icon(text_input_chevron())
        .style(theme::text_input::form);

    if let Some(on_input) = on_input {
        field = field.on_input(on_input);
    }

    mouse_area(wrap_field(field.into()))
        .interaction(mouse::Interaction::Text)
        .into()
}

struct EditableMenu<'a, Message> {
    input: Element<'a, Message>,
    menu: Element<'a, Message>,
}

#[derive(Debug, Default)]
struct EditableMenuState {
    is_open: bool,
}

impl<'a, Message> EditableMenu<'a, Message> {
    fn new(input: Element<'a, Message>, menu: Element<'a, Message>) -> Self {
        Self { input, menu }
    }
}

impl<'a, Message> Widget<Message, theme::Theme, Renderer> for EditableMenu<'a, Message>
where
    Message: Clone + 'a,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<EditableMenuState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(EditableMenuState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.input), Tree::new(&self.menu)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[&self.input, &self.menu]);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Shrink,
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.input
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.input
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.input.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        let state = tree.state.downcast_mut::<EditableMenuState>();

        if matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) && cursor.is_over(layout.bounds())
        {
            state.is_open = true;
            shell.request_redraw();
            shell.capture_event();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &theme::Theme,
        style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.input.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        _renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, theme::Theme, Renderer>> {
        tree.state
            .downcast_ref::<EditableMenuState>()
            .is_open
            .then(|| {
                overlay::Element::new(Box::new(EditableMenuOverlay {
                    position: layout.position() + translation,
                    target_size: layout.bounds().size(),
                    menu: &mut self.menu,
                    tree,
                }))
            })
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.input.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
}

struct EditableMenuOverlay<'a, 'b, Message> {
    position: Point,
    target_size: Size,
    menu: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
}

impl<Message> overlay::Overlay<Message, theme::Theme, Renderer>
    for EditableMenuOverlay<'_, '_, Message>
where
    Message: Clone,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let limits = layout::Limits::new(
            Size::ZERO,
            Size::new(
                bounds.width - self.position.x,
                bounds.height - self.position.y,
            ),
        )
        .width(self.target_size.width);

        self.menu
            .as_widget_mut()
            .layout(&mut self.tree.children[1], renderer, &limits)
            .move_to(self.position + Vector::new(0.0, self.target_size.height))
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        self.menu.as_widget_mut().update(
            &mut self.tree.children[1],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if !cursor.is_over(layout.bounds()) {
                    self.tree.state.downcast_mut::<EditableMenuState>().is_open = false;
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if cursor.is_over(layout.bounds()) {
                    self.tree.state.downcast_mut::<EditableMenuState>().is_open = false;
                    shell.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &theme::Theme,
        style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.menu.as_widget().draw(
            &self.tree.children[1],
            renderer,
            theme,
            style,
            layout,
            cursor,
            &layout.bounds(),
        );
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.menu
            .as_widget_mut()
            .operate(&mut self.tree.children[1], layout, renderer, operation);
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.menu.as_widget().mouse_interaction(
            &self.tree.children[1],
            layout,
            cursor,
            &layout.bounds(),
            renderer,
        )
    }
}

impl<'a, Message> From<EditableMenu<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(menu: EditableMenu<'a, Message>) -> Self {
        Self::new(menu)
    }
}

fn menu<'a, T, Message>(
    entries: Vec<MenuEntry<'a, T, Message>>,
    on_selected: impl Fn(T) -> Message + 'static,
) -> Element<'a, Message>
where
    T: Display + Clone + 'static,
    Message: Clone + 'a,
{
    let body = entries
        .into_iter()
        .fold(column![].spacing(0), |column, entry| match entry {
            MenuEntry::Header(content) => {
                column.push(container(content).padding(MENU_HEADER_PADDING))
            }
            MenuEntry::Option {
                value,
                body,
                selected,
            } => column.push(menu_option(body, selected, on_selected(value))),
            MenuEntry::Empty(content) => column.push(container(content).padding(MENU_ROW_PADDING)),
        });

    container(scrollable(body))
        .max_height(MENU_HEIGHT)
        .width(Length::Fill)
        .style(menu_panel)
        .into()
}

fn menu_option<'a, Message>(
    content: Element<'a, Message>,
    selected: bool,
    on_press: Message,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Button::new(content)
        .width(Length::Fill)
        .padding(MENU_ROW_PADDING)
        .style(move |theme, status| menu_option_style(theme, status, selected))
        .on_press(on_press)
        .into()
}

fn menu_option_style(
    theme: &theme::Theme,
    status: button::Status,
    selected: bool,
) -> button::Style {
    let menu = theme.colors.menus.pick_list;
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: (selected || hovered).then_some(theme.colors.combobox.selected.into()),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        text_color: menu.text,
        ..button::Style::default()
    }
}

fn menu_panel(theme: &theme::Theme) -> container::Style {
    let menu = theme.colors.menus.pick_list;
    container::Style {
        background: Some(menu.background.into()),
        border: Border {
            color: menu.border,
            width: 1.0,
            radius: 4.0.into(),
            ..Default::default()
        },
        shadow: MENU_SHADOW,
        ..container::Style::default()
    }
}

fn wrap_combobox<'a, T, Message>(
    input: IcedComboBox<'a, T, Message, theme::Theme, Renderer>,
) -> Combobox<'a, Message>
where
    T: Display + Clone + 'static,
    Message: Clone + 'a,
{
    container(input)
        .width(Length::Fill)
        .height(FIELD_HEIGHT)
        .style(theme::combobox::field)
        .into()
}

fn wrap_field<'a, Message>(input: Element<'a, Message>) -> Combobox<'a, Message>
where
    Message: Clone + 'a,
{
    container(input)
        .width(Length::Fill)
        .height(FIELD_HEIGHT)
        .style(container::transparent)
        .into()
}

fn iced_chevron() -> IcedIcon<Font> {
    IcedIcon {
        font: <Renderer as iced::advanced::text::Renderer>::ICON_FONT,
        code_point: <Renderer as iced::advanced::text::Renderer>::ARROW_DOWN_ICON,
        size: Some(Pixels(16.0)),
        spacing: 8.0,
        side: iced::widget::text_input::Side::Right,
    }
}

fn text_input_chevron() -> crate::widget::text_input::Icon<Font> {
    crate::widget::text_input::Icon {
        font: <Renderer as iced::advanced::text::Renderer>::ICON_FONT,
        code_point: <Renderer as iced::advanced::text::Renderer>::ARROW_DOWN_ICON,
        size: Some(Pixels(16.0)),
        spacing: 8.0,
        side: crate::widget::text_input::Side::Right,
    }
}

pub fn menu_header<'a, Message>(label: impl Display) -> Element<'a, Message> {
    text::new::small_caption(label.to_string().to_uppercase())
        .style(theme::text::secondary)
        .bold()
        .into()
}

pub fn height() -> Length {
    Length::Fixed(FIELD_HEIGHT)
}
