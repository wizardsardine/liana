use crossbeam::channel;
use iced::futures::Stream;
use liana_connect::WssError;
pub use liana_connect::{Org, OrgData, User, UserRole, Wallet, WalletStatus};
use miniscript::DescriptorPublicKey;
use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::mpsc,
    task::{Context, Poll},
};
use thiserror::Error;
use uuid::Uuid;

use crate::Message;

#[derive(Debug, Clone, Error)]
pub enum Error {
    #[error("")]
    None,
    #[error("Iced subscription failed!")]
    SubscriptionFailed,
    #[error("Missing token for auth on backend!")]
    TokenMissing,
    #[error("Failed to open the websocket connection")]
    WsConnection,
    #[error("Failed to handle a Websocket response: {0}")]
    WsMessageHandling(String),
    #[error("Receive an error from the server: {0}")]
    Wss(WssError),
}

impl Error {
    pub fn show_warning(&self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone)]
pub enum Notification {
    Connected,
    Disconnected,
    AuthCodeSent,
    InvalidEmail,
    AuthCodeFail,
    LoginSuccess,
    LoginFail,
    Org(Uuid),
    Wallet(Uuid),
    User(Uuid),
    Error(Error),
}

impl From<Notification> for crate::Message {
    fn from(value: Notification) -> Self {
        crate::Message::BackendNotif(value)
    }
}

#[allow(unused)]
#[rustfmt::skip]
pub trait Backend {
    // Auth, not part of WSS protocol
    fn auth_request(&mut self, email: String);  // -> Response::AuthCodeSent
                                                // -> Response::InvalidEmail
                                                // -> Response::AuthCodeFail
    fn auth_code(&mut self, code: String);  // -> Response::LoginSuccess
                                            // -> Response::LoginFail

    // Cache only, not backend calls
    fn get_orgs(&self) -> BTreeMap<Uuid, Org>;
    fn get_org(&self, id: Uuid) -> Option<OrgData>;
    fn get_user(&self, id: Uuid) -> Option<User>;
    fn get_wallet(&self, id: Uuid) -> Option<Wallet>;

    // Connection (WSS)
    fn connect_ws(&mut self, url: String, version: u8, notif_sender: channel::Sender<Message>) ; // -> Response::Connected
    fn ping(&mut self); // -> Response::Pong
    fn close(&mut self);    // Connection closed

    // Org management (WSS)
    fn fetch_org(&mut self, id: Uuid);                                      // -> Response::Org
    fn remove_wallet_from_org(&mut self, wallet_id: Uuid, org_id: Uuid);    // -> Response::Org

    fn create_wallet(&mut self, name: String, org: Uuid, owner: Uuid);      // -> Response::Wallet
    fn edit_wallet(&mut self, wallet: Wallet);                              // -> Response::Wallet
    fn fetch_wallet(&mut self, id: Uuid);                                   // -> Response::Wallet
    fn edit_xpub(
        &mut self,
        wallet_id: Uuid,
        xpub: Option<DescriptorPublicKey>,
        key_id: u8);                                                     // -> Response::Wallet

    fn fetch_user(&mut self, id: Uuid);         // -> Response::User

}

/// Stream wrapper for Backend responses
pub struct BackendStream {
    pub receiver: mpsc::Receiver<Notification>,
}

impl Stream for BackendStream {
    type Item = Notification;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Use try_recv for non-blocking check
        match self.receiver.try_recv() {
            Ok(item) => Poll::Ready(Some(item)),
            Err(mpsc::TryRecvError::Empty) => Poll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => Poll::Ready(None),
        }
    }
}
