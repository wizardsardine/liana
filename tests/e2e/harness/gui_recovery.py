"""Draft desktop GUI recovery coordinator. No headless action can pass this suite.

The session-bound CUA adapter is intentionally unavailable until implemented and
reviewed. The CLI therefore reports BLOCKED rather than importing old receipts,
executing arbitrary driver commands or counting synthetic state as GUI proof.
"""
import base64
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time
import uuid

from .gui_verify import CHAINS, funding_parent, references, verify_export
from .state import Blocked, Failed, Journal, atomic_write, digest, private_read

PREREQUISITE = "9e9a1f25995b4be9a6ab188240a2bf92d5f3209b"
CACHE_SERVICE = "io.coincube.tenshu.device-secret.v1"
STAGES = ("preflight", "create", "mode_parity", "backup", "exit_original", "exclude_original_cache", "restore", "restart", "wrong_pin",
          "unlock", "funding", "sign_export", "verify", "retain")
REPLY_FIELDS = {
    "create": {"developer_mode", "cube_open", "vault_open", "process", "credentials_saved", "credential_namespace"},
    "mode_parity": {"off_creation_hidden", "off_identity", "on_identity", "developer_mode_restored_on", "gate_bypassed"},
    "backup": {"product_flow", "has_encrypted_seed", "has_encrypted_descriptor", "identity_sha256"},
    "exit_original": {"process", "exited_normally", "process_absent", "hot_cache_absent"},
    "exclude_original_cache": {"method", "service", "account", "original_record_absent", "other_records_unchanged", "fixture_owned", "funded", "full_kit_sha256", "isolation_receipt_sha256"},
    "restore": {"new_pin_distinct", "full_kit_sha256", "cube_open", "vault_open", "original_process_absent", "process"},
    "restart": {"process", "previous_process_absent", "exited_normally", "locked_cube_visible"},
    "wrong_pin": {"wrong_pin_refused", "cube_open", "fixture_unchanged", "process"},
    "unlock": {"process", "new_pin_used", "cube_open", "vault_open"},
    "funding": {"process", "wallet_recognized", "funding_facility", "chain_oracle_verified", "injected_utxo"},
    "sign_export": {"process", "gui_sign_clicked", "ready_visible", "self_transfer", "native_save_dialog", "broadcast_clicked", "receive_index", "change_index"},
    "retain": {"all_owned_processes_exited", "profiles_retained", "funded_roots_retained", "personal_keys_untouched", "credential_namespace"},
}
COMMON_REPLY = {"request_sha256", "scope", "session", "chain", "stage", "nonce", "broadcast", "evidence_source", "gui_action", "sensitive_output_logged", "screenshots_of_secrets", "observation_sha256"}
OBSERVATIONS = {"create", "mode_parity", "backup", "restore", "restart", "wrong_pin", "unlock", "funding", "sign_export"}


def require(value, code):
    if not value:
        raise Failed("GUI_RECOVERY_" + code, "Desktop GUI recovery validation failed: " + code)


def validate_config(value):
    require(isinstance(value, dict) and set(value) == {"version", "checkout", "revision", "app_receipt", "driver", "developer_mode", "maximum_fee_sats", "funding"}, "CONFIG")
    require(type(value["version"]) is int and value["version"] == 1
            and value["driver"] == {"kind": "cua_repl", "version": 1}
            and value["developer_mode"] is True and type(value["maximum_fee_sats"]) is int
            and 1 <= value["maximum_fee_sats"] <= 10000, "CONFIG")
    require(all(isinstance(value[k], str) and Path(value[k]).is_absolute() for k in ("checkout", "app_receipt"))
            and isinstance(value["revision"], str) and re.fullmatch(r"[0-9a-f]{40}", value["revision"]), "CONFIG")
    require(isinstance(value["funding"], dict) and set(value["funding"]) == set(CHAINS), "FUNDING_CONFIG")
    for chain, policy in value["funding"].items():
        require(isinstance(policy, dict) and set(policy) == {"facility", "chain", "maximum_sats"}
                and policy["chain"] == chain and policy["facility"] == "approved_disposable_qa"
                and type(policy["maximum_sats"]) is int and 10000 <= policy["maximum_sats"] <= 1000000, "FUNDING_CONFIG")
    return deepcopy(value)


def prerequisite(config):
    try:
        checkout = Path(config["checkout"])
        remote = subprocess.check_output(["git", "remote", "get-url", "origin"], cwd=checkout, stderr=subprocess.DEVNULL).decode().strip()
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout, stderr=subprocess.DEVNULL).decode().strip()
        require(remote in ("git@github.com:coincubetech/coincube.git", "https://github.com/coincubetech/coincube.git")
                and head == config["revision"], "SOURCE")
        result = subprocess.run(["git", "merge-base", "--is-ancestor", PREREQUISITE, head], cwd=checkout,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
    except (OSError, subprocess.SubprocessError):
        raise Blocked("GUI_RECOVERY_SOURCE", "The pinned desktop source and recovery prerequisite could not be verified.") from None
    if result.returncode:
        raise Blocked("GUI_RECOVERY_PR1174", "PR #1174's reviewed recovery behavior is not integrated in the pinned app source; no GUI action was attempted.")


class CuaWorker:
    """Reserved real session adapter. Not an executable/receipt import interface."""
    def connect(self, challenge):
        raise Blocked("GUI_RECOVERY_CUA_UNAVAILABLE", "The reviewed session-bound desktop CUA worker adapter is not implemented; no GUI action was attempted.")

    def perform(self, request):
        raise Blocked("GUI_RECOVERY_CUA_UNAVAILABLE", "No desktop CUA session is bound to this run.")


def read_identity(path, chain):
    value = json.loads(private_read(path))
    require(isinstance(value, dict) and set(value) == {"cube_id", "root_fingerprint", "descriptor", "chain"}
            and value["chain"] == chain and isinstance(value["cube_id"], str)
            and str(uuid.UUID(value["cube_id"])) == value["cube_id"]
            and re.fullmatch(r"[0-9a-f]{8}", value["root_fingerprint"])
            and isinstance(value["descriptor"], str) and value["descriptor"].startswith("wsh(")
            and len(value["descriptor"]) < 20000, "IDENTITY")
    require(not any(token in value["descriptor"] for token in ("xprv", "tprv", "[REDACTED]")), "IDENTITY")
    return value


def process(value, profile, app):
    require(isinstance(value, dict) and set(value) == {"pid", "start_id", "profile", "app_sha256"}
            and type(value["pid"]) is int and value["pid"] > 0 and isinstance(value["start_id"], str)
            and value["start_id"] and value["profile"] == str(profile)
            and value["app_sha256"] == app, "PROCESS")
    return deepcopy(value)


class GuiRecovery:
    def __init__(self, config, directory, *, worker=None, offline=False):
        self.config = validate_config(config)
        require(type(offline) is bool and (worker is None or offline), "DRIVER_OVERRIDE")
        self.offline = offline
        self.worker = worker if offline else CuaWorker()
        self.root = Path(directory).absolute()
        self.journal = Journal(self.root, {"kind": "gui_recovery", "config": config, "offline_selftest": offline})
        self.scope = str(uuid.uuid5(uuid.NAMESPACE_URL, "coincube-gui-recovery/" + str(self.root)))
        self.handshake = None
        self.completed = {chain: set() for chain in CHAINS}
        self.stopped = set()

    def close(self):
        self.journal.close()  # Never deletes profiles, credentials, kits, funding sources or history.

    def directory(self, chain):
        require(chain in CHAINS, "CHAIN")
        path = self.root / chain
        if path.is_symlink(): raise Failed("GUI_RECOVERY_DIRECTORY", "GUI case directory cannot be a symlink.")
        path.mkdir(mode=0o700, exist_ok=True)
        require(path.stat().st_uid == os.getuid() and not path.stat().st_mode & 0o077, "DIRECTORY")
        return path

    def state(self, chain):
        return self.journal.data["artifacts"].setdefault(chain, {})

    def save(self, chain, key, value):
        state = self.state(chain)
        if key in state:
            require(state[key] == value, "CHANGED")
        state[key] = deepcopy(value); self.journal.save()

    def request(self, chain, stage):
        directory = self.directory(chain)
        nonce = uuid.uuid4().hex
        request = {"version": 1, "scope": self.scope, "chain": chain, "stage": stage, "nonce": nonce,
                   "case_directory": str(directory), "original_profile": str(directory / "original"),
                   "restored_profile": str(directory / "restored"), "broadcast": False,
                   "app_sha256": self.handshake["app_sha256"], "session": self.handshake["session"]}
        # Intent is durable before any credential/funding/GUI action. An
        # interrupted action is quarantined, never replayed as another action.
        pending_key = chain + ":" + stage
        pending = self.journal.data["artifacts"].get("pending_action")
        if pending is not None:
            raise Blocked("GUI_RECOVERY_UNCERTAIN", "A prior GUI action has an uncertain outcome; retain its profile and reconcile the exact action before retrying.")
        self.journal.artifact("pending_action", {"key": pending_key, "request": request})
        reply = self.worker.perform(deepcopy(request))
        require(isinstance(reply, dict) and reply.get("request_sha256") == digest(request)
                and reply.get("scope") == self.scope and reply.get("session") == self.handshake["session"]
                and reply.get("chain") == chain and reply.get("stage") == stage
                and reply.get("nonce") == nonce and reply.get("broadcast") is False, "ACTION_BINDING")
        require(not set(reply) - COMMON_REPLY - REPLY_FIELDS[stage], "UNEXPECTED_REPLY_FIELDS")
        require(reply.get("evidence_source") == "live_cua" and reply.get("gui_action") is True
                and reply.get("sensitive_output_logged") is False and reply.get("screenshots_of_secrets") is False, "GUI_EVIDENCE")
        if stage in OBSERVATIONS:
            require(isinstance(reply.get("observation_sha256"), str)
                    and re.fullmatch(r"[0-9a-f]{64}", reply["observation_sha256"]), "OBSERVATION")
        self.save(chain, stage + "_receipt", reply)
        del self.journal.data["artifacts"]["pending_action"]; self.journal.save()
        return reply

    def run(self, chain, stage):
        require(chain in CHAINS and stage in STAGES, "STAGE")
        if chain in self.stopped: raise Blocked("GUI_RECOVERY_DEPENDENCY", "An earlier GUI recovery stage did not complete.")
        require(set(STAGES[:STAGES.index(stage)]) <= self.completed[chain], "STAGE_ORDER")
        try:
            getattr(self, "stage_" + stage)(chain)
            self.completed[chain].add(stage)
            self.journal.outcome(chain + ":" + stage, "offline_passed" if self.offline else "passed")
        except Blocked as error:
            self.stopped.add(chain)
            self.journal.outcome(chain + ":" + stage, "failed" if isinstance(error, Failed) else "blocked", error.code)
            raise

    def stage_preflight(self, chain):
        if not self.offline: prerequisite(self.config)
        if self.handshake is None:
            challenge = {"version": 1, "scope": self.scope, "nonce": uuid.uuid4().hex, "revision": self.config["revision"],
                         "driver": "cua_repl", "chains": list(CHAINS), "broadcast": False}
            hello = self.worker.connect(deepcopy(challenge))
            require(isinstance(hello, dict) and hello.get("challenge_sha256") == digest(challenge)
                    and hello.get("driver") == "cua_repl" and hello.get("real_gui") is True
                    and hello.get("scope") == self.scope and hello.get("session")
                    and hello.get("revision") == self.config["revision"]
                    and hello.get("secret_store") == "macos_data_protection_keychain"
                    and hello.get("no_broadcast") is True and hello.get("approved_funding_chains") == list(CHAINS)
                    and re.fullmatch(r"[0-9a-f]{64}", str(hello.get("app_sha256"))), "HANDSHAKE")
            if not self.offline:
                receipt = json.loads(private_read(self.config["app_receipt"]))
                require(receipt.get("revision") == self.config["revision"] and receipt.get("app_sha256") == hello["app_sha256"], "APP")
            if hello.get("cache_isolation") != "fixture_device_secret_quarantine_v1":
                raise Blocked("GUI_RECOVERY_CACHE_ISOLATION_UNAVAILABLE", "The original Cube-UUID Keychain record cannot be excluded by a fresh app profile alone; a reviewed fixture-only isolator is required.")
            self.handshake = deepcopy(hello)
            self.journal.artifact("runtime", {"scope": self.scope, "revision": hello["revision"], "session": hello["session"],
                "app_sha256": hello["app_sha256"], "offline_selftest": self.offline})
        directory = self.directory(chain)
        for profile in ("original", "restored"):
            require(not (directory / profile).exists() and not (directory / profile).is_symlink(), "NOT_FRESH")
        self.save(chain, "initial_absence", {"original": True, "restored": True})

    def stage_create(self, chain):
        reply = self.request(chain, "create"); directory = self.directory(chain)
        require(reply.get("developer_mode") is True and reply.get("cube_open") is True and reply.get("vault_open") is True, "CREATE")
        self.save(chain, "original_process", process(reply["process"], directory / "original", self.handshake["app_sha256"]))
        self.save(chain, "identity", read_identity(directory / "original-identity.json", chain))
        require(reply.get("credentials_saved") is True and reply.get("credential_namespace") == self.scope, "CREDENTIALS")

    def stage_mode_parity(self, chain):
        reply = self.request(chain, "mode_parity")
        require(reply.get("off_creation_hidden") is True and reply.get("off_identity") == reply.get("on_identity") == digest(self.state(chain)["identity"])
                and reply.get("developer_mode_restored_on") is True and reply.get("gate_bypassed") is False, "MODE_PARITY")

    def stage_backup(self, chain):
        reply = self.request(chain, "backup")
        require(reply.get("product_flow") == "full_encrypted_recovery_kit" and reply.get("has_encrypted_seed") is True
                and reply.get("has_encrypted_descriptor") is True and reply.get("identity_sha256") == digest(self.state(chain)["identity"]), "FULL_KIT")
        kit = json.loads(private_read(self.directory(chain) / "encrypted-kit.json"))
        require(set(kit) == {"encrypted_seed", "encrypted_descriptor", "cube_id", "chain"}
                and kit["cube_id"] == self.state(chain)["identity"]["cube_id"] and kit["chain"] == chain
                and all(isinstance(kit[k], str) and len(kit[k]) >= 80 for k in ("encrypted_seed", "encrypted_descriptor")), "FULL_KIT")
        for field in ("encrypted_seed", "encrypted_descriptor"):
            try: envelope = base64.b64decode(kit[field], validate=True)
            except ValueError: raise Failed("GUI_RECOVERY_FULL_KIT", "The recovery kit contains an invalid ciphertext envelope.") from None
            require(len(envelope) >= 52 and envelope[0] in (1, 2) and envelope[1] == 1, "FULL_KIT")
        require(kit["encrypted_seed"] != kit["encrypted_descriptor"], "FULL_KIT")
        self.save(chain, "kit_sha256", digest(kit))

    def stage_exit_original(self, chain):
        reply = self.request(chain, "exit_original")
        require(reply.get("process") == self.state(chain)["original_process"] and reply.get("exited_normally") is True
                and reply.get("process_absent") is True and reply.get("hot_cache_absent") is True, "ORIGINAL_ALIVE")
        require(not (self.directory(chain) / "restored").exists(), "RESTORE_NOT_ABSENT")

    def stage_exclude_original_cache(self, chain):
        reply = self.request(chain, "exclude_original_cache")
        identity = self.state(chain)["identity"]
        require(reply.get("method") == "fixture_device_secret_quarantine_v1"
                and reply.get("service") == CACHE_SERVICE and reply.get("account") == identity["cube_id"]
                and reply.get("original_record_absent") is True and reply.get("other_records_unchanged") is True
                and reply.get("fixture_owned") is True and reply.get("funded") is False
                and reply.get("full_kit_sha256") == self.state(chain)["kit_sha256"], "CACHE_ISOLATION")
        raw = private_read(self.directory(chain) / "keychain-isolation.json")
        proof = json.loads(raw)
        require(set(proof) == {"version", "scope", "nonce", "method", "service", "account", "original_record_absent", "other_records_unchanged", "identity_sha256", "full_kit_sha256", "original_process", "quarantine_locator_sha256"}
                and type(proof["version"]) is int and proof["version"] == 1
                and proof["scope"] == self.scope and proof["nonce"] == reply["nonce"]
                and proof["method"] == reply["method"] and proof["service"] == CACHE_SERVICE
                and proof["account"] == identity["cube_id"] and proof["original_record_absent"] is True
                and proof["other_records_unchanged"] is True and proof["identity_sha256"] == digest(identity)
                and proof["full_kit_sha256"] == self.state(chain)["kit_sha256"]
                and proof["original_process"] == self.state(chain)["original_process"]
                and re.fullmatch(r"[0-9a-f]{64}", str(proof["quarantine_locator_sha256"]))
                and hashlib.sha256(raw).hexdigest() == reply["isolation_receipt_sha256"], "CACHE_ISOLATION")
        self.save(chain, "cache_isolation", proof)

    def stage_restore(self, chain):
        require(self.state(chain).get("cache_isolation", {}).get("original_record_absent") is True, "CACHE_ISOLATION")
        require(not (self.directory(chain) / "restored").exists(), "RESTORE_NOT_ABSENT")
        reply = self.request(chain, "restore"); directory = self.directory(chain)
        identity = read_identity(directory / "restored-identity.json", chain)
        require(identity == self.state(chain)["identity"] and reply.get("new_pin_distinct") is True
                and reply.get("full_kit_sha256") == self.state(chain)["kit_sha256"]
                and reply.get("cube_open") is True and reply.get("vault_open") is True
                and reply.get("original_process_absent") is True, "RESTORE")
        current = process(reply["process"], directory / "restored", self.handshake["app_sha256"])
        original = self.state(chain)["original_process"]
        require((current["pid"], current["start_id"]) != (original["pid"], original["start_id"]), "HOT_CACHE")
        self.save(chain, "restore_process", current)

    def stage_restart(self, chain):
        reply = self.request(chain, "restart")
        current = process(reply["process"], self.directory(chain) / "restored", self.handshake["app_sha256"])
        require(reply.get("previous_process_absent") is True and reply.get("exited_normally") is True
                and (current["pid"], current["start_id"]) != (self.state(chain)["restore_process"]["pid"], self.state(chain)["restore_process"]["start_id"])
                and reply.get("locked_cube_visible") is True, "RESTART")
        self.save(chain, "restart_process", current)

    def stage_wrong_pin(self, chain):
        reply = self.request(chain, "wrong_pin")
        require(reply.get("wrong_pin_refused") is True and reply.get("cube_open") is False
                and reply.get("fixture_unchanged") is True and reply.get("process") == self.state(chain)["restart_process"], "WRONG_PIN")

    def stage_unlock(self, chain):
        reply = self.request(chain, "unlock")
        require(reply.get("process") == self.state(chain)["restart_process"] and reply.get("new_pin_used") is True
                and reply.get("cube_open") is True and reply.get("vault_open") is True
                and read_identity(self.directory(chain) / "reopened-identity.json", chain) == self.state(chain)["identity"], "UNLOCK")

    def stage_funding(self, chain):
        reply = self.request(chain, "funding")
        require(reply.get("process") == self.state(chain)["restart_process"] and reply.get("wallet_recognized") is True
                and reply.get("funding_facility") == "approved_disposable_qa" and reply.get("chain") == chain
                and reply.get("chain_oracle_verified") is True and reply.get("injected_utxo") is False, "FUNDING")
        funding = json.loads(private_read(self.directory(chain) / "funding-proof.json"))
        require(funding.get("chain") == chain and funding.get("wallet_recognized") is True
                and type(funding.get("vout")) is int and funding["vout"] >= 0
                and isinstance(funding.get("txid"), str) and re.fullmatch(r"[0-9a-f]{64}", funding["txid"])
                and type(funding.get("value_sats")) is int
                and 10000 <= funding["value_sats"] <= self.config["funding"][chain]["maximum_sats"]
                and funding.get("chain_oracle_verified") is True, "FUNDING")
        funding_parent(funding, references()[2], self.config["funding"][chain]["maximum_sats"])
        self.save(chain, "funding", funding)

    def stage_sign_export(self, chain):
        reply = self.request(chain, "sign_export")
        require(reply.get("process") == self.state(chain)["restart_process"] and reply.get("gui_sign_clicked") is True
                and reply.get("ready_visible") is True and reply.get("self_transfer") is True
                and reply.get("native_save_dialog") is True and reply.get("broadcast_clicked") is False, "SIGN")
        self.save(chain, "derivation", {"receive_index": reply["receive_index"], "change_index": reply["change_index"]})
        raw = private_read(self.directory(chain) / "gui-signed.psbt")
        self.save(chain, "export_sha256", hashlib.sha256(raw).hexdigest())

    def stage_verify(self, chain):
        identity = self.state(chain)["identity"]
        manifest = {**identity, **self.state(chain)["derivation"], "funding": self.state(chain)["funding"],
                    "maximum_fee_sats": self.config["maximum_fee_sats"], "broadcast": False}
        raw = private_read(self.directory(chain) / "gui-signed.psbt")
        require(hashlib.sha256(raw).hexdigest() == self.state(chain)["export_sha256"], "EXPORT_CHANGED")
        proof = verify_export(raw, manifest)
        self.save(chain, "signature_proof", proof)

    def stage_retain(self, chain):
        reply = self.request(chain, "retain")
        require(reply.get("all_owned_processes_exited") is True and reply.get("profiles_retained") is True
                and reply.get("funded_roots_retained") is True and reply.get("personal_keys_untouched") is True
                and reply.get("credential_namespace") == self.scope, "RETAIN")
        self.save(chain, "acceptance", {"kind": "offline_selftest" if self.offline else "real_gui_prebroadcast",
            "chain": chain, "broadcast": False, "identity_sha256": digest(self.state(chain)["identity"]),
            "signature_proof": self.state(chain)["signature_proof"]})
