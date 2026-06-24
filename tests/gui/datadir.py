import json
import sqlite3
from dataclasses import dataclass
from pathlib import Path

from bip380.descriptors import Descriptor

from fixtures import single_key_desc, xpub_fingerprint
from test_framework.signer import SingleSigner
from test_framework.utils import USE_TAPROOT, wait_for


@dataclass
class GuiWallet:
    datadir: Path
    network_dir: Path
    data_dir: Path
    wallet_id: str
    descriptor: Descriptor
    signer: SingleSigner

    @classmethod
    def single_sig(cls, datadir, bitcoind, timestamp=1_700_000_000):
        datadir = Path(datadir)
        network_dir = datadir / "regtest"
        network_dir.mkdir(parents=True, exist_ok=True)

        signer = SingleSigner(is_taproot=USE_TAPROOT)
        (primary_fingerprint, primary_xpub), (recovery_fingerprint, recovery_xpub) = (
            (xpub_fingerprint(signer.primary_hd), signer.primary_hd.get_xpub()),
            (xpub_fingerprint(signer.recovery_hd), signer.recovery_hd.get_xpub()),
        )
        descriptor = Descriptor.from_str(
            single_key_desc(
                primary_fingerprint,
                primary_xpub,
                recovery_fingerprint,
                recovery_xpub,
                10,
                is_taproot=USE_TAPROOT,
            )
        )

        descriptor_checksum = str(descriptor).split("#", maxsplit=1)[1]
        wallet_id = f"{descriptor_checksum}-{timestamp}"
        data_dir = network_dir / "data" / wallet_id
        data_dir.mkdir(parents=True, exist_ok=True)

        _write_global_settings(datadir)
        _write_gui_config(network_dir)
        _write_gui_settings(
            network_dir,
            descriptor_checksum,
            timestamp,
            primary_fingerprint,
            recovery_fingerprint,
        )
        _write_daemon_config(data_dir, descriptor, bitcoind)

        return cls(
            datadir=datadir,
            network_dir=network_dir,
            data_dir=data_dir,
            wallet_id=wallet_id,
            descriptor=descriptor,
            signer=signer,
        )

    @property
    def db_path(self):
        return self.data_dir / "lianad.sqlite3"

    def wait_for_db(self):
        self._wait_for_db(lambda: self._db_value("SELECT COUNT(*) FROM wallets") == 1)

    def receive_index(self):
        return self._db_value("SELECT deposit_derivation_index FROM wallets LIMIT 1")

    def wait_for_receive_index(self, minimum):
        self._wait_for_db(lambda: self.receive_index() >= minimum)

    def receive_address(self, index):
        return self._db_value(
            "SELECT receive_address FROM addresses WHERE derivation_index = ?",
            (index,),
        )

    def chain_height(self):
        return self._db_value("SELECT blockheight FROM tip LIMIT 1")

    def confirmed_coin_count(self):
        return self._db_value(
            """
            SELECT COUNT(*)
            FROM coins
            WHERE blockheight IS NOT NULL
              AND spend_txid IS NULL
              AND is_immature = 0
            """
        )

    def wait_for_confirmed_coin_count(self, minimum):
        self._wait_for_db(lambda: self.confirmed_coin_count() >= minimum)

    def wait_for_sync(self, bitcoind):
        self._wait_for_db(lambda: self.chain_height() == bitcoind.rpc.getblockcount())

    def sign_psbt_base64(self, psbt_base64, recovery=False):
        from test_framework.serializations import PSBT

        return self.signer.sign_psbt(PSBT.from_base64(psbt_base64), recovery).to_base64()

    def _db_value(self, query, params=()):
        if not self.db_path.exists():
            raise FileNotFoundError(self.db_path)
        uri = f"file:{self.db_path}?mode=ro"
        with sqlite3.connect(uri, uri=True, timeout=1) as connection:
            row = connection.execute(query, params).fetchone()
        if row is None:
            raise LookupError(query)
        return row[0]

    def _wait_for_db(self, predicate):
        def ready():
            try:
                return predicate()
            except (FileNotFoundError, LookupError, sqlite3.Error):
                return False

        wait_for(ready)


def _write_global_settings(datadir):
    (datadir / "global_settings.json").write_text(
        json.dumps({"window_config": {"width": 1280.0, "height": 960.0}}, indent=2)
    )


def _write_gui_config(network_dir):
    (network_dir / "gui.toml").write_text(
        "\n".join(
            [
                'log_level = "debug"',
                "debug = false",
                "start_internal_bitcoind = false",
                "",
            ]
        )
    )


def _write_gui_settings(
    network_dir,
    descriptor_checksum,
    timestamp,
    primary_fingerprint,
    recovery_fingerprint,
):
    settings = {
        "wallets": [
            {
                "name": f"Liana-{descriptor_checksum}",
                "alias": "GUI regtest wallet",
                "descriptor_checksum": descriptor_checksum,
                "pinned_at": timestamp,
                "keys": [
                    {
                        "name": "primary",
                        "master_fingerprint": primary_fingerprint,
                        "provider_key": None,
                    },
                    {
                        "name": "recovery",
                        "master_fingerprint": recovery_fingerprint,
                        "provider_key": None,
                    },
                ],
                "hardware_wallets": [],
                "remote_backend_auth": None,
                "start_internal_bitcoind": False,
                "fiat_price": None,
            }
        ]
    }
    (network_dir / "settings.json").write_text(json.dumps(settings, indent=2))


def _write_daemon_config(data_dir, descriptor, bitcoind):
    cookie_path = Path(bitcoind.bitcoin_dir) / "regtest" / ".cookie"
    (data_dir / "daemon.toml").write_text(
        "\n".join(
            [
                f"data_directory = '{data_dir}'",
                "log_level = 'debug'",
                f'main_descriptor = "{descriptor}"',
                "",
                "[bitcoin_config]",
                "network = 'regtest'",
                "poll_interval_secs = 1",
                "",
                "[bitcoind_config]",
                f"cookie_path = '{cookie_path}'",
                f"addr = '127.0.0.1:{bitcoind.rpcport}'",
                "",
            ]
        )
    )
