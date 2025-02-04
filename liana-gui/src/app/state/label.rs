use liana::miniscript::bitcoin;
use std::str::FromStr;
use std::{collections::HashMap, iter::IntoIterator, sync::Arc};

use crate::{
    app::{error::Error, message::Message, view},
    daemon::{
        model::{LabelItem, LabelsLoader},
        Daemon,
    },
};
use iced::Task;
use liana_ui::component::form;

#[derive(Default)]
pub struct LabelsEdited(HashMap<String, form::Value<String>>);

impl LabelsEdited {
    pub fn cache(&self) -> &HashMap<String, form::Value<String>> {
        &self.0
    }
    pub fn update<'a, T: IntoIterator<Item = &'a mut dyn LabelsLoader>>(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        targets: T,
    ) -> Result<Task<Message>, Error> {
        match message {
            Message::View(view::Message::Label(items, msg)) => match msg {
                view::LabelMessage::Edited(value) => {
                    let valid = value.len() <= 100;
                    for item in items {
                        if let Some(label) = self.0.get_mut(&item) {
                            label.valid = valid;
                            label.value.clone_from(&value);
                        } else {
                            self.0.insert(
                                item,
                                form::Value {
                                    valid,
                                    value: value.clone(),
                                },
                            );
                        }
                    }
                }
                view::LabelMessage::Cancel => {
                    for item in items {
                        self.0.remove(&item);
                    }
                }
                view::LabelMessage::Confirm => {
                    let mut updated_labels = HashMap::<LabelItem, Option<String>>::new();
                    let mut updated_labels_str = HashMap::<String, Option<String>>::new();
                    for item in items {
                        if let Some(label) = self.0.get(&item).cloned() {
                            let item_str = label_item_from_str(&item);
                            if label.value.is_empty() {
                                updated_labels.insert(item_str, None);
                                updated_labels_str.insert(item, None);
                            } else {
                                updated_labels.insert(item_str, Some(label.value.clone()));
                                updated_labels_str.insert(item, Some(label.value));
                            }
                        }
                    }
                    return Ok(Task::perform(
                        async move {
                            daemon.update_labels(&updated_labels).await?;
                            Ok(updated_labels_str)
                        },
                        Message::LabelsUpdated,
                    ));
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
        Ok(Task::none())
    }
}

pub fn label_item_from_str(s: &str) -> LabelItem {
    if let Ok(addr) = bitcoin::Address::from_str(s) {
        LabelItem::Address(addr.assume_checked())
    } else if let Ok(txid) = bitcoin::Txid::from_str(s) {
        LabelItem::Txid(txid)
    } else if let Ok(outpoint) = bitcoin::OutPoint::from_str(s) {
        LabelItem::OutPoint(outpoint)
    } else {
        unreachable!()
    }
}
