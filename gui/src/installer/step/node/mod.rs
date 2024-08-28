pub mod bitcoind;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{self, Message},
        step::{node::bitcoind::DefineBitcoind, Step},
        view, Error,
    },
};

use iced::Command;
use liana_ui::widget::Element;

pub struct Node {
    definition: DefineBitcoind,
    is_running: Option<Result<(), Error>>,
}

impl Node {
    fn new() -> Self {
        Node {
            definition: DefineBitcoind::new(),
            is_running: None,
        }
    }
}

pub struct DefineNode {
    node: Node,
}

impl From<DefineNode> for Box<dyn Step> {
    fn from(s: DefineNode) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl DefineNode {
    pub fn new() -> Self {
        Self { node: Node::new() }
    }

    fn ping(&self) -> Command<Message> {
        let def = self.node.definition.clone();
        Command::perform(async move { def.ping() }, move |res| {
            Message::DefineNode(message::DefineNode::PingResult(res))
        })
    }

    fn update_node(&mut self, message: message::DefineNode) -> Command<Message> {
        self.node.is_running = None;
        self.node.definition.update(message)
    }
}

impl Default for DefineNode {
    fn default() -> Self {
        Self::new()
    }
}

impl Step for DefineNode {
    fn load_context(&mut self, ctx: &Context) {
        self.node.definition.load_context(ctx);
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Command<Message> {
        if let Message::DefineNode(msg) = message {
            match msg {
                message::DefineNode::Ping => {
                    return self.ping();
                }
                message::DefineNode::PingResult(res) => {
                    self.node.is_running = Some(res);
                }
                msg @ message::DefineNode::DefineBitcoind(_) => {
                    return self.update_node(msg);
                }
            }
        }
        Command::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        self.node.definition.apply(ctx)
    }

    fn view(
        &self,
        _hws: &HardwareWallets,
        progress: (usize, usize),
        _email: Option<&str>,
    ) -> Element<Message> {
        view::define_bitcoin_node(
            progress,
            self.node.definition.view(),
            self.node.is_running.as_ref(),
            self.node.definition.can_try_ping(),
        )
    }

    fn load(&self) -> Command<Message> {
        self.ping()
    }

    fn skip(&self, ctx: &Context) -> bool {
        !ctx.bitcoind_is_external || ctx.remote_backend.is_some()
    }
}
