from test_framework.bitcoind import Bitcoind
from test_framework.serializations import ser_compact_size


def inventory_payload(*entry_types):
    entries = b"".join(
        entry_type.to_bytes(4, "little") + bytes(32) for entry_type in entry_types
    )
    return ser_compact_size(len(entry_types)) + entries


def test_inv_contains_type_scans_all_entries():
    payload = inventory_payload(1, 2, 3)

    assert Bitcoind.inv_contains_type(payload, 2)
    assert not Bitcoind.inv_contains_type(payload, 4)


def test_inv_contains_type_decodes_compact_size():
    payload = inventory_payload(*([1] * 252), 2)

    assert Bitcoind.inv_contains_type(payload, 2)
