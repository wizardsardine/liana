use super::*;
use crate::app::state::connect::{AddToCubeDialog, InviteCubeOption};
use crate::app::view::shared::landing::capture;
use crate::services::coincube::{Contact, ContactCube};

fn inspect(element: Element<'_, ConnectAccountMessage>) -> capture::Inspection {
    capture::inspect_rendered_full_window(
        "contacts-behavior",
        element,
        (1024, 1600),
        "dark",
        theme::Theme::dark(),
    )
}

fn cube() -> InviteCubeOption {
    InviteCubeOption {
        id: 9,
        name: "Synthetic Cube".into(),
        network: "bitcoin".into(),
    }
}

fn received() -> ReceivedInvite {
    ReceivedInvite {
        id: 7,
        owner_email: "sender@example.invalid".into(),
        role: ContactRole::Keyholder,
        expires_at: "2099-01-01T00:00:00Z".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn has_submit(messages: &[ConnectAccountMessage]) -> bool {
    messages.iter().any(|m| {
        matches!(
            m,
            ConnectAccountMessage::Contacts(ContactsMessage::SubmitInvite)
        )
    })
}

#[test]
fn list_transitions_from_loading_to_empty_and_received_invites() {
    let mut state = ConnectAccountPanel::new();
    state.contacts_state.loading = true;
    assert!(inspect(contacts_ux(&state)).contains("Loading…"));
    state.contacts_state.contacts = Some(vec![]);
    assert!(inspect(contacts_ux(&state)).contains("No contacts yet"));
    state.contacts_state.received_invites = Some(vec![received()]);
    let view = inspect(contacts_ux(&state));
    assert!(view.contains("Received invites"));
    assert!(view.contains("sender@example.invalid"));
    assert!(!view.contains("No contacts yet"));
    state.contacts_state.loading = false;
    state.contacts_state.error = Some("Synthetic load failure".into());
    assert!(inspect(contacts_ux(&state)).contains("Synthetic load failure"));
}

#[test]
fn invite_accept_button_emits_one_id_and_disables_while_accepting() {
    let invite = received();
    let messages = capture::click_text(
        received_invite_card(&invite, false),
        "Accept",
        0,
        (1024, 1000),
    );
    assert_eq!(
        messages
            .iter()
            .filter(|m| matches!(
                m,
                ConnectAccountMessage::Contacts(ContactsMessage::AcceptReceivedInvite(7))
            ))
            .count(),
        1
    );
    assert!(inspect(received_invite_card(&invite, true)).contains("Accepting…"));
    assert!(capture::click_text(
        received_invite_card(&invite, true),
        "Accepting…",
        0,
        (1024, 1000)
    )
    .is_empty());
}

#[test]
fn outbound_invites_expose_resend_and_revoke_only_for_pending_rows() {
    let mut state = ConnectAccountPanel::new();
    let invite = Invite {
        id: 12,
        owner_user_id: 1,
        invitee_email: "invitee@example.invalid".into(),
        invitee_user_id: None,
        role: ContactRole::Beneficiary,
        status: "pending".into(),
        expires_at: "2099-01-01T00:00:00Z".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    state.contacts_state.invites = Some(vec![invite.clone()]);
    assert!(inspect(contacts_ux(&state)).contains("Pending invites"));
    for (label, revoke) in [("Resend", false), ("Revoke", true)] {
        let messages = capture::click_text(invite_card(&invite), label, 0, (1024, 1000));
        assert!(messages.iter().any(|m| match m {
            ConnectAccountMessage::Contacts(ContactsMessage::RevokeInvite(12)) => revoke,
            ConnectAccountMessage::Contacts(ContactsMessage::ResendInvite(12)) => !revoke,
            _ => false,
        }));
    }
    state.contacts_state.invites.as_mut().unwrap()[0].status = "accepted".into();
    assert!(!inspect(contacts_ux(&state)).contains("invitee@example.invalid"));
}

#[test]
fn email_validation_gates_submit_and_busy_forms_cannot_resubmit() {
    let mut state = ConnectAccountPanel::new();
    state.contacts_state.step = ContactsStep::InviteForm;
    for (email, valid) in [
        ("", false),
        ("missing-at", false),
        ("a@localhost", false),
        ("synthetic@example.invalid", true),
        ("  synthetic@example.invalid  ", true),
    ] {
        state.contacts_state.invite_email = email.into();
        let messages = capture::click_text(contacts_ux(&state), "Send Invite", 0, (1024, 1600));
        assert_eq!(has_submit(&messages), valid, "{}", email);
    }
    state.contacts_state.invite_sending = true;
    assert!(inspect(contacts_ux(&state)).contains("Sending…"));
    let messages = capture::click_text(contacts_ux(&state), "Sending…", 0, (1024, 1600));
    assert!(!has_submit(&messages));
}

#[test]
fn invite_cube_conflicts_keep_form_values_and_offer_reselection() {
    let mut state = ConnectAccountPanel::new();
    state.contacts_state.step = ContactsStep::InviteForm;
    state.contacts_state.invite_available_cubes = Some(vec![cube()]);
    state.contacts_state.invite_cube_selections = vec![9];
    state.contacts_state.invite_cube_error = Some("Synthetic membership changed".into());
    state.contacts_state.error = Some("Synthetic invite failure".into());
    let view = inspect(contacts_ux(&state));
    assert!(view.contains("Synthetic Cube (bitcoin)"));
    assert!(view.contains("Synthetic membership changed"));
    assert!(view.contains("Synthetic invite failure"));
    assert_eq!(state.contacts_state.invite_cube_selections, [9]);
}

#[test]
fn contact_detail_distinguishes_missing_loading_retry_and_recovery_kit() {
    let mut state = ConnectAccountPanel::new();
    state.contacts_state.step = ContactsStep::Detail(5);
    assert!(inspect(contacts_ux(&state)).contains("Contact not found"));
    let contact: Contact = serde_json::from_value(serde_json::json!({"id":5, "role":"keyholder", "contactUser":{"id":6, "email":"contact@example.invalid"}, "createdAt":"2026-01-01T00:00:00Z"})).unwrap();
    state.contacts_state.contacts = Some(vec![contact]);
    let view = inspect(contacts_ux(&state));
    assert!(view.contains("contact@example.invalid"));
    assert!(view.contains("Loading cubes…"));
    assert!(view.contains("Add to Cube(s)…"));
    state.contacts_state.detail_cubes_error = Some("Synthetic cube fetch failure".into());
    assert!(inspect(contacts_ux(&state)).contains("Synthetic cube fetch failure"));
    let messages = capture::click_text(contacts_ux(&state), "Retry", 0, (1024, 1600));
    assert!(messages.iter().any(|m| matches!(
        m,
        ConnectAccountMessage::Contacts(ContactsMessage::ShowDetail(5))
    )));
    state.contacts_state.detail_cubes = Some(vec![]);
    assert!(inspect(contacts_ux(&state)).contains("No cubes found"));
    state.contacts_state.detail_cubes = Some(vec![ContactCube {
        id: 9,
        uuid: "synthetic-cube".into(),
        name: "Synthetic Cube".into(),
        network: "bitcoin".into(),
        has_recovery_kit: true,
    }]);
    let view = inspect(contacts_ux(&state));
    assert!(view.contains("Synthetic Cube") && view.contains("Recovery Kit"));
    state.contacts_state.contacts.as_mut().unwrap()[0].contact_user = None;
    let view = inspect(contacts_ux(&state));
    assert!(view.contains("unknown contact"));
    assert!(!view.contains("Add to Cube(s)…"));
}

#[test]
fn add_to_cube_dialog_requires_loaded_selection_and_prevents_duplicate_submit() {
    let mut dialog = AddToCubeDialog {
        contact_id: 5,
        contact_email: "contact@example.invalid".into(),
        candidate_cubes: None,
        selections: vec![],
        submitting: false,
        failures: Default::default(),
    };
    assert!(inspect(add_to_cube_dialog_card(&dialog)).contains("Loading eligible cubes…"));
    dialog.candidate_cubes = Some(vec![]);
    assert!(inspect(add_to_cube_dialog_card(&dialog)).contains("No eligible cubes"));
    dialog.candidate_cubes = Some(vec![cube()]);
    let click = |d: &AddToCubeDialog, label| {
        capture::click_text(add_to_cube_dialog_card(d), label, 0, (1024, 1600))
    };
    assert!(!click(&dialog, "Add").iter().any(|m| matches!(
        m,
        ConnectAccountMessage::Contacts(ContactsMessage::ConfirmAddToCube)
    )));
    dialog.selections = vec![9];
    assert!(click(&dialog, "Add").iter().any(|m| matches!(
        m,
        ConnectAccountMessage::Contacts(ContactsMessage::ConfirmAddToCube)
    )));
    dialog
        .failures
        .insert(9, "Synthetic per-cube failure".into());
    assert!(inspect(add_to_cube_dialog_card(&dialog)).contains("Synthetic per-cube failure"));
    dialog.submitting = true;
    assert!(!click(&dialog, "Adding…").iter().any(|m| matches!(
        m,
        ConnectAccountMessage::Contacts(ContactsMessage::ConfirmAddToCube)
    )));
}

#[test]
fn expiry_labels_use_utc_calendar_days_and_invalid_dates_are_unknown() {
    let today = chrono::Utc::now().date_naive();
    for (days, label) in [
        (-2, "Expired"),
        (0, "Expires today"),
        (1, "Expires in 1 day"),
        (5, "Expires in 5 days"),
    ] {
        let date = (today + chrono::Duration::days(days))
            .format("%Y-%m-%dT12:00:00Z")
            .to_string();
        assert_eq!(compute_days_until(&date), Some(days));
        assert!(inspect(expiry_element(&date)).contains(label));
    }
    assert_eq!(compute_days_until("invalid"), None);
    for role in [
        ContactRole::Keyholder,
        ContactRole::Beneficiary,
        ContactRole::Observer,
        ContactRole::Owner,
        ContactRole::Unknown,
    ] {
        let c = role_color(&role);
        assert!(c.a > 0.0);
    }
}
