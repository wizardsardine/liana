use super::{NavContext, SubItem};
use crate::app::menu::{ConnectSubMenu, Menu};
use coincube_ui::icon::{coins_icon, home_icon, key_icon, lightning_icon, person_icon, plus_icon};

/// Secondary-rail items for the Connect section.
///
/// Unauthenticated users see only a "Sign In" button that routes to
/// `Connect(Overview)` — the Connect panel renders the login form from
/// there. Once signed in the rail populates with the full section
/// (Lightning Address / Avatar / Contacts / Invites).
pub fn items(ctx: &NavContext) -> Vec<SubItem> {
    if !ctx.connect_authenticated {
        return vec![SubItem {
            label: "Sign In",
            icon: key_icon,
            route: Menu::Connect(ConnectSubMenu::Overview),
            matches: |m| matches!(m, Menu::Connect(_)),
        }];
    }

    let mut items = vec![
        SubItem {
            label: "Overview",
            icon: home_icon,
            route: Menu::Connect(ConnectSubMenu::Overview),
            matches: |m| matches!(m, Menu::Connect(ConnectSubMenu::Overview)),
        },
        SubItem {
            label: "Lightning Address",
            icon: lightning_icon,
            route: Menu::Connect(ConnectSubMenu::LightningAddress),
            matches: |m| matches!(m, Menu::Connect(ConnectSubMenu::LightningAddress)),
        },
        SubItem {
            label: "Avatar",
            icon: coins_icon,
            route: Menu::Connect(ConnectSubMenu::Avatar),
            matches: |m| matches!(m, Menu::Connect(ConnectSubMenu::Avatar)),
        },
        SubItem {
            label: "Contacts",
            icon: person_icon,
            route: Menu::Connect(ConnectSubMenu::Contacts),
            matches: |m| matches!(m, Menu::Connect(ConnectSubMenu::Contacts)),
        },
        SubItem {
            label: "Invites",
            icon: plus_icon,
            route: Menu::Connect(ConnectSubMenu::Invites),
            matches: |m| matches!(m, Menu::Connect(ConnectSubMenu::Invites)),
        },
    ];

    if crate::feature_flags::CUBE_MEMBERS_UI_ENABLED {
        items.push(SubItem {
            label: "Members",
            icon: person_icon,
            route: Menu::Connect(ConnectSubMenu::CubeMembers),
            matches: |m| matches!(m, Menu::Connect(ConnectSubMenu::CubeMembers)),
        });
    }

    items
}
