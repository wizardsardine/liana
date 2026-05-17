import pytest


@pytest.mark.gui_core
def test_generate_receive_address_from_gui(opened_liana_gui, gui_wallet):
    app = opened_liana_gui

    address = generate_receive_address(app, gui_wallet)

    assert gui_wallet.receive_index() == 1
    assert address.startswith("bcrt1")


@pytest.mark.gui_core
def test_gui_generated_address_receives_confirmed_deposit(
    opened_liana_gui, gui_wallet, bitcoind
):
    app = opened_liana_gui

    address = generate_receive_address(app, gui_wallet)

    txid = bitcoind.rpc.sendtoaddress(address, 0.01)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    gui_wallet.wait_for_sync(bitcoind)
    gui_wallet.wait_for_confirmed_coin_count(1)

    app.click_text("Coins", timeout=20)
    app.assert_text("Coins", timeout=20)
    app.click_text("Transactions", timeout=20)
    app.assert_text("Transactions", timeout=20)


def generate_receive_address(app, gui_wallet):
    app.click_text("Receive", timeout=20)
    app.assert_text("Always generate", timeout=20)
    app.click_at(1100, 278)
    gui_wallet.wait_for_receive_index(1)
    return gui_wallet.receive_address(1)
