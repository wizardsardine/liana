use iced::Task;
use std::sync::Arc;

use liana_ui::widget::Element;

use crate::{
    app::{
        buysell::{
            meld::{MeldClient, MeldError},
            ServiceProvider,
        },
        cache::Cache,
        message::Message,
        state::State,
        view::{
            meld_buysell::{meld_buysell_view, MeldBuySellPanel},
            MeldBuySellMessage, Message as ViewMessage,
        },
        wallet::Wallet,
    },
    daemon::Daemon,
};

impl Default for MeldBuySellPanel {
    fn default() -> Self {
        Self::new(liana::miniscript::bitcoin::Network::Bitcoin)
    }
}

impl State for MeldBuySellPanel {
    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, ViewMessage> {
        // Return the meld view directly - dashboard wrapper will be applied by app/mod.rs
        meld_buysell_view(self)
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::WalletAddressChanged(
                address,
            ))) => {
                self.set_wallet_address(address);
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::CountryCodeChanged(
                code,
            ))) => {
                self.set_country_code(code);
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::SourceAmountChanged(
                amount,
            ))) => {
                self.set_source_amount(amount);
            }

            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::CreateSession)) => {
                if self.is_form_valid() {
                    tracing::info!(
                        "🚀 [MELD] Creating new session - clearing any existing session data"
                    );

                    // Ensure we start with a clean slate
                    self.widget_url = None;
                    self.widget_session_created = None;
                    self.error = None;
                    self.loading = true;

                    // init session
                    let wallet_address = self.wallet_address.value.clone();
                    let country_code = self.country_code.value.clone();
                    let source_amount = self.source_amount.value.clone();

                    tracing::info!(
                        "🚀 [MELD] Making fresh API call with: address={}, country={}, amount={}",
                        wallet_address,
                        country_code,
                        source_amount
                    );

                    return Task::perform(
                        create_meld_session(
                            wallet_address,
                            country_code,
                            source_amount,
                            // Use Transak as the default payment provider
                            ServiceProvider::Transak,
                            self.network,
                        ),
                        |result| match result {
                            Ok(widget_url) => {
                                tracing::info!(
                                    "✅ [MELD] New session created successfully: {}",
                                    widget_url
                                );
                                Message::View(ViewMessage::MeldBuySell(
                                    MeldBuySellMessage::SessionCreated(widget_url),
                                ))
                            }
                            Err(error) => {
                                tracing::error!("❌ [MELD] Session creation failed: {}", error);
                                Message::View(ViewMessage::MeldBuySell(
                                    MeldBuySellMessage::SessionError(error),
                                ))
                            }
                        },
                    );
                } else {
                    tracing::warn!("⚠️ [MELD] Cannot create session - form validation failed");
                }
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::SessionCreated(
                widget_url,
            ))) => {
                self.session_created(widget_url.clone());

                // Immediately open the webview after session creation
                tracing::info!(
                    "Auto-opening widget URL in embedded webview: {}",
                    widget_url
                );

                return Task::done(Message::View(ViewMessage::OpenWebview(widget_url)));
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::OpenWidget(widget_url))) => {
                // Log the URL we're trying to open
                tracing::info!("Attempting to open widget URL: {}", widget_url);
                return Task::done(Message::View(ViewMessage::OpenWebview(widget_url)));
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::CopyUrl(widget_url))) => {
                tracing::info!("Attempting to copy URL to clipboard: {}", widget_url);

                let mut success = false;

                // Try WSL clipboard commands first
                let powershell_cmd = format!("Set-Clipboard '{}'", widget_url);
                let wsl_clipboard_commands = [
                    ("clip.exe", vec![]), // Windows clipboard via WSL
                    ("powershell.exe", vec!["-c", &powershell_cmd]),
                ];

                for (cmd, args) in wsl_clipboard_commands {
                    if cmd == "clip.exe" {
                        // For clip.exe, we need to pipe the input
                        match std::process::Command::new(cmd)
                            .stdin(std::process::Stdio::piped())
                            .spawn()
                        {
                            Ok(mut child) => {
                                if let Some(stdin) = child.stdin.take() {
                                    use std::io::Write;
                                    if let Ok(mut stdin) =
                                        std::io::BufWriter::new(stdin).into_inner()
                                    {
                                        if stdin.write_all(widget_url.as_bytes()).is_ok() {
                                            drop(stdin);
                                            if child.wait().is_ok() {
                                                tracing::info!(
                                                    "Successfully copied URL to clipboard with {}",
                                                    cmd
                                                );
                                                success = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                tracing::debug!("WSL clipboard command {} not available", cmd);
                            }
                        }
                    } else {
                        match std::process::Command::new(cmd).args(&args).output() {
                            Ok(_) => {
                                tracing::info!("Successfully copied URL to clipboard with {}", cmd);
                                success = true;
                                break;
                            }
                            Err(_) => {
                                tracing::debug!("WSL clipboard command {} not available", cmd);
                            }
                        }
                    }
                }

                // Try Linux clipboard commands if WSL commands failed
                if !success {
                    let linux_clipboard_commands = [
                        ("xclip", vec!["-selection", "clipboard"]),
                        ("xsel", vec!["--clipboard", "--input"]),
                        ("pbcopy", vec![]), // macOS
                    ];

                    for (cmd, args) in &linux_clipboard_commands {
                        match std::process::Command::new(cmd)
                            .args(args)
                            .stdin(std::process::Stdio::piped())
                            .spawn()
                        {
                            Ok(mut child) => {
                                if let Some(stdin) = child.stdin.take() {
                                    use std::io::Write;
                                    if let Ok(mut stdin) =
                                        std::io::BufWriter::new(stdin).into_inner()
                                    {
                                        if stdin.write_all(widget_url.as_bytes()).is_ok() {
                                            drop(stdin);
                                            if child.wait().is_ok() {
                                                tracing::info!(
                                                    "Successfully copied URL to clipboard with {}",
                                                    cmd
                                                );
                                                success = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                tracing::debug!("Linux clipboard command {} not available", cmd);
                            }
                        }
                    }
                }

                if !success {
                    tracing::warn!(
                        "Could not copy to clipboard automatically. URL logged for manual copying."
                    );
                    self.set_error("Could not copy to clipboard automatically. Please copy the URL manually from the display above.".to_string());
                }
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::SessionError(error))) => {
                self.set_error(error);
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::UrlCopied)) => {
                // Show success message for copied URL
                tracing::info!("URL copied to clipboard successfully");
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::CopyError)) => {
                self.set_error(
                    "Failed to copy URL to clipboard. Please copy manually.".to_string(),
                );
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::ResetForm)) => {
                self.error = None;
                self.widget_url = None;
                self.widget_session_created = None;
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::GoBackToForm)) => {
                tracing::info!("🔄 [MELD] Going back to form - clearing all session data");

                // Complete session reset - clear all session-related data
                self.widget_url = None;
                self.error = None;
                self.widget_session_created = None;
                self.loading = false;

                // Don't reset form validation - keep existing valid data
                // Only reset session-related data, not form data

                tracing::info!("🔄 [MELD] Session data cleared, form reset to initial state");

                // Also close the webview
                return Task::done(Message::View(ViewMessage::CloseWebview));
            }
            Message::View(ViewMessage::MeldBuySell(MeldBuySellMessage::OpenWidgetInNewWindow(
                widget_url,
            ))) => {
                // Open in a new window/browser tab - similar to OpenWidget but explicitly for new window
                tracing::info!(
                    "Attempting to open widget URL in new window: {}",
                    widget_url
                );

                let mut success = false;

                // Method 1: Try open::that_detached first (non-blocking)
                match open::that_detached(&widget_url) {
                    Ok(_) => {
                        tracing::info!(
                            "Successfully opened widget URL in new window with detached method"
                        );
                        success = true;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to open browser with detached method: {}", e);
                    }
                }

                // Method 2: Try WSL-specific commands first, then Linux commands
                if !success {
                    // WSL-specific commands (these work better in WSL)
                    let wsl_commands = [
                        ("cmd.exe", vec!["/c", "start", &widget_url]),
                        ("powershell.exe", vec!["-c", "Start-Process", &widget_url]),
                        ("explorer.exe", vec![&widget_url]),
                    ];

                    // Try WSL commands first
                    for (cmd, args) in &wsl_commands {
                        match std::process::Command::new(cmd).args(args).spawn() {
                            Ok(_) => {
                                tracing::info!("Successfully opened widget URL in new window with WSL command: {}", cmd);
                                success = true;
                                break;
                            }
                            Err(_) => {
                                tracing::debug!("WSL command {} not available", cmd);
                            }
                        }
                    }

                    // If WSL commands failed, try Linux commands
                    if !success {
                        let linux_commands = [
                            ("xdg-open", [&widget_url]),
                            ("firefox", [&widget_url]),
                            ("google-chrome", [&widget_url]),
                            ("chromium", [&widget_url]),
                            ("sensible-browser", [&widget_url]),
                        ];

                        for (cmd, args) in &linux_commands {
                            match std::process::Command::new(cmd).args(args).spawn() {
                                Ok(_) => {
                                    tracing::info!("Successfully opened widget URL in new window with Linux command: {}", cmd);
                                    success = true;
                                    break;
                                }
                                Err(_) => {
                                    tracing::debug!("Linux command {} not available", cmd);
                                }
                            }
                        }
                    }
                }

                if !success {
                    tracing::error!("All browser opening methods failed for new window");
                    self.set_error("Could not open browser automatically. Please copy the URL manually and paste it into your browser.".to_string());
                }
            }
            _ => {}
        };

        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        Task::none()
    }
}

async fn create_meld_session(
    wallet_address: String,
    country_code: String,
    source_amount: String,
    provider: ServiceProvider,
    network: liana::miniscript::bitcoin::Network,
) -> Result<String, String> {
    let client = MeldClient::new();

    match client
        .create_widget_session(
            wallet_address,
            country_code,
            source_amount,
            provider,
            network,
        )
        .await
    {
        Ok(response) => Ok(response.widget_url),
        Err(MeldError::Network(e)) => Err(format!("Network error: {}", e)),
        Err(MeldError::Serialization(e)) => Err(format!("Data error: {}", e)),
        Err(MeldError::Api(e)) => Err(format!("API error: {}", e)),
    }
}
