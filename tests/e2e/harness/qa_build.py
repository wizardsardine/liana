"""Build and install a source-pinned QA app before the strict process is bound."""
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tarfile

from .state import Blocked, Failed, atomic_write, digest, private_read

BUNDLE = "io.coincube.keychainApp"
TEAM = "8UVR249AD5"
DEFINE = "COINCUBE_QA_HOME_REFRESH=true"
# The mobile repository was renamed keychain-app -> coincube-mobile on
# 2026-11-15. GitHub redirects the old name, so checkouts whose origin still
# uses it are the same repository and stay accepted.
QA_SOURCE_REMOTES = (
    "git@github.com:coincubetech/coincube-mobile.git", "https://github.com/coincubetech/coincube-mobile.git",
    "git@github.com:coincubetech/keychain-app.git", "https://github.com/coincubetech/keychain-app.git",
)


def configure_fresh(config, *, compatibility=False):
    """The supported fresh launcher is strict unless compatibility is explicit."""
    config = json.loads(json.dumps(config))
    if compatibility:
        if "strict_acceptance" in config:
            raise Failed("STRICT_CONFIG", "Compatibility mode cannot override a configured strict run.")
        return config
    config.setdefault("strict_acceptance", {"version": 1})
    validate_build_config(config.get("ios", {}).get("qa_build"))
    return config


def validate_build_config(config):
    if (not isinstance(config, dict) or set(config) != {"checkout", "revision", "flutter"}
            or not isinstance(config.get("revision"), str)
            or not re.fullmatch(r"[0-9a-f]{40}", config["revision"])
            or any(not isinstance(config.get(k), str) or not Path(config[k]).is_absolute()
                   for k in ("checkout", "flutter"))):
        raise Failed("QA_BUILD_CONFIG", "Strict QA requires ios.qa_build with absolute checkout/flutter paths and a full source revision.")
    return config


def command(args, *, cwd=None, log=None, timeout=180):
    try:
        result = subprocess.run([str(a) for a in args], cwd=cwd, check=True, timeout=timeout,
                                stdout=log if log else subprocess.PIPE,
                                stderr=log if log else subprocess.DEVNULL)
        return result.stdout
    except (OSError, subprocess.SubprocessError):
        raise Blocked("QA_BUILD", "QA build, signing or device command failed; inspect the private qa-build/build.log and retry the same run.") from None


def plist_output(args):
    data = command(args)
    try:
        return plistlib.loads(data)
    except (ValueError, plistlib.InvalidFileException):
        raise Failed("QA_BUILD_PLIST", "The signed QA app has invalid provisioning or entitlement metadata.") from None


def validate_app(app, device, minimum_build):
    command(["codesign", "--verify", "--deep", "--strict", app])
    try:
        info = plistlib.loads((app / "Info.plist").read_bytes())
        build = int(info["CFBundleVersion"])
        env = dict(line.split("=", 1) for line in
                   (app / "Frameworks/App.framework/flutter_assets/.env").read_text().splitlines() if "=" in line)
    except (OSError, ValueError, KeyError):
        raise Failed("QA_BUILD_APP", "The built QA app is missing its version or production configuration.") from None
    entitlements = plist_output(["codesign", "-d", "--entitlements", ":-", app])
    profile = plist_output(["security", "cms", "-D", "-i", app / "embedded.mobileprovision"])
    expected = TEAM + "." + BUNDLE
    profile_entitlements = profile.get("Entitlements", {})
    if (info.get("CFBundleIdentifier") != BUNDLE or build < minimum_build
            or entitlements.get("application-identifier") != expected
            or entitlements.get("get-task-allow") is not False
            or entitlements.get("aps-environment") != "production"
            or entitlements.get("com.apple.developer.associated-domains") != ["webcredentials:coincube.io"]
            or profile.get("TeamIdentifier") != [TEAM]
            or profile_entitlements.get("application-identifier") != expected
            or profile_entitlements.get("get-task-allow") is not False
            or profile_entitlements.get("aps-environment") != "production"
            or profile.get("ProvisionsAllDevices", False)
            or device["hardwareProperties"]["udid"] not in profile.get("ProvisionedDevices", [])
            or profile.get("ExpirationDate", datetime.min).replace(tzinfo=timezone.utc) <= datetime.now(timezone.utc)):
        raise Failed("QA_BUILD_IDENTITY", "QA app identity, release-testing profile, device coverage, entitlements or build is invalid.")
    if any(env.get(k) != v for k, v in {"API_URL": "https://api.coincube.io/api/v1/",
                                        "GRPC_HOST": "grpc.coincube.io", "GRPC_PORT": "443", "GRPC_TLS": "true"}.items()):
        raise Failed("QA_BUILD_ENDPOINTS", "The QA app must use production REST and TLS gRPC endpoints.")
    framework = (app / "Frameworks/App.framework/App").read_bytes()
    if b"qa-home-refresh-trigger" not in framework:
        raise Failed("QA_BUILD_HOOK", "The compiled app lacks the QA Home-refresh trigger; build number or a define alone is insufficient.")
    # The validated signed bundle, not its version alone, binds the receipt.
    return {"build": build, "binary_sha256": hashlib.sha256((app / "Runner").read_bytes()).hexdigest(),
            "flutter_sha256": hashlib.sha256(framework).hexdigest()}


def verify_installed(device, build, query):
    apps = query("device", "info", "apps", "--device", device["identifier"], "--bundle-id", BUNDLE,
                 "--include-removable-apps", "--include-default-apps")["apps"]
    if len(apps) != 1 or str(apps[0].get("bundleVersion")) != str(build):
        raise Blocked("QA_BUILD_CHANGED", "The installed app does not match the signed QA receipt.")


def prepare(config, strict, device, minimum_build, query):
    """Called under the same USB lease used by the whole acceptance run."""
    build_config = validate_build_config(config)
    journal = strict.journal
    root = journal.directory / "qa-build"
    if root.is_symlink():
        raise Failed("QA_BUILD_DIRECTORY", "The QA build directory cannot be a symlink.")
    root.mkdir(mode=0o700, exist_ok=True)
    info = root.stat()
    if info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise Failed("QA_BUILD_DIRECTORY", "The QA build directory must be private and owned by this user.")
    receipt_path = root / "receipt.json"
    binding = digest({"config": build_config, "define": DEFINE,
                      "device": device["hardwareProperties"]["udid"]})
    receipt = json.loads(private_read(receipt_path)) if receipt_path.exists() else None
    if receipt is not None:
        if receipt.get("binding") != binding or journal.data["artifacts"].get("qa_build") != receipt:
            raise Failed("QA_BUILD_BINDING", "The QA build or USB device changed; use a fresh run directory.")
        app = root / "validated/Payload/Runner.app"
        observed = validate_app(app, device, minimum_build)
        if observed != receipt["app"]:
            raise Failed("QA_BUILD_CHANGED", "The saved QA application changed after installation.")
        verify_installed(device, receipt["app"]["build"], query)
        return
    if "strict_process" in journal.data["artifacts"]:
        raise Failed("QA_BUILD_RESUME", "An existing strict process cannot receive a new QA build; use a fresh run.")
    checkout = Path(build_config["checkout"])
    if command(["git", "remote", "get-url", "origin"], cwd=checkout).decode().strip() not in QA_SOURCE_REMOTES:
        raise Failed("QA_BUILD_REPOSITORY", "The QA source must be the configured COINCUBE Mobile repository.")
    source = root / "source"
    if source.is_symlink():
        raise Failed("QA_BUILD_SOURCE", "The QA source directory cannot be a symlink.")
    if source.exists():
        shutil.rmtree(source)
    # Always re-extract before compilation, including an interrupted build.
    # A receipt or stamp cannot authenticate a mutable source directory.
    archive = command(["git", "archive", build_config["revision"]], cwd=checkout)
    source.mkdir(mode=0o700)
    with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
        stream.extractall(source, filter="data")
    pinned_flutter = json.loads((source / ".fvmrc").read_text())["flutter"]
    flutter = Path(build_config["flutter"])
    version = json.loads(command([flutter, "--version", "--machine"]))
    if version.get("frameworkVersion") != pinned_flutter:
        raise Blocked("QA_FLUTTER_VERSION", "The configured Flutter executable must match Keychain's .fvmrc pin.")
    atomic_write(source / ".env", b"API_URL=https://api.coincube.io/api/v1/\nPASSKEY_RP_ID=coincube.io\nNETWORK_ENV=mainnet\nGRPC_HOST=grpc.coincube.io\nGRPC_PORT=443\nGRPC_TLS=true\n")
    atomic_write(root / "ExportOptions.plist", plistlib.dumps({"method": "release-testing", "signingStyle": "automatic", "teamID": TEAM}))
    archive_path = root / "Runner.xcarchive"
    with (root / "build.log").open("wb") as log:
        command([flutter, "pub", "get"], cwd=source, log=log, timeout=600)
        command([flutter, "build", "ios", "--release", "--no-codesign", "--no-pub", "--config-only",
                 "--dart-define=" + DEFINE], cwd=source, log=log)
        command(["pod", "install", "--project-directory=ios"], cwd=source, log=log, timeout=600)
        command(["xcodebuild", "-workspace", "ios/Runner.xcworkspace", "-scheme", "Runner", "-configuration", "Release",
                 "-destination", "generic/platform=iOS", "-archivePath", archive_path, "-derivedDataPath", root / "derived",
                 "-quiet", "CODE_SIGNING_ALLOWED=NO", "IPHONEOS_DEPLOYMENT_TARGET=16.0", "archive"],
                cwd=source, log=log, timeout=1800)
        command(["codesign", "--force", "--sign", "-", "--entitlements", source / "ios/Runner/Runner.entitlements",
                 "--generate-entitlement-der", archive_path / "Products/Applications/Runner.app"], log=log)
        command(["xcodebuild", "-exportArchive", "-archivePath", archive_path, "-exportPath", root / "export",
                 "-exportOptionsPlist", root / "ExportOptions.plist", "-allowProvisioningUpdates"], log=log, timeout=900)
    ipas = list((root / "export").glob("*.ipa"))
    if len(ipas) != 1:
        raise Failed("QA_BUILD_EXPORT", "The build did not produce exactly one signed QA IPA.")
    command(["ditto", "-x", "-k", ipas[0], root / "validated"])
    app = root / "validated/Payload/Runner.app"
    observed = validate_app(app, device, minimum_build)
    query("device", "install", "app", "--device", device["identifier"], str(app), timeout=180)
    verify_installed(device, observed["build"], query)
    query("device", "process", "launch", "--device", device["identifier"], BUNDLE, timeout=60)
    receipt = {"binding": binding, "source": build_config["revision"], "define": DEFINE, "app": observed,
               "ipa_sha256": hashlib.sha256(ipas[0].read_bytes()).hexdigest()}
    journal.artifact("qa_build", receipt)
    atomic_write(receipt_path, json.dumps(receipt, sort_keys=True).encode())
