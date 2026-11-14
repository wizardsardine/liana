"""Retire one provenance-bound, completed disposable cloud fixture at capacity.

Never deletes local files, recovery material or coins. Uses the funding registry's
host lock so donor selection and cloud retirement cannot race on this host.
"""
from contextlib import contextmanager
from copy import deepcopy
import fcntl
import os
import re
import stat
import json
from pathlib import Path
import time
import xml.etree.ElementTree as ET
from urllib.error import HTTPError, URLError
from urllib.request import Request, build_opener, ProxyHandler

from . import qa_funding, sponsor
from .clients import EXPLORERS, GENESIS, REST, Native, NoRedirect, http
from .state import Blocked, Failed, digest, private_read


def require(value, code):
    if not value:
        raise Failed("QA_CLEANUP_" + code, "Disposable QA retirement validation failed: " + code)


def options(value):
    require(isinstance(value, dict) and set(value) == {"version", "preserve_runs"}
            and type(value["version"]) is int and value["version"] == 1, "CONFIG")
    require(isinstance(value["preserve_runs"], list) and len(value["preserve_runs"]) <= 64
            and all(isinstance(p, str) and Path(p).is_absolute() for p in value["preserve_runs"]), "CONFIG")
    return deepcopy(value)


def api(runner, path, *, delete=False, body=None, missing=False):
    """Owner-authenticated, no redirects, bounded response, exact API paths only."""
    require(path.startswith(("/connect/cubes", "/keys")), "API_PATH")
    headers = {"Authorization": "Bearer " + runner.account["tokens"]["access_token"]}
    if body is not None:
        headers["Content-Type"] = "application/json"
    request = Request(REST + "/api/v1" + path, headers=headers,
                      data=sponsor.encoded(body) if body is not None else None,
                      method="DELETE" if delete else "GET")
    try:
        with build_opener(ProxyHandler({}), NoRedirect()).open(request, timeout=20) as response:
            raw = response.read(4_000_001)
        require(len(raw) <= 4_000_000, "RESPONSE_SIZE")
        value = json.loads(raw)
        require(value.get("success") is True, "API_RESPONSE")
        return value.get("data")
    except HTTPError as error:
        if missing and not delete and error.code == 404:
            return None  # Only exact IDs from a durable retirement manifest.
        raise Blocked("QA_CLEANUP_HTTP", "The QA retirement API did not confirm the operation.") from None
    except (URLError, OSError, TimeoutError):
        raise Blocked("QA_CLEANUP_UNCERTAIN", "Reconcile the saved QA retirement before any new cleanup.") from None
    except (ValueError, UnicodeError):
        raise Failed("QA_CLEANUP_RESPONSE", "QA retirement returned an invalid response.") from None


def run_identity(directory):
    directory = Path(directory)
    require(directory.is_absolute() and not directory.is_symlink(), "DIRECTORY")
    state, cp, binding = sponsor.read_run(directory)
    a = cp["artifacts"]
    require(state.get("version") == 1 and state.get("registration_started") is True
            and state.get("cube_uuid") == state.get("run_id") == a.get("cube_uuid")
            and state.get("cube_id") == a.get("cube_id")
            and str(state.get("target_key_id")) == a.get("target_key_id")
            and state.get("target_fingerprint") == a.get("target_fingerprint"), "PROVENANCE")
    require(type(state["cube_id"]) is int and state["cube_id"] > 0
            and str(state["target_key_id"]).isdigit() and int(state["target_key_id"]) > 0, "PROVENANCE")
    return state, cp, binding


LOCK_FILES = {"run.lock", "provision.lock", "profile/.device-secret.lock"}


def retained_tree(directory, *, pending_results=False):
    """Exact owned recovery/history tree; ignore only known synchronization locks.

    The live checkpoint legitimately gains confirmation/timestamps on re-verify;
    its complete immutable counterpart is retained in the source snapshot.
    """
    directory = Path(directory)
    require(directory.is_absolute() and not directory.is_symlink(), "DIRECTORY")
    result = {}
    paths = []
    for root, directories, files in os.walk(directory, followlinks=False):
        root = Path(root)
        if root == directory / "qa-build":
            # Reproducible compiler/source products, not recovery/history.
            caches = {"source", "derived", "Runner.xcarchive", "export", "validated"}
            directories[:] = [name for name in directories if name not in caches]
        paths.extend(root / name for name in directories + files)
    for path in sorted(paths):
        relative = path.relative_to(directory).as_posix()
        info = path.lstat()
        require(not stat.S_ISLNK(info.st_mode) and info.st_uid == os.getuid(), "HISTORY_FILE")
        if stat.S_ISDIR(info.st_mode):
            require(not info.st_mode & 0o077, "HISTORY_DIRECTORY")
            continue
        require(stat.S_ISREG(info.st_mode), "HISTORY_FILE")
        if (relative in LOCK_FILES or relative == "checkpoint.json"
                or (pending_results and relative == "results.xml")
                or (path.parent == directory / "profile" and path.name.startswith(".testnet4-secret-")
                    and path.name.endswith(".lock"))
                or relative == "profile/testnet4/settings.lock"):
            continue
        result[relative] = sponsor.sha(private_read(path))
    require(result, "HISTORY_EMPTY")
    return result


@contextmanager
def candidate_lease(record):
    directory = Path(record["directory"])
    require(directory.is_absolute() and not directory.is_symlink(), "DIRECTORY")
    try:
        fd = os.open(directory / "run.lock", os.O_RDWR | os.O_NOFOLLOW)
    except OSError:
        raise Blocked("QA_CLEANUP_RUN_LOCK", "The completed QA run's existing lease is unavailable.") from None
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o077, "RUN_LOCK")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise Blocked("QA_CLEANUP_RUN_BUSY", "The completed QA run is still owned by another process.") from None
        yield
    finally:
        os.close(fd)


def verify_local(record):
    from .provisioning import STAGES
    state, cp, binding = run_identity(record["snapshot"])
    require(set(cp.get("stages", {})) == set(STAGES)
            and all(v.get("status") == "passed" for v in cp["stages"].values()), "COMPLETION")
    require(binding == record["source_binding"] and cp["artifacts"]["account_identity"] == record["account"]
            and state["owner_user_id"] == record["owner"]
            and state["run_id"] == record["run_id"]
            and state["cube_id"] == record["cube_id"]
            and state["vault_id"] == record["vault_id"]
            and int(state["target_key_id"]) == record["key_id"]
            and state["target_fingerprint"] == record["fingerprint"], "PROVENANCE")
    directory = Path(record["directory"])
    require(retained_tree(directory, pending_results=record.get("sealed") is not True) == record["retained_files"], "RECOVERY_CHANGED")
    require(Path(state["profile_dir"]).resolve() == (directory / "profile").resolve(), "PROFILE")
    return state, cp


def backup_check(runner, record):
    from .provisioning import Provision
    state, _ = verify_local(record)
    reply = Provision(runner.provision.binary, Path(state["profile_dir"]),
                      Path(record["directory"]), state["run_id"]).call("backup_check")
    require(reply.get("backup_verified") is True and reply.get("run_id") == record["run_id"], "BACKUP")


def pending(runner, record):
    state, cp = verify_local(record)
    a = cp["artifacts"]
    fixture = {"profile_dir": str(runner.profile), "psbt_file": str(Path(record["directory"]) / "local-signed.psbt"),
               "descriptor_file": str(Path(record["directory"]) / "descriptor.txt"),
               "expected_txid": state["spend"]["txid"], "cube_id": state["cube_id"],
               "vault_id": str(state["vault_id"]), "target_key_id": str(state["target_key_id"]),
               "target_fingerprint": state["target_fingerprint"], "request_id": a["request_id"]}
    reply = Native(runner.signing_binary, fixture).call("cleanup_audit")
    require(reply.get("owner_user_id") == record["owner"] == runner.account_user_id
            and isinstance(reply.get("sessions"), list) and isinstance(reply.get("decrypt_cubes"), list), "ACCOUNT")
    require(all(isinstance(s, dict) and isinstance(s.get("vault_id"), str)
                and isinstance(s.get("key_ids"), list) for s in reply["sessions"]), "PENDING")
    require(not any(s["vault_id"] == str(record["vault_id"]) or str(record["key_id"]) in s["key_ids"]
                    for s in reply["sessions"])
            and str(record["cube_id"]) not in reply["decrypt_cubes"]
            and record["run_id"] not in reply["decrypt_cubes"], "ACTIVE_REQUEST")


def accepted_child(record, data, txid, previous_raw, outspend, depth, parent=None):
    """Recognize only an already accepted bounded automatic QA sponsorship."""
    if data is None:
        raise Blocked("QA_CLEANUP_UNKNOWN_SPEND", "The completed change was spent outside a recorded automatic QA sponsorship.")
    claim = data["claims"].get(txid + ":1")
    if not isinstance(claim, dict) or claim.get("phase") != "accepted":
        raise Blocked("QA_CLEANUP_UNKNOWN_SPEND", "The completed change has no accepted automatic QA sponsorship proof.")
    require(claim.get("account") == record["account"] and isinstance(claim.get("entry"), dict), "SPONSOR_ACCOUNT")
    managed = [item for item in data["accounts"].get(record["account"], []) if item.get("entry") == claim["entry"]]
    require(len(managed) == 1, "SPONSOR_SOURCE")
    _, prior, value, binding = qa_funding.source_metadata(claim["entry"], managed[0], record["account"], record["owner"])
    require(binding == record["source_binding"], "SPONSOR_SOURCE")
    require(isinstance(claim.get("authorization_roots"), list) and claim["authorization_roots"]
            and set(claim["authorization_roots"]) <= set(managed[0].get("authorization_roots", [])), "SPONSOR_AUTHORIZATION")
    output = Path(claim["output"])
    require(output.is_absolute() and not output.is_symlink(), "SPONSOR_OUTPUT")
    raw_intent = private_read(output / "sponsor-intent.json")
    raw_receipt = private_read(output / "sponsor-receipt.json")
    require(raw_intent == claim["intent_json"].encode() and sponsor.sha(raw_intent) == claim["intent_sha256"]
            and sponsor.sha(raw_receipt) == claim["receipt_sha256"], "SPONSOR_CHANGED")
    intent, receipt = json.loads(raw_intent), json.loads(raw_receipt)
    require(type(intent.get("version")) is int and intent["version"] == depth and depth in (1, 2)
            and intent.get("authorization_ref") == "coincube#1137:testnet4-sponsor"
            and intent.get("network") == "testnet4" and intent.get("source") == binding
            and intent.get("previous_txid") == txid and intent.get("previous_vout") == 1
            and intent.get("previous_raw_sha256") == sponsor.sha(previous_raw.encode())
            and intent.get("previous_value_sats") == value
            and intent.get("source_script") == sponsor.p2wsh_script(verify_local(record)[0]["spend"]["change_address"])
            and type(intent.get("amount_sats")) is int and 100000 <= intent["amount_sats"] == claim["amount_sats"] <= 500000
            and type(intent.get("fee_sats")) is int and 100 <= intent["fee_sats"] == claim["fee_sats"] <= 1000
            and type(intent.get("change_sats")) is int and intent["change_sats"] >= 1000
            and receipt.get("native_binary_sha256") == claim.get("native_sha256"), "SPONSOR_BINDING")
    if depth == 1:
        require(prior is None and parent is None and "prior_sponsor" not in intent, "SPONSOR_DEPTH")
    else:
        require(prior is not None and parent is not None and isinstance(intent.get("prior_sponsor"), dict), "SPONSOR_DEPTH")
        require(intent["prior_sponsor"]["intent_json"].encode() == parent[0]
                and intent["prior_sponsor"]["receipt_json"].encode() == parent[1]
                and private_read(prior / "sponsor-intent.json") == parent[0]
                and private_read(prior / "sponsor-receipt.json") == parent[1], "SPONSOR_PARENT")
    sponsor.validate_receipt(receipt, intent, claim["intent_sha256"])
    sponsor.verify_snapshot(intent)
    snapshot = Path(claim["snapshot"])
    manifest_raw = private_read(snapshot / "manifest.json")
    manifest = json.loads(manifest_raw)
    target_state, target_cp, target_binding = sponsor.read_run(snapshot)
    require(manifest.get("version") == 1 and manifest.get("directory") == claim["live"]
            and manifest.get("identity") == claim["identity"]
            and sponsor.sha(manifest_raw) == claim["manifest_sha256"]
            and manifest.get("hashes") == {name: sponsor.sha(private_read(snapshot / name)) for name in qa_funding.FILES}
            and intent.get("target") == target_binding
            and target_state["owner_user_id"] == record["owner"] and target_state["cube_id"] != record["cube_id"]
            and target_cp["artifacts"]["account_identity"] == record["account"]
            and intent.get("target_address") == claim["identity"]["artifacts"]["receive_address"]
            and intent.get("target_script") == sponsor.p2wsh_script(intent["target_address"])
            and claim["identity"]["artifacts"]["cube_id"] == target_state["cube_id"], "SPONSOR_TARGET")
    require(outspend["txid"] == receipt["txid"] and outspend["vin"] == 0, "SPONSOR_SPENDER")
    return receipt, (raw_intent, raw_receipt)


def residual(record, data=None):
    """Trace only accepted existing QA v1/v2 change; never infer zero from spent."""
    state, cp = verify_local(record)
    spend = state["spend"]
    raw = private_read(Path(record["directory"]) / "signed.hex").decode().strip()
    txid, value = spend["txid"], spend["change_sats"]
    require(sponsor.transaction(raw)["txid"] == txid == cp["artifacts"]["broadcast_txid"], "TRANSACTION")
    parent = None
    for depth in range(3):
        decoded = sponsor.transaction(raw)
        require(len(decoded["outputs"]) == 2 and decoded["outputs"][1] == {
            "value_sats": value, "script": sponsor.p2wsh_script(spend["change_address"])}, "CHANGE_OUTPUT")
        observed = []
        for endpoint in EXPLORERS:
            require(http(endpoint + "/block-height/0").decode().strip() == GENESIS, "CHAIN")
            require(http(endpoint + "/tx/" + txid + "/hex").decode().strip() == raw, "TRANSACTION")
            outspend = json.loads(http(endpoint + "/tx/" + txid + "/outspend/1"))
            require(type(outspend.get("spent")) is bool, "BALANCE_INDEX")
            if outspend["spent"]:
                require(isinstance(outspend.get("txid"), str) and re.fullmatch(r"[0-9a-f]{64}", outspend["txid"])
                        and type(outspend.get("vin")) is int and outspend["vin"] >= 0, "SPENDER")
                observed.append({"spent": True, "txid": outspend["txid"], "vin": outspend["vin"]})
            else:
                observed.append({"spent": False})
        require(observed[0] == observed[1], "SPENDER_DISAGREEMENT")
        if not observed[0]["spent"]:
            points = []
            for endpoint in EXPLORERS:
                entries = []
                for address in (cp["artifacts"]["receive_address"], spend["change_address"]):
                    rows = json.loads(http(endpoint + "/address/" + address + "/utxo"))
                    require(isinstance(rows, list), "BALANCE")
                    entries += [(p["txid"], p["vout"], p["value"]) for p in rows]
                require(entries == [(txid, 1, value)], "BALANCE_INDEX")
                points.append(entries)
            require(points[0] == points[1], "EXPLORER_DISAGREEMENT")
            return value
        if depth == 2:
            raise Blocked("QA_CLEANUP_UNKNOWN_SPEND", "Further sponsorship change is outside the bounded QA cleanup protocol.")
        receipt, parent = accepted_child(record, data, txid, raw, observed[0], depth + 1, parent)
        raw, txid, value = receipt["raw_tx"], receipt["txid"], receipt["change_sats"]
    raise Failed("QA_CLEANUP_SPONSOR_DEPTH", "Unsupported QA sponsorship depth.")


def protected_ids(runner, data):
    """Cloud protection uses exact IDs; labels and run UUIDs never grant deletion."""
    funding = qa_funding.options(runner.config.get("qa_funding", {"version": 1, "sources": []}))
    protected = set()
    paths = [e["source_run"] for e in funding["sources"]] + options(runner.config["qa_cleanup"])["preserve_runs"]
    for path in paths:
        state, _, _ = sponsor.read_run(Path(path))
        protected.add(state["cube_id"])
    # An unfinished or externally superseded funding claim keeps both ends.
    for claim in data["claims"].values():
        if claim.get("phase") != "accepted":
            state, _, _ = sponsor.read_run(Path(claim["entry"]["source_run"]))
            protected.add(state["cube_id"])
            protected.add(claim["identity"]["artifacts"]["cube_id"])
    for outcome in data["external"].values():
        protected.add(outcome["identity"]["artifacts"]["cube_id"])
    # Managed donors with usable unspent change retain their cloud fixture.
    for item in data["accounts"].get(runner.artifacts["account_identity"], []):
        if retired(data, item["entry"], item, account=runner.artifacts["account_identity"], owner=runner.account_user_id):
            continue
        source, prior, value, _ = qa_funding.source_metadata(item["entry"], item,
                                    runner.artifacts["account_identity"], runner.account_user_id)
        if value < qa_funding.minimum_amount(runner.config) + funding["fee_sats"] + 1000:
            continue
        state, _, _ = sponsor.read_run(source)
        txid = state["spend"]["txid"]
        if prior:
            txid = json.loads(private_read(prior / "sponsor-receipt.json"))["txid"]
        try:
            sponsor.observe(txid, 1)
        except Failed as error:
            if error.code != "SOURCE_SPENT":
                raise
        else:
            protected.add(state["cube_id"])
    return protected


def retired(data, entry, enrolled=None, *, account=None, owner=None):
    path = str(Path(entry["source_run"]).resolve())
    tombstones = data.get("retired_cloud_sources", {})
    require(isinstance(tombstones, dict), "TOMBSTONE")
    if path not in tombstones:
        return False
    tombstone = tombstones[path]
    require(isinstance(tombstone, dict) and set(tombstone) == {
        "retirement", "run_id", "cube_id", "account", "source_binding", "residual_sats", "local_recovery_retained"}, "TOMBSTONE")
    records = data.get("retirements", {})
    require(isinstance(records, dict) and isinstance(tombstone["retirement"], str), "TOMBSTONE")
    record = records.get(tombstone["retirement"])
    require(isinstance(record, dict) and type(record.get("version")) is int and record["version"] == 1
            and record.get("phase") == "complete" and record.get("sealed") is True and record.get("snapshot") == path
            and tombstone["local_recovery_retained"] is True
            and type(tombstone["cube_id"]) is int and type(tombstone["residual_sats"]) is int
            and tombstone["residual_sats"] >= 0
            and all(tombstone[k] == record.get(k) for k in ("run_id", "cube_id", "account", "source_binding", "residual_sats")), "TOMBSTONE")
    require((account is None or record["account"] == account)
            and (owner is None or record["owner"] == owner), "TOMBSTONE")
    verify_local(record)
    if enrolled is not None:
        require(enrolled.get("source_binding") == record["source_binding"], "TOMBSTONE")
        _, _, _, binding = qa_funding.source_metadata(entry, enrolled, record["account"], record["owner"])
        require(binding == record["source_binding"], "TOMBSTONE")
    return True


def inventory_key_ids(rows):
    require(isinstance(rows, list) and all(isinstance(row, dict) and type(row.get("id")) is int
            and row["id"] > 0 for row in rows), "OTHER_KEY_SCHEMA")
    ids = [row["id"] for row in rows]
    require(len(ids) == len(set(ids)), "OTHER_KEY_SCHEMA")
    return set(ids)


def vault_key_ids(vault, cube_id):
    if vault is None:
        return set()
    require(isinstance(vault, dict) and type(vault.get("id")) is int and vault["id"] > 0
            and type(vault.get("cubeId")) is int and vault["cubeId"] == cube_id
            and isinstance(vault.get("members"), list), "OTHER_VAULT_SCHEMA")
    keys = set()
    for member in vault["members"]:
        require(isinstance(member, dict) and type(member.get("id")) is int and member["id"] > 0
                and isinstance(member.get("role"), str) and member["role"], "OTHER_VAULT_SCHEMA")
        key_id = member.get("keyId")
        require(key_id is None or (type(key_id) is int and key_id > 0), "OTHER_VAULT_SCHEMA")
        require(key_id is not None or (type(member.get("contactId")) is int and member["contactId"] > 0), "OTHER_VAULT_SCHEMA")
        nested = member.get("key")
        if nested is not None:
            require(isinstance(nested, dict) and type(nested.get("id")) is int
                    and nested["id"] > 0 and nested["id"] == key_id, "OTHER_VAULT_SCHEMA")
        if key_id is not None:
            keys.add(key_id)
    return keys


def protect_other_cubes(runner, record):
    # The owned list excludes membership in another owner's Cube. Read both
    # without a network filter; hasVault is only a nullable client assertion.
    inventories = [api(runner, "/connect/cubes"), api(runner, "/connect/cubes/shared-with-me")]
    require(all(isinstance(rows, list) for rows in inventories), "CUBE_DISCOVERY")
    by_id, owned, shared = {}, set(), set()
    for rows, ids in zip(inventories, (owned, shared)):
        for row in rows:
            require(isinstance(row, dict) and type(row.get("id")) is int and row["id"] > 0
                    and row["id"] not in ids, "CUBE_DISCOVERY")
            ids.add(row["id"])
            by_id[row["id"]] = row
    require(not owned.intersection(shared), "CUBE_DISCOVERY")
    for cube_id, listed in sorted(by_id.items()):
        if cube_id == record["cube_id"]:
            require(cube_id in owned, "CUBE_DISCOVERY")
            continue
        base = "/connect/cubes/" + str(cube_id)
        # This endpoint includes both attached keys and Vault member keys.
        # Never substitute an omitted list field or hasVault=false for it.
        authoritative = inventory_key_ids(api(runner, base + "/keys"))
        require(record["key_id"] not in authoritative, "SHARED_KEY")
        other = api(runner, base)
        require(isinstance(other, dict) and type(other.get("id")) is int and other["id"] == cube_id
                and type(other.get("userId")) is int and other["userId"] > 0
                and (other["userId"] == runner.account_user_id) == (cube_id in owned)
                and isinstance(other.get("network"), str) and other["network"]
                and all(isinstance(other.get(k), list) and all(isinstance(row, dict) for row in other[k])
                        for k in ("members", "pendingInvites"))
                and "vault" in other, "OTHER_CUBE_SCHEMA")
        require("userId" not in listed or (type(listed["userId"]) is int and listed["userId"] == other["userId"]), "OTHER_CUBE_SCHEMA")
        attached = inventory_key_ids(other["keys"]) if "keys" in other else set()
        members = vault_key_ids(other["vault"], cube_id)
        require(record["key_id"] not in attached | members, "SHARED_KEY")
        # A stale discovery projection is protective evidence, never permission
        # to ignore references or contradict a fresh authoritative inventory.
        listed_keys = inventory_key_ids(listed["keys"]) if "keys" in listed else None
        listed_members = vault_key_ids(listed["vault"], cube_id) if "vault" in listed else None
        require(record["key_id"] not in (listed_keys or set()) | (listed_members or set()), "SHARED_KEY")
        if listed_keys is not None:
            require(listed_keys == attached, "OTHER_CUBE_GRAPH")
        if "vault" in listed:
            require(listed["vault"] == other["vault"], "OTHER_CUBE_GRAPH")
        if cube_id in owned:
            # /vault is owner-only. A 404 can prove absence here, but would
            # mean nothing for a shared Cube; its complete GET detail and keys
            # above are the member-authorized Vault inventory surfaces.
            vault = api(runner, base + "/vault", missing=True)
            require(record["key_id"] not in vault_key_ids(vault, cube_id), "SHARED_KEY")
            require(vault == other["vault"], "OTHER_VAULT_GRAPH")
        require(authoritative == attached | members, "OTHER_KEY_GRAPH")


def graph(runner, record):
    base = "/connect/cubes/" + str(record["cube_id"])
    cube = api(runner, base, missing=True)
    vault = api(runner, base + "/vault", missing=True)
    cube_keys = api(runner, base + "/keys", missing=True)
    require((cube is None and cube_keys is None) or isinstance(cube_keys, list), "KEY_SCHEMA")
    key_result = api(runner, "/keys")
    require(isinstance(key_result, dict) and isinstance(key_result.get("keys"), list), "KEY_SCHEMA")
    keys = key_result["keys"]
    matches = [k for k in keys if k.get("id") == record["key_id"]]
    require(len(matches) <= 1, "KEY_GRAPH")
    key = matches[0] if matches else None
    if key is not None:
        require(key.get("cubeId") == record["cube_id"] and key.get("keychainId") == record["keychain_id"]
                and key.get("network") == "testnet4" and key.get("fingerprint") == record["fingerprint"]
                and key.get("status") == "active" and key.get("vaultId") in (None, record["vault_id"]), "KEY_GRAPH")
    protect_other_cubes(runner, record)
    if cube is not None:
        require(isinstance(cube, dict) and all(isinstance(cube.get(k), list) for k in ("members", "pendingInvites"))
                and (isinstance(cube.get("keys"), list) or ("keys" not in cube and cube_keys == []))
                and "vault" in cube, "CUBE_SCHEMA")
        require(cube.get("id") == record["cube_id"] and cube.get("userId") == record["owner"]
                and cube.get("uuid") == record["run_id"] and cube.get("name") == record["cube_name"]
                and cube.get("network") == "testnet4" and cube.get("status") == "active"
                and cube.get("encryptionPubkey") == record["encryption_pubkey"]
                and not cube.get("members") and not cube.get("pendingInvites")
                and cube.get("lightningAddress") is None and cube.get("sparkStableBalance") is None
                and cube.get("hasRecoveryKit") is False
                and all(k.get("id") == record["key_id"] and k.get("primaryOwnerId") == record["owner"]
                        for k in cube.get("keys", [])), "CUBE_GRAPH")
        require(len(cube_keys) == len(cube.get("keys", [])) == (1 if key is not None else 0)
                and all(k.get("id") == record["key_id"] for k in cube_keys), "KEY_GRAPH")
        embedded = cube.get("vault")
        require((embedded is None) == (vault is None)
                and (embedded is None or embedded.get("id") == record["vault_id"]), "VAULT_GRAPH")
    if vault is not None:
        require(isinstance(vault, dict) and isinstance(vault.get("members"), list), "VAULT_SCHEMA")
        require(cube is not None and vault.get("id") == record["vault_id"]
                and vault.get("cubeId") == record["cube_id"] and vault.get("fingerprint") == record["descriptor_id"]
                and vault.get("status") == "active" and len(vault.get("members", [])) == 1
                and vault["members"][0].get("keyId") == record["key_id"]
                and vault["members"][0].get("role") == "keyholder", "VAULT_GRAPH")
    return cube, vault, key


def register_completed(runner):
    """Only a newly verified strict run can auto-enroll; never discover by label."""
    from .provisioning import STAGES
    require(runner.strict is not None and not runner.stop, "MODE")
    directory = runner.journal.directory.resolve()
    qa_funding.register_completed(runner, cleanup_only=True)
    state, cp, _ = run_identity(directory)
    require(state["run_id"] == runner.run_id and cp["binding"] == digest(runner.config)
            and set(cp["stages"]) == set(STAGES)
            and all(s.get("status") == "passed" for s in cp["stages"].values()), "COMPLETION")
    a = cp["artifacts"]
    require(a.get("signature_count_min", 0) >= 2 and a.get("strict_pin_input", {}).get("completed") is True
            and a.get("strict_home_refresh", {}).get("home_refresh_proven") is True, "COMPLETION")
    runner.strict.require_completion(a["request_id"], a["session_id"])
    snapshot = directory.with_name(directory.name + "-completed-source")
    saved, saved_cp, binding = run_identity(snapshot)
    require(saved == state and saved_cp["binding"] == cp["binding"], "COMPLETION")
    with qa_funding.pool() as data:
        records = data.setdefault("retirements", {})
        require(isinstance(records, dict) and len(records) < 1024, "REGISTRY")
        key = digest({"account": a["account_identity"], "run_id": runner.run_id, "directory": str(directory)})
        record = {"version": 1, "account": a["account_identity"], "owner": state["owner_user_id"],
                  "run_id": state["run_id"], "directory": str(directory), "snapshot": str(snapshot),
                  "source_binding": binding, "cube_id": state["cube_id"], "cube_name": state["cube_name"],
                  "vault_id": state["vault_id"], "descriptor_id": state["descriptor_id"],
                  "key_id": int(state["target_key_id"]), "fingerprint": state["target_fingerprint"],
                  "keychain_id": None, "phase": "enrolled", "sealed": False,
                  "retained_files": retained_tree(directory, pending_results=True)}
        keys = api(runner, "/keys")["keys"]
        match = [k for k in keys if k.get("id") == record["key_id"]]
        require(len(match) == 1 and type(match[0].get("keychainId")) is int, "KEY_GRAPH")
        record["keychain_id"] = match[0]["keychainId"]
        cube = api(runner, "/connect/cubes/" + str(record["cube_id"]))
        public_key = cube.get("encryptionPubkey")
        require(isinstance(public_key, str) and len(public_key) == 66 and public_key[:2] in ("02", "03")
                and all(c in "0123456789abcdef" for c in public_key), "CUBE_GRAPH")
        record["encryption_pubkey"] = public_key
        graph(runner, record)
        backup_check(runner, record)
        if key in records:
            previous = deepcopy(records[key])
            previous["phase"] = "enrolled"
            previous["sealed"] = False
            previous["retained_files"].pop("results.xml", None)
            previous.pop("residual_sats", None)
            previous.pop("updated_at", None)
            previous.pop("consumer", None)
            require(previous == record, "ENROLLMENT_CHANGED")
            return
        records[key] = record
        qa_funding.save_pool(data)


class StrictMobileAdmission:
    """The existing mobile authorization; no alternate phone identity."""
    def __init__(self, runner):
        self.runner = runner

    def select(self):
        self.observe()

    def observe(self):
        require(self.runner.strict is not None and not self.runner.strict.requested, "MODE")
        self.runner.strict.observe()
        require(not self.runner.strict.requested, "MODE")


def cleanup_admission(runner, admission):
    if admission is None:
        return StrictMobileAdmission(runner)
    # A closed internal union, never a JSON flag or arbitrary observe callback.
    from .native_recovery import NativeCleanupConsumerV1
    require(type(admission) in (StrictMobileAdmission, NativeCleanupConsumerV1)
            and admission.runner is runner, "ADMISSION")
    return admission


def cleanup_local(admission, operation, runner, record):
    # The legacy mobile path is unchanged. Native contexts additionally pin
    # real backup/audit binaries immediately around their private-key work.
    from .native_recovery import NativeCleanupConsumerV1
    if type(admission) is not NativeCleanupConsumerV1:
        return operation(runner, record)
    admission.observe()
    try:
        return operation(runner, record)
    finally:
        admission.observe()


def ensure_capacity(runner, *, admission=None):
    admission = cleanup_admission(runner, admission)
    admission.observe()
    require(not any(k in runner.artifacts for k in ("request_id", "session_id", "approval_started", "pin_dispatch_started")), "ACTIVE_RUN")
    account = runner.artifacts["account_identity"]
    consumer = digest({"run_id": runner.run_id, "binding": runner.journal.data["binding"]})
    with qa_funding.pool() as data:
        records = data.setdefault("retirements", {})
        require(isinstance(records, dict), "REGISTRY")
        # Resume our one durable selection, including a lost successful DELETE.
        own = [(k, r) for k, r in records.items() if r.get("consumer") == consumer]
        require(len(own) <= 1, "REGISTRY")
        require(not any(r.get("account") == account and r.get("phase") not in ("enrolled", "complete")
                        and r.get("consumer") != consumer for r in records.values()), "RETIREMENT_BUSY")
        if own:
            key, record = own[0]
        else:
            admission.select()  # Native selection requires a fresh FULL quota.
            protections = protected_ids(runner, data)
            candidates = [(k, r) for k, r in records.items()
                          if r.get("phase") == "enrolled" and type(r.get("sealed")) is bool and r.get("account") == account
                          and r.get("owner") == runner.account_user_id and r.get("cube_id") not in protections
                          and r.get("run_id") != runner.run_id]
            if not candidates:
                raise Blocked("QA_CLEANUP_NO_CANDIDATE", "No safely enrolled disposable QA fixture can free a signer slot.")
            key, record = sorted(candidates, key=lambda item: item[0])[0]
        require(record.get("account") == account and record.get("owner") == runner.account_user_id, "ACCOUNT")
        if record["phase"] == "complete":
            retired(data, {"source_run": record["snapshot"]})
            return  # One fixture maximum per consumer, even if another remains full.
        with candidate_lease(record):
            if record.get("sealed") is not True:
                cleanup_local(admission, backup_check, runner, record)
                seal_record(record)
                qa_funding.save_pool(data)
            def guard():
                admission.observe()
                require(record["cube_id"] not in protected_ids(runner, data), "PROTECTED")
                verify_local(record)
                cleanup_local(admission, backup_check, runner, record)
                cleanup_local(admission, pending, runner, record)
                amount = residual(record, data)
                funding = qa_funding.options(runner.config.get("qa_funding", {"version": 1, "sources": []}))
                require(amount < qa_funding.minimum_amount(runner.config) + funding["fee_sats"] + 1000, "FUNDING_SOURCE")
                if "residual_sats" in record:
                    require(amount == record["residual_sats"], "BALANCE_CHANGED")
                else:
                    record["residual_sats"] = amount
                return graph(runner, record)
            cube, vault, phone_key = guard()
            if record["phase"] == "enrolled":
                require(cube is not None and vault is not None and phone_key is not None, "MISSING_FIXTURE")
                record.update(consumer=consumer, phase="vault_intent", updated_at=int(time.time()))
                qa_funding.save_pool(data)
            base = "/connect/cubes/" + str(record["cube_id"])
            steps = (("vault_intent", "key_intent", base + "/vault", None, 1),
                     ("key_intent", "cube_intent", "/keys/delete", {"keyId": record["key_id"], "keychainId": record["keychain_id"]}, 2),
                     ("cube_intent", "complete", base, None, 0))
            require(record["phase"] in {s[0] for s in steps}, "PHASE")
            for phase, after, path, body, index in steps:
                if record["phase"] != phase:
                    continue
                resources = guard()
                if phase != "vault_intent":
                    require(resources[1] is None, "VAULT_REAPPEARED")
                if phase == "cube_intent":
                    require(resources[2] is None, "KEY_REAPPEARED")
                if resources[index] is not None:
                    # Intent already fsynced. An uncertain transport stays here.
                    api(runner, path, delete=True, body=body)
                require(graph(runner, record)[index] is None, "DELETE_UNCONFIRMED")
                record.update(phase=after, updated_at=int(time.time()))
                if after == "complete":
                    data.setdefault("retired_cloud_sources", {})[record["snapshot"]] = {
                        "retirement": key, "run_id": record["run_id"], "cube_id": record["cube_id"],
                        "account": record["account"], "source_binding": record["source_binding"],
                        "residual_sats": record["residual_sats"], "local_recovery_retained": True}
                qa_funding.save_pool(data)

        runner.journal.artifact("qa_cleanup", {"retirement": key, "cube_id": record["cube_id"],
            "vault_id": record["vault_id"], "key_id": record["key_id"], "residual_sats": record["residual_sats"],
            "local_recovery_retained": True, "phase": record["phase"]})


def reconcile_capacity(runner, *, admission=None):
    """Finish our prior exact retirement even after its key freed capacity."""
    consumer = digest({"run_id": runner.run_id, "binding": runner.journal.data["binding"]})
    with qa_funding.pool() as data:
        records = data.get("retirements", {})
        require(isinstance(records, dict), "REGISTRY")
        pending = [r for r in records.values() if r.get("consumer") == consumer
                   and r.get("phase") != "complete"]
        require(len(pending) <= 1, "REGISTRY")
    if pending:
        ensure_capacity(runner, admission=admission)
        return True
    return False


def seal_record(record):
    from .provisioning import STAGES
    verify_local(record)
    if record.get("sealed") is True:
        return
    try:
        results = ET.fromstring(private_read(Path(record["directory"]) / "results.xml"))
    except ET.ParseError:
        raise Failed("QA_CLEANUP_RESULTS", "The completed QA JUnit evidence is invalid.") from None
    cases = results.findall(".//testcase")
    stages = []
    require(len(cases) == len(STAGES), "RESULTS")
    for case in cases:
        properties = {p.get("name"): p.get("value") for p in case.findall("./properties/property")}
        require(properties.get("outcome") == "passed" and properties.get("stage") in STAGES
                and not any(case.find(kind) is not None for kind in ("error", "failure", "skipped")), "RESULTS")
        stages.append(properties["stage"])
    require(set(stages) == set(STAGES), "RESULTS")
    record["retained_files"] = retained_tree(Path(record["directory"]))
    record["sealed"] = True


def seal_completed(runner):
    """Called after final JUnit serialization; no phone interaction is performed."""
    key = digest({"account": runner.artifacts["account_identity"], "run_id": runner.run_id,
                  "directory": str(runner.journal.directory.resolve())})
    with qa_funding.pool() as data:
        record = data.get("retirements", {}).get(key)
        require(isinstance(record, dict) and record.get("phase") == "enrolled"
                and record.get("run_id") == runner.run_id and record.get("owner") == runner.account_user_id
                and record.get("account") == runner.artifacts["account_identity"]
                and record.get("consumer") is None, "COMPLETION")
        with candidate_lease(record):
            verify_local(record)
            seal_record(record)
            qa_funding.save_pool(data)
