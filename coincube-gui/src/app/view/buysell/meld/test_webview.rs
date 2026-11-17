//! An inert native boundary for unit tests of Meld's forms and state machine.
//! Any attempt to touch a webview fails immediately instead of initializing GTK,
//! opening a window, or silently pretending the native operation succeeded.

#[derive(Default)]
pub struct WebviewManager;

impl WebviewManager {
    pub fn new() -> Self {
        Self
    }

    pub fn clear_view(&mut self, _view: &iced_wry::IcedWebview) {
        panic!("native webview access is forbidden in Meld unit tests")
    }

    pub fn new_webview(
        &mut self,
        _attributes: iced_wry::wry::WebViewAttributes<'static>,
        _window: iced_wry::ExtractedWindowId,
    ) -> Option<iced_wry::IcedWebview> {
        panic!("native webview access is forbidden in Meld unit tests")
    }

    pub fn update(&mut self, _message: iced_wry::IcedWryMessage) {
        panic!("native webview access is forbidden in Meld unit tests")
    }

    pub fn subscription(
        &self,
        _duration: std::time::Duration,
    ) -> iced::Subscription<iced_wry::IcedWryMessage> {
        panic!("native webview access is forbidden in Meld unit tests")
    }
}
