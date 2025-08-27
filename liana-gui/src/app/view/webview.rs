use iced::{Task, Subscription};
use iced::alignment::Horizontal;

// Ultralight webview imports
#[cfg(feature = "webview")]
use iced_webview::{WebView, Ultralight, Action as WebviewAction};
use iced::widget::Column;
use liana_ui::{
    color,
    component::text,
    widget::Element,
};

// Clean Ultralight webview implementation

/// Webview state for managing embedded browser
#[derive(Debug, Clone)]
pub struct WebviewState {
    pub url: String,
    pub is_loading: bool,
    pub show_webview: bool,
    pub has_webview: bool,
}

impl Default for WebviewState {
    fn default() -> Self {
        Self {
            url: String::new(),
            is_loading: false,
            show_webview: false,
            has_webview: false,
        }
    }
}

impl WebviewState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_url(&mut self, url: String) {
        self.url = url;
        self.show_webview = true;
        self.is_loading = true;
    }

    pub fn close(&mut self) {
        self.show_webview = false;
        self.is_loading = false;
        #[cfg(feature = "webview")]
        {
            self.has_webview = false;
        }
    }

    pub fn toggle(&mut self) {
        self.show_webview = !self.show_webview;
    }
}

/// Messages for webview operations
#[derive(Debug, Clone)]
pub enum WebviewMessage {
    OpenUrl(String),
    Close,
    ToggleWebview,
    #[cfg(feature = "webview")]
    WebviewAction(WebviewAction),
    #[cfg(feature = "webview")]
    WebviewCreated,
    #[cfg(feature = "webview")]
    UrlChanged(String),
}

/// Webview component with actual webview integration
pub struct WebviewComponent {
    pub state: WebviewState,
    #[cfg(feature = "webview")]
    pub webview: WebView<Ultralight, WebviewMessage>,
}

impl WebviewComponent {
    pub fn new() -> Self {
        Self {
            state: WebviewState::new(),
            #[cfg(feature = "webview")]
            webview: WebView::new(),
        }
    }

    pub fn update(&mut self, message: WebviewMessage) -> Task<WebviewMessage> {
        match message {
            WebviewMessage::OpenUrl(url) => {
                tracing::info!("Opening URL in webview: {}", url);
                self.state.open_url(url.clone());
                self.state.is_loading = false;
                Task::none()
            }
            WebviewMessage::Close => {
                tracing::info!("Closing webview");
                self.state.close();
                Task::none()
            }
            WebviewMessage::ToggleWebview => {
                self.state.toggle();
                Task::none()
            }
            #[cfg(feature = "webview")]
            WebviewMessage::WebviewCreated => {
                tracing::info!("Webview created successfully");
                self.state.has_webview = true;
                self.state.is_loading = false;
                Task::none()
            }
            #[cfg(feature = "webview")]
            WebviewMessage::UrlChanged(new_url) => {
                tracing::info!("Webview URL changed to: {}", new_url);
                self.state.url = new_url;
                Task::none()
            }
            #[cfg(feature = "webview")]
            WebviewMessage::WebviewAction(action) => {
                // Handle Ultralight webview actions
                self.webview.update(action)
            }
        }
    }

    pub fn view(&self) -> Element<'_, WebviewMessage> {
        Column::new()
            .push(
                text::text("Webview component placeholder")
                    .size(16)
                    .color(color::GREY_2)
            )
            .align_x(Horizontal::Center)
            .into()
    }
}





impl WebviewComponent {


    pub fn subscription(&self) -> Subscription<WebviewMessage> {
        if self.state.has_webview {
            // Subscribe to webview updates - disabled due to missing Action type
            // time::every(Duration::from_millis(16)) // ~60 FPS
            //     .map(|_| Action::Update)
            //     .map(WebviewMessage::WebviewAction)
            Subscription::none()
        } else {
            Subscription::none()
        }
    }
}

/// Create a webview widget for Meld integration using Ultralight
/// This renders the actual Meld widget content inline with proper initialization
#[cfg(feature = "webview")]
pub fn meld_webview_widget_ultralight<'a>(
    webview: Option<&'a WebView<Ultralight, crate::app::WebviewMessage>>,
    url: Option<&'a str>,
    show_webview: bool,
    webview_ready: bool,
    is_loading: bool,
    current_webview_index: Option<u32>,
) -> Element<'a, crate::app::view::Message> {
    use iced::widget::{Container, Column, Row, Space};
    use iced::{Length, Alignment};
    use liana_ui::{color, component::{text::text, button::*}};

    if is_loading {
        // Show loading state while webview is being created
        Container::new(
            Column::new()
                .push(text("🌐 Loading Meld Widget...").size(16).color(color::GREEN))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(text("Creating embedded webview...").size(14).color(color::GREY_3))
                .align_x(Alignment::Center)
                .spacing(5)
        )
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fixed(300.0))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_theme| iced::widget::container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(0.05, 0.05, 0.05))),
            border: iced::Border {
                color: color::ORANGE,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
    } else if url.is_some() && show_webview && webview_ready && current_webview_index.is_some() {
        // Show the actual Ultralight webview content only when everything is ready AND we have a current view index
        if let Some(webview) = webview {
            Container::new(
                Column::new()
                    .push(
                        // Header with webview info
                        Row::new()
                            .push(text("Meld Widget").size(16).color(color::GREEN))
                            .push(Space::with_width(Length::Fill))
                            // .push(text(url.unwrap_or("Loading...")).size(12).color(color::GREY_3))
                            // .push(Space::with_width(Length::Fixed(10.0)))
                            // .push(text(format!("View: {}", current_webview_index.unwrap_or(0))).size(10).color(color::GREY_2))
                            .align_y(Alignment::Center)
                            .padding([5, 10])
                    )
                    .push(
                        // The actual Ultralight webview content area
                        Container::new(
                            webview.view()
                                .map(|action| crate::app::view::Message::WebviewAction(action))
                        )
                        .width(Length::Fill)
                        .height(Length::Fixed(800.0))
                    )
            )
            .width(Length::Fill)
            .style(webview_container_style)
            .into()
        } else {
            // Webview not available, show error
            Container::new(
                Column::new()
                    .push(text("❌ Webview Error").size(16).color(color::RED))
                    .push(Space::with_height(Length::Fixed(10.0)))
                    .push(text("Webview not available").size(14).color(color::GREY_3))
                    .align_x(Alignment::Center)
                    .spacing(5)
            )
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fixed(800.0))
            .style(webview_placeholder_style)
            .into()
        }
    } else if url.is_some() && show_webview && webview_ready && current_webview_index.is_none() {
        // Webview is ready but no current view index set - show waiting for view switch
        Container::new(
            Column::new()
                .push(text("🌐 Switching to Webview...").size(16).color(color::ORANGE))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(text("Setting up view index").size(14).color(color::GREY_3))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(text("Please wait...").size(12).color(color::GREY_2))
                .align_x(Alignment::Center)
                .spacing(5)
        )
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fixed(800.0))
        .style(webview_placeholder_style)
        .into()
    } else if url.is_some() && show_webview && !webview_ready {
        // Show initializing state when webview is being prepared
        Container::new(
            Column::new()
                .push(text("🌐 Initializing Webview...").size(16).color(color::ORANGE))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(text("Setting up payment interface").size(14).color(color::GREY_3))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(text("Please wait...").size(12).color(color::GREY_2))
                .align_x(Alignment::Center)
                .spacing(5)
        )
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fixed(800.0))
        .style(webview_placeholder_style)
        .into()
    } else if url.is_some() && !show_webview {
        // Show button to open webview when URL is available but webview is not shown
        Container::new(
            Column::new()
                .push(text("🌐 Meld Payment Ready").size(16).color(color::GREEN))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(text("Your payment session is ready").size(14).color(color::GREY_3))
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(
                    primary(None, "Show Payment Interface")
                        .on_press(crate::app::view::Message::ShowWebView)
                        .width(Length::Fixed(200.0))
                )
                .align_x(Alignment::Center)
                .spacing(5)
        )
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fixed(800.0))
        .style(webview_placeholder_style)
        .into()
    } else {
        // Show that webview will be available after session creation
        Container::new(
            Column::new()
                .push(text("🌐 Meld Widget").size(16).color(color::GREEN))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(text("Payment interface will appear after session creation").size(14).color(color::GREY_3))
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(text("Generate a session to continue").size(12).color(color::GREY_2))
                .align_x(Alignment::Center)
                .spacing(5)
        )
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fixed(800.0))
        .style(webview_placeholder_style)
        .into()
    }
}

/// Theme-compatible style function for webview container
/// Uses liana-ui theme colors to ensure visual consistency
fn webview_container_style(theme: &liana_ui::theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.colors.general.foreground)),
        border: iced::Border {
            color: theme.colors.text.success, // Use theme's success color (green)
            width: 2.0,
            radius: 8.0.into(),
        },
        text_color: Some(theme.colors.text.primary),
        ..Default::default()
    }
}

/// Theme-compatible style function for webview placeholder
/// Uses liana-ui theme colors for the placeholder state
fn webview_placeholder_style(theme: &liana_ui::theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.colors.general.foreground)),
        border: iced::Border {
            color: theme.colors.text.success, // Use theme's success color (green)
            width: 1.0,
            radius: 8.0.into(),
        },
        text_color: Some(theme.colors.text.primary),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liana_ui::theme::Theme;
    use crate::app::WebviewMessage;

    #[test]
    fn test_webview_state_default() {
        let state = WebviewState::default();
        assert_eq!(state.url, "");
        assert!(!state.is_loading);
        assert!(!state.show_webview);
        assert!(!state.has_webview);
    }

    #[test]
    fn test_webview_state_new() {
        let state = WebviewState::new();
        assert_eq!(state.url, "");
        assert!(!state.is_loading);
        assert!(!state.show_webview);
        assert!(!state.has_webview);
    }

    #[test]
    fn test_webview_container_style() {
        let theme = Theme::default();
        let style = webview_container_style(&theme);

        // Verify background uses theme foreground color
        assert!(style.background.is_some());
        if let Some(iced::Background::Color(color)) = style.background {
            assert_eq!(color, theme.colors.general.foreground);
        }

        // Verify border uses theme success color
        assert_eq!(style.border.color, theme.colors.text.success);
        assert_eq!(style.border.width, 2.0);
        assert_eq!(style.border.radius, 8.0.into());

        // Verify text color uses theme primary color
        assert_eq!(style.text_color, Some(theme.colors.text.primary));
    }

    #[test]
    fn test_webview_placeholder_style() {
        let theme = Theme::default();
        let style = webview_placeholder_style(&theme);

        // Verify background uses theme foreground color
        assert!(style.background.is_some());
        if let Some(iced::Background::Color(color)) = style.background {
            assert_eq!(color, theme.colors.general.foreground);
        }

        // Verify border uses theme success color with different width
        assert_eq!(style.border.color, theme.colors.text.success);
        assert_eq!(style.border.width, 1.0);
        assert_eq!(style.border.radius, 8.0.into());

        // Verify text color uses theme primary color
        assert_eq!(style.text_color, Some(theme.colors.text.primary));
    }

    #[cfg(feature = "webview")]
    #[test]
    fn test_webview_component_new() {
        let component = WebviewComponent::new();
        assert_eq!(component.state.url, "");
        assert!(!component.state.is_loading);
        assert!(!component.state.show_webview);
        assert!(!component.state.has_webview);
    }

    #[test]
    fn test_webview_message_variants() {
        // Test that WebviewMessage variants can be created and cloned
        let created = WebviewMessage::Created;
        let url_changed = WebviewMessage::UrlChanged("https://example.com".to_string());

        // Test cloning
        let _created_clone = created.clone();
        let _url_changed_clone = url_changed.clone();

        // Test debug formatting
        let debug_str = format!("{:?}", created);
        assert!(debug_str.contains("Created"));

        let debug_str = format!("{:?}", url_changed);
        assert!(debug_str.contains("UrlChanged"));
        assert!(debug_str.contains("https://example.com"));
    }
}