use super::*;
use crate::installer::proposal::tests::TestDir;
use iced::advanced::{
    layout,
    renderer::Headless,
    widget::{Id, Operation, Tree},
    Layout,
};

const NOTICE: &str = "This proposal belongs to a different network than this Cube. Reopen the Cube and prepare a new proposal.";

#[derive(Default)]
struct Labels(Vec<String>);

impl Operation for Labels {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, text: &str) {
        self.0.push(text.to_owned());
    }
}

fn labels(home: &Home, renderer: &iced::Renderer) -> Vec<String> {
    let mut element = home.view();
    let mut tree = Tree::new(element.as_widget());
    let node = element.as_widget_mut().layout(
        &mut tree,
        renderer,
        &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1024.0, 768.0)),
    );
    let mut labels = Labels::default();
    element
        .as_widget_mut()
        .operate(&mut tree, Layout::new(&node), renderer, &mut labels);
    labels.0
}

#[test]
fn native_refusal_survives_startup_order_and_feature_revocation_visibly() {
    let _guard = crate::app::session::test_guard();
    // Native refusal is reached after its caller closes the setup session.
    crate::app::session::close();
    let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(16.0),
        Some("tiny-skia"),
    ))
    .expect("software renderer for native refusal regression");
    for network in coincube_mcp::Chain::ALL {
        for feature_first in [false, true] {
            let temp = TestDir::new();
            let root = CoincubeDirectory::new(temp.path().to_path_buf());
            let chain = network.as_str().parse::<ChainId>().unwrap();
            GlobalSettings::update_bitcoin_blake2b_beta(&GlobalSettings::path(&root), true)
                .unwrap();
            let (mut home, startup) = Home::after_native_refusal(root, chain, NOTICE);
            // Supply deterministic results for the startup tasks instead of
            // contacting Connect. Both delivery orders use the real update path.
            drop(startup);
            let checked = Message::Checked {
                for_chain: chain,
                res: Ok(State::NoCube { create_cube: false }),
            };
            let feature = Message::GlobalFeatureLoaded(0, false);
            for message in if feature_first {
                [feature, checked]
            } else {
                [checked, feature]
            } {
                drop(home.update(message));
                assert_eq!(home.network, chain);
                assert_eq!(home.error(), Some(NOTICE));
                let visible = labels(&home, &renderer);
                assert!(visible.iter().any(|label| label == NOTICE));
                assert!(visible.iter().any(|label| label == chain.label()));
                assert!(visible.iter().any(|label| label == "Back to Cube list"));
                assert!(!crate::app::session::is_open());
            }
            for enabled in [true, false] {
                drop(home.update(Message::GlobalFeatureLoaded(0, enabled)));
                let generation = home.connect_account.session_generation();
                drop(home.update(Message::View(ViewMessage::ConnectAccount(
                    ConnectAccountMessage::FeaturesLoaded(None, generation),
                ))));
                drop(home.reload_global_settings());
                assert_eq!(home.network, chain);
                assert!(labels(&home, &renderer).iter().any(|label| label == NOTICE));
            }
            if chain.is_blake2b() {
                assert!(!home.chain_availability(chain).is_available());
            }
        }
    }
}

#[test]
fn native_refusal_blocks_stale_actions_until_explicit_dismissal() {
    let _guard = crate::app::session::test_guard();
    // Native refusal is reached after its caller closes the setup session.
    crate::app::session::close();
    let temp = TestDir::new();
    let root = CoincubeDirectory::new(temp.path().to_path_buf());
    let chain = ChainId::BitcoinBlake2b;
    let (mut home, startup) = Home::after_native_refusal(root.clone(), chain, NOTICE);
    drop(startup);
    drop(home.update(Message::Checked {
        for_chain: chain,
        res: Ok(State::NoCube { create_cube: false }),
    }));
    for action in [
        ViewMessage::Run(0),
        ViewMessage::ShowCreateCube(true),
        ViewMessage::CreateCube,
        ViewMessage::SelectNetwork(ChainId::Bitcoin),
    ] {
        assert_eq!(home.update(Message::View(action)).units(), 0);
        assert_eq!(home.network, chain);
        assert_eq!(home.error(), Some(NOTICE));
        assert!(matches!(home.state, State::NoCube { create_cube: false }));
        assert!(!crate::app::session::is_open());
    }
    assert!(!home.chain_availability(chain).is_available());
    let cube = CubeSettings::new_with_raw_id("queued".into(), "Queued Cube".into(), chain);
    for queued in [Message::StartRecovery, Message::CubeCreated(Ok(cube))] {
        assert_eq!(home.update(queued).units(), 0);
        assert!(home.open_created_cube.is_none());
        assert!(home.open_after_registration.is_none());
        assert!(home.pending_restore_destination.is_none());
        assert!(matches!(home.state, State::NoCube { create_cube: false }));
    }
    drop(home.update(Message::View(ViewMessage::DismissNativeReturn)));
    assert!(home.native_return.is_none());
    assert_eq!(home.network, ChainId::Bitcoin);
    assert_eq!(home.error(), None);

    // Ordinary launchers still normalize an unavailable selection, including
    // after the refusal is dismissed. The return context grants no admission.
    let (mut ordinary, startup) = Home::new_for_chain(root, Some(chain));
    drop(startup);
    drop(ordinary.update(Message::GlobalFeatureLoaded(0, false)));
    assert_eq!(ordinary.network, home.network);
    assert!(!ordinary.chain_availability(chain).is_available());
    drop(home.update(Message::GlobalFeatureLoaded(0, true)));
    assert_eq!(home.network, ChainId::Bitcoin);
    assert_eq!(home.error(), None);
}
