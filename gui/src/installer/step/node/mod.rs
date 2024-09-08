pub mod bitcoind;
pub mod electrum;

use crate::{
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{self, Message, PingType},
        step::{
            node::{bitcoind::DefineBitcoind, electrum::DefineElectrum},
            Step,
        },
        view, Error,
    },
    node::NodeType,
};

use iced::Command;
use liana_ui::widget::Element;

#[derive(Clone)]
pub enum NodeDefinition {
    Bitcoind(DefineBitcoind),
    Electrum(DefineElectrum),
}

impl NodeDefinition {
    fn new(node_type: NodeType) -> Self {
        match node_type {
            NodeType::Bitcoind => NodeDefinition::Bitcoind(DefineBitcoind::new()),
            NodeType::Electrum => NodeDefinition::Electrum(DefineElectrum::new()),
        }
    }

    fn node_type(&self) -> NodeType {
        match self {
            NodeDefinition::Bitcoind(_) => NodeType::Bitcoind,
            NodeDefinition::Electrum(_) => NodeType::Electrum,
        }
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        match self {
            NodeDefinition::Bitcoind(def) => def.apply(ctx),
            NodeDefinition::Electrum(def) => def.apply(ctx),
        }
    }

    fn can_try_ping(&self) -> bool {
        match self {
            NodeDefinition::Bitcoind(def) => def.can_try_ping(),
            NodeDefinition::Electrum(def) => def.can_try_ping(),
        }
    }

    fn load_context(&mut self, ctx: &Context) {
        match self {
            NodeDefinition::Bitcoind(def) => def.load_context(ctx),
            NodeDefinition::Electrum(_) => {
                // noop for now
            }
        }
    }

    fn update(&mut self, message: message::DefineNode) -> Command<Message> {
        match self {
            NodeDefinition::Bitcoind(def) => def.update(message),
            NodeDefinition::Electrum(def) => def.update(message),
        }
    }

    fn view(&self, is_running: bool) -> Element<Message> {
        match self {
            NodeDefinition::Bitcoind(def) => def.view(),
            NodeDefinition::Electrum(def) => def.view(is_running),
        }
    }

    fn ping(&self) -> Result<(), Error> {
        match self {
            NodeDefinition::Bitcoind(def) => def.ping(),
            NodeDefinition::Electrum(def) => def.ping(),
        }
    }
}

pub struct Node {
    definition: NodeDefinition,
    is_running: Option<Result<(), Error>>,
    waiting_for_ping_result: bool,
    ssl_ping_result: Option<Result<(), Error>>,
    tcp_ping_result: Option<Result<(), Error>>,
    original_ssl: bool,
}

impl Node {
    fn new(node_type: NodeType) -> Self {
        Node {
            definition: NodeDefinition::new(node_type),
            is_running: None,
            waiting_for_ping_result: false,
            ssl_ping_result: None,
            tcp_ping_result: None,
            original_ssl: false,
        }
    }

    fn is_ssl(&self) -> bool {
        if let NodeDefinition::Electrum(def) = &self.definition {
            def.is_ssl()
        } else {
            false
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
            NodeType::Electrum,
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

    fn update_node(
        &mut self,
        node_type: NodeType,
        message: message::DefineNode,
    ) -> Command<Message> {
        if let Some(node) = self.get_mut(node_type) {
            // Don't make changes while waiting for a ping result so that we
            // know which values the ping result applies to.
            if !node.waiting_for_ping_result {
                node.is_running = None;
                return node.definition.update(message);
            }
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
                    let selected = self.selected_mut();
                    // Make sure we don't send more than one ping request at a time
                    // so that we know which values the result applies to.
                    if !selected.waiting_for_ping_result {
                        selected.waiting_for_ping_result = true;
                        selected.is_running = None;
                        let def = selected.definition.clone();
                        let node_type = def.node_type();
                        match node_type {
                            NodeType::Bitcoind => {
                                return Command::perform(async move { def.ping() }, move |res| {
                                    Message::DefineNode(message::DefineNode::PingResult((
                                        PingType::Bitcoind,
                                        res,
                                    )))
                                });
                            }
                            // In order to handle user forgetting to specify address prefix, we
                            // ping both w/ tcp & ssl and update w/ correct value (prefix added if
                            // missing). Note: we always display a warning if the address is not
                            // ssl in order to not silently switch to tcp if the user intend
                            // connect to a ssl server & the server is misconfigured.
                            NodeType::Electrum => {
                                selected.tcp_ping_result = None;
                                selected.ssl_ping_result = None;
                                selected.original_ssl = selected.is_ssl();

                                let mut batch = Vec::new();
                                if let NodeDefinition::Electrum(def) = def {
                                    let ssl = def.clone().force_ssl();
                                    let tcp = def.force_tcp();
                                    let ssl_cmd =
                                        Command::perform(async move { ssl.ping() }, move |res| {
                                            Message::DefineNode(message::DefineNode::PingResult((
                                                PingType::ElectrumSsl,
                                                res,
                                            )))
                                        });
                                    let tcp_cmd =
                                        Command::perform(async move { tcp.ping() }, move |res| {
                                            Message::DefineNode(message::DefineNode::PingResult((
                                                PingType::ElectrumTcp,
                                                res,
                                            )))
                                        });
                                    batch.push(ssl_cmd);
                                    batch.push(tcp_cmd);
                                }
                                return Command::batch(batch);
                            }
                        }
                    }
                }
                message::DefineNode::PingResult((ping_type, res)) => {
                    // Result may not be for the selected node type.
                    if let Some(node) = self.get_mut(ping_type.node_type()) {
                        // Make sure we're expecting the ping result. Otherwise, the user may have changed values
                        // and so the ping result may not apply to the current values.
                        if node.waiting_for_ping_result {
                            match ping_type {
                                PingType::Bitcoind => {
                                    node.waiting_for_ping_result = false;
                                    node.is_running = Some(res);
                                }
                                ping_type => {
                                    if let PingType::ElectrumTcp = ping_type {
                                        node.tcp_ping_result = Some(res);
                                    } else {
                                        node.ssl_ping_result = Some(res);
                                    }
                                    // we wait receiving both ping before decide using tcp or
                                    // ssl
                                    if let (Some(ssl), Some(tcp)) =
                                        (&node.ssl_ping_result, &node.tcp_ping_result)
                                    {
                                        node.waiting_for_ping_result = false;
                                        if let NodeDefinition::Electrum(def) = &mut node.definition
                                        {
                                            if ssl.is_ok() {
                                                *def = def.clone().force_ssl();
                                            } else if tcp.is_ok() {
                                                *def = def.clone().force_tcp();
                                            }
                                        }

                                        node.is_running = if node.original_ssl || ssl.is_ok() {
                                            node.ssl_ping_result.clone()
                                        } else {
                                            node.tcp_ping_result.clone()
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
                msg @ message::DefineNode::DefineBitcoind(_) => {
                    return self.update_node(NodeType::Bitcoind, msg);
                }
                msg @ message::DefineNode::DefineElectrum(_) => {
                    return self.update_node(NodeType::Electrum, msg);
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
        // TODO: Make input fields read-only while waiting for a ping result.
        view::define_bitcoin_node(
            progress,
            self.nodes.iter().map(|node| node.definition.node_type()),
            self.selected_node_type,
            self.selected()
                .definition
                .view(if let Some(r) = self.selected().is_running.as_ref() {
                    r.is_ok()
                } else {
                    false
                }),
            self.selected().is_running.as_ref(),
            self.selected().definition.can_try_ping(),
            self.selected().waiting_for_ping_result,
        )
    }

    fn skip(&self, ctx: &Context) -> bool {
        !ctx.bitcoind_is_external || ctx.remote_backend.is_some()
    }
}
