pub mod bitcoind;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{self, Message},
        step::{node::bitcoind::DefineBitcoind, Step},
        view, Error,
    },
    node::NodeType,
};

use iced::Command;
use liana_ui::widget::Element;

#[derive(Clone)]
pub enum NodeDefinition {
    Bitcoind(DefineBitcoind),
}

impl NodeDefinition {
    fn new(node_type: NodeType) -> Self {
        match node_type {
            NodeType::Bitcoind => NodeDefinition::Bitcoind(DefineBitcoind::new()),
        }
    }

    fn node_type(&self) -> NodeType {
        match self {
            NodeDefinition::Bitcoind(_) => NodeType::Bitcoind,
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        match self {
            NodeDefinition::Bitcoind(def) => def.apply(ctx),
        }
    }

    fn can_try_ping(&self) -> bool {
        match self {
            NodeDefinition::Bitcoind(def) => def.can_try_ping(),
        }
    }

    fn load_context(&mut self, ctx: &Context) {
        match self {
            NodeDefinition::Bitcoind(def) => def.load_context(ctx),
        }
    }

    fn update(&mut self, message: message::DefineNode) -> Command<Message> {
        match self {
            NodeDefinition::Bitcoind(def) => def.update(message),
        }
    }

    fn view(&self) -> Element<Message> {
        match self {
            NodeDefinition::Bitcoind(def) => def.view(),
        }
    }

    fn ping(&self) -> Result<(), Error> {
        match self {
            NodeDefinition::Bitcoind(def) => def.ping(),
        }
    }
}

pub struct Node {
    definition: NodeDefinition,
    is_running: Option<Result<(), Error>>,
}

impl Node {
    fn new(node_type: NodeType) -> Self {
        Node {
            definition: NodeDefinition::new(node_type),
            is_running: None,
        }
    }
}

pub struct DefineNode {
    nodes: Vec<Node>,
    selected_node_type: NodeType,
}

impl From<DefineNode> for Box<dyn Step> {
    fn from(s: DefineNode) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl DefineNode {
    pub fn new(selected_node_type: NodeType) -> Self {
        let available_node_types = [
            // This is the order in which the available node types will be shown to the user.
            NodeType::Bitcoind,
        ];
        assert!(available_node_types.contains(&selected_node_type));

        let nodes = available_node_types
            .iter()
            .copied()
            .map(Node::new)
            .collect();

        Self {
            nodes,
            selected_node_type,
        }
    }

    pub fn selected_mut(&mut self) -> &mut Node {
        self.get_mut(self.selected_node_type)
            .expect("selected type must be present")
    }

    pub fn selected(&self) -> &Node {
        self.get(self.selected_node_type)
            .expect("selected type must be present")
    }

    pub fn get_mut(&mut self, node_type: NodeType) -> Option<&mut Node> {
        self.nodes
            .iter_mut()
            .find(|node| node.definition.node_type() == node_type)
    }

    pub fn get(&self, node_type: NodeType) -> Option<&Node> {
        self.nodes
            .iter()
            .find(|node| node.definition.node_type() == node_type)
    }

    fn ping_selected(&self) -> Command<Message> {
        let selected = self.selected().definition.clone();
        let node_type = selected.node_type();
        Command::perform(async move { selected.ping() }, move |res| {
            Message::DefineNode(message::DefineNode::PingResult((node_type, res)))
        })
    }

    fn update_node(
        &mut self,
        node_type: NodeType,
        message: message::DefineNode,
    ) -> Command<Message> {
        if let Some(node) = self.get_mut(node_type) {
            node.is_running = None;
            return node.definition.update(message);
        }
        Command::none()
    }
}

impl Default for DefineNode {
    fn default() -> Self {
        Self::new(NodeType::Bitcoind)
    }
}

impl Step for DefineNode {
    fn load_context(&mut self, ctx: &Context) {
        for node in self.nodes.iter_mut() {
            node.definition.load_context(ctx);
        }
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Command<Message> {
        if let Message::DefineNode(msg) = message {
            match msg {
                message::DefineNode::NodeTypeSelected(node_type) => {
                    self.selected_node_type = node_type;
                }
                message::DefineNode::Ping => {
                    return self.ping_selected();
                }
                message::DefineNode::PingResult((node_type, res)) => {
                    // Result may not be for the selected node type.
                    if let Some(node) = self.get_mut(node_type) {
                        node.is_running = Some(res);
                    }
                }
                msg @ message::DefineNode::DefineBitcoind(_) => {
                    return self.update_node(NodeType::Bitcoind, msg);
                }
            }
        }
        Command::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        self.selected_mut().definition.apply(ctx)
    }

    fn view(
        &self,
        _hws: &HardwareWallets,
        progress: (usize, usize),
        _email: Option<&str>,
    ) -> Element<Message> {
        view::define_bitcoin_node(
            progress,
            self.nodes.iter().map(|node| node.definition.node_type()),
            self.selected_node_type,
            self.selected().definition.view(),
            self.selected().is_running.as_ref(),
            self.selected().definition.can_try_ping(),
        )
    }

    fn load(&self) -> Command<Message> {
        self.ping_selected()
    }

    fn skip(&self, ctx: &Context) -> bool {
        !ctx.bitcoind_is_external || ctx.remote_backend.is_some()
    }
}
