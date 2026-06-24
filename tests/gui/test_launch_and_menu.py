import pytest


@pytest.mark.gui_smoke
def test_open_wallet_and_visit_all_menus(opened_liana_gui):
    app = opened_liana_gui

    app.assert_text("Balance", timeout=20)

    top_level_menus = [
        ("Receive", "Always generate"),
        ("Send", "Feerate"),
        ("Drafts", "Import"),
        ("Transactions", "Transactions"),
        ("Coins", "Coins"),
        ("Recovery", "Recovery"),
        ("Settings", "General"),
    ]

    for menu_label, expected_text in top_level_menus:
        app.click_text(menu_label, timeout=20)
        app.assert_text(expected_text, timeout=20)

    settings_sections = [
        ("General", "Fiat price"),
        ("Node", "Bitcoin Core"),
        ("Wallet", "Wallet descriptor"),
        ("Import", "Encrypted descriptor"),
        ("About", "Version"),
    ]

    app.click_text("Settings", timeout=20)
    app.assert_text("General", timeout=20)
    for index, (section_label, expected_text) in enumerate(settings_sections):
        app.assert_text("General", timeout=20)
        app.click_text(section_label, timeout=20)
        app.assert_text(expected_text, timeout=20)
        if index + 1 < len(settings_sections):
            app.click_at(540, 280)
            app.assert_text("General", timeout=20)
