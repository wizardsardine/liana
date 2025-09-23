use iced::{Subscription, Task};
use liana_ui::widget::Element;

use crate::{
    app::state::export::ExportModal,
    export::{ImportExportMessage, Progress},
    hw::HardwareWallets,
    installer::{
        self,
        decrypt::{Decrypt, DecryptModal},
    },
};

pub const BACKUP_NETWORK_NOT_MATCH: &str = "Backup network do not match the selected network!";

#[derive(Debug)]
pub enum ImportDescriptorModal {
    None,
    Export(ExportModal),
    Decrypt(DecryptModal),
}

impl ImportDescriptorModal {
    pub fn subscriptions(&self, hws: &HardwareWallets) -> Subscription<installer::Message> {
        if let ImportDescriptorModal::Export(modal) = &self {
            if let Some(sub) = modal.subscription() {
                sub.map(|m| installer::Message::ImportExport(ImportExportMessage::Progress(m)))
            } else {
                Subscription::none()
            }
        } else if let ImportDescriptorModal::Decrypt(modal) = &self {
            let mut batch = vec![hws.refresh().map(installer::Message::HardwareWallets)];
            if let Some(import_modal) = modal.modal.as_ref() {
                if let Some(sub) = import_modal.subscription() {
                    batch.push(sub.map(|p| {
                        installer::Message::ImportExport(ImportExportMessage::Progress(p))
                    }))
                }
            }
            Subscription::batch(batch)
        } else {
            Subscription::none()
        }
    }

    pub fn view<'a>(
        &'a self,
        content: Element<'a, installer::Message>,
    ) -> Element<'a, installer::Message> {
        match &self {
            ImportDescriptorModal::None => content,
            ImportDescriptorModal::Export(modal) => modal.view(content),
            ImportDescriptorModal::Decrypt(modal) => modal.view(content),
        }
    }

    pub fn update(&mut self, msg: installer::Message) -> Task<installer::Message> {
        match msg {
            installer::Message::ImportExport(ImportExportMessage::Progress(Progress::Xpub(
                xpub,
            ))) => {
                if let ImportDescriptorModal::Decrypt(modal) = self {
                    let _ = modal.update(Decrypt::CloseModal);
                    return modal.update(Decrypt::Xpub(xpub));
                }
            }
            installer::Message::ImportExport(m) => {
                if let ImportDescriptorModal::Export(modal) = self {
                    let task: Task<installer::Message> = modal.update(m);
                    return task;
                } else if let ImportDescriptorModal::Decrypt(modal) = self {
                    if let Some(mo) = &mut modal.modal {
                        let task: Task<installer::Message> = mo.update(m);
                        return task;
                    }
                }
            }
            installer::Message::Decrypt(msg) => {
                if let ImportDescriptorModal::Decrypt(modal) = self {
                    match msg {
                        Decrypt::Fetched(_, _)
                        | Decrypt::Xpub(_)
                        | Decrypt::XpubError(_)
                        | Decrypt::Mnemonic(_)
                        | Decrypt::MnemonicStatus(_, _)
                        | Decrypt::UnexpectedPayload(_)
                        | Decrypt::InvalidDescriptor
                        | Decrypt::ContentNotSupported
                        | Decrypt::PasteXpub
                        | Decrypt::SelectXpub
                        | Decrypt::PasteMnemonic
                        | Decrypt::SelectMnemonic
                        | Decrypt::SelectImportXpub
                        | Decrypt::None
                        | Decrypt::CloseModal
                        | Decrypt::ShowOptions(_) => return modal.update(msg),
                        Decrypt::Backup(_) | Decrypt::Close => {}
                    }
                }
            }
            _ => {}
        }
        Task::none()
    }

    pub fn is_some(&self) -> bool {
        !matches!(self, ImportDescriptorModal::None)
    }
}
