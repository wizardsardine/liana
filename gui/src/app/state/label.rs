use liana::miniscript::bitcoin;
use std::str::FromStr;
use std::{collections::HashMap, iter::IntoIterator, sync::Arc};

use crate::{
    app::{error::Error, message::Message, view},
    daemon::{
        model::{LabelItem, Labelled},
        Daemon,
    },
};
use iced::Command;
use liana_ui::component::form;

#[derive(Default)]
pub struct LabelsEdited(HashMap<String, form::Value<String>>);

impl LabelsEdited {
    pub fn cache(&self) -> &HashMap<String, form::Value<String>> {
        &self.0
    }
    pub fn update<'a, T: IntoIterator<Item = &'a mut dyn Labelled>>(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        targets: T,
    ) -> Result<Command<Message>, Error> {
        match message {
            Message::View(view::Message::Label(labelled, msg)) => match msg {
                view::LabelMessage::Edited(value) => {
                    let valid = value.len() <= 100;
                    if let Some(label) = self.0.get_mut(&labelled) {
                        label.valid = valid;
                        label.value = value;
                    } else {
                        self.0.insert(labelled, form::Value { valid, value });
                    }
                }
                view::LabelMessage::Cancel => {
                    self.0.remove(&labelled);
                }
                view::LabelMessage::Confirm => {
                    if let Some(label) = self.0.get(&labelled).cloned() {
                        return Ok(Command::perform(
                            async move {
                                if let Some(item) = label_item_from_str(&labelled) {
                                    daemon.update_labels(&HashMap::from([(
                                        item,
                                        label.value.clone(),
                                    )]))?;
                                }
                                Ok(HashMap::from([(labelled, label.value)]))
                            },
                            Message::LabelsUpdated,
                        ));
                    }
                }
            },
            Message::LabelsUpdated(res) => match res {
                Ok(new_labels) => {
                    for target in targets {
                        target.load_labels(&new_labels);
                    }
                    for (labelled, _) in new_labels {
                        self.0.remove(&labelled);
                    }
                }
                Err(e) => {
                    return Err(e);
                }
            },
            _ => {}
        };
        Ok(Command::none())
    }
}

pub fn label_item_from_str(s: &str) -> Option<LabelItem> {
    if let Ok(addr) = bitcoin::Address::from_str(s) {
        Some(LabelItem::Address(addr.assume_checked()))
    } else if let Ok(txid) = bitcoin::Txid::from_str(s) {
        Some(LabelItem::Txid(txid))
    } else if let Ok(outpoint) = bitcoin::OutPoint::from_str(s) {
        Some(LabelItem::OutPoint(outpoint))
    } else {
        None
    }
}
