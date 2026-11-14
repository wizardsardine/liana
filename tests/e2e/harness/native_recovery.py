"""Signed native Recovery Kit lifecycle, separate from GUI/mobile acceptance.

Every action is a new owned process. No external receipt or arbitrary executable
can satisfy this gate. Preflight runs before credential, cloud or fixture access.
"""
from contextlib import contextmanager
import sys
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import uuid

from .state import Blocked, Failed, Journal, atomic_write, digest, private_read
from .native_recovery_verify import CHAINS, dependencies, verify

@contextmanager
def keep_awake():
    """Own one finite, parent-bound Darwin assertion; never unlock the host."""
    if sys.platform != 'darwin':
        yield
        return
    try:
        guard = subprocess.Popen(['/usr/bin/caffeinate', '-diu', '-t', '7200', '-w', str(os.getpid())],
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                 start_new_session=True)
    except OSError:
        raise Blocked('NATIVE_RECOVERY_AWAKE', 'Could not start the owned bounded awake assertion.') from None
    try:
        if guard.poll() is not None:
            raise Blocked('NATIVE_RECOVERY_AWAKE', 'The owned awake assertion exited before the native suite.')
        yield
    finally:
        if guard.poll() is None:
            guard.terminate()
        try:
            guard.wait(timeout=5)
        except subprocess.TimeoutExpired:
            guard.kill()
            guard.wait()


PREREQUISITE = '9e9a1f25995b4be9a6ab188240a2bf92d5f3209b'
IDENTITY = 'Developer ID Application: COINCUBE TECHNOLOGY LLC (8UVR249AD5)'
ORPHAN_SOURCE = '21503b253ebc29285da651c2b43dd7c3b5a8fbb1'
ORPHAN_HELPER = '0f039ee515d7918b288f916872272a26224bded02e89388beaf88f7a16fefaff'
LEGACY_CAPACITY_SOURCE = '74da94d2848783951ff9b305724340afbb230ab8'
LEGACY_CAPACITY_HELPER = 'f98dfad45951000a7538ad5fe595dcb6dddf9dc4c7b7d7f6cf31905dfa0ab48f'
ACTIONS = ('create_export', 'wipe_original', 'restore', 'reopen_sign')


def require(ok, code):
    if not ok:
        raise Failed('NATIVE_RECOVERY_' + code, 'Native recovery prerequisite/validation failed: ' + code)


def options(value):
    require(isinstance(value, dict) and set(value) == {'version', 'checkout', 'revision'}, 'CONFIG')
    require(type(value['version']) is int and value['version'] == 1
            and isinstance(value['checkout'], str) and Path(value['checkout']).is_absolute()
            and isinstance(value['revision'], str)
            and (value['revision'] == 'current' or re.fullmatch('[0-9a-f]{40}', value['revision'])), 'CONFIG')
    return dict(value)


def source(value):
    cfg = options(value); checkout = Path(cfg['checkout'])
    try:
        remote = subprocess.check_output(['git', 'remote', 'get-url', 'origin'], cwd=checkout).decode().strip()
        head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=checkout).decode().strip()
        tree = subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], cwd=checkout).decode().strip()
        clean = not subprocess.check_output(['git', 'status', '--porcelain'], cwd=checkout).strip()
        result = subprocess.run(['git', 'merge-base', '--is-ancestor', PREREQUISITE, head], cwd=checkout,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except (OSError, subprocess.SubprocessError):
        raise Blocked('NATIVE_RECOVERY_SOURCE', 'The native source binding could not be verified.') from None
    require(remote in ('git@github.com:coincubetech/coincube.git', 'https://github.com/coincubetech/coincube.git')
            and (cfg['revision'] == 'current' or cfg['revision'] == head), 'SOURCE_BINDING')
    if result.returncode:
        raise Blocked('NATIVE_RECOVERY_PR1174', 'Reviewed Recovery Kit cold-loader prerequisite is not integrated; no credentials or fixtures accessed.')
    if not clean:
        raise Blocked('NATIVE_RECOVERY_DIRTY', 'Native recovery requires reviewed clean source; no live action attempted.')
    return {'head': head, 'tree': tree, 'prerequisite': PREREQUISITE}


def preflight(value):
    binding = source(value)
    dependencies()  # Before credentials, helper launch or mobile fixtures.
    return binding


def build(cfg, binding):
    """Build/sign only this source. No caller-supplied binary or compile assertion."""
    checkout = Path(cfg['checkout']); local = checkout / 'tests/e2e/.local/native-recovery'
    local.mkdir(mode=0o700, parents=True, exist_ok=True)
    require(not local.is_symlink() and not local.stat().st_mode & 0o077, 'BUILD_DIRECTORY')
    import fcntl
    with (local / 'build.lock').open('a+b') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        require(source(cfg) == binding, 'SOURCE_CHANGED')
        bundle = local / 'RecoveryNative.app'; executable = bundle / 'Contents/MacOS/recovery_kit_native'
        receipt_path = local / 'build.json'
        if receipt_path.exists() and executable.exists():
            receipt = json.loads(private_read(receipt_path))
            if receipt.get('source') == binding and receipt.get('sha256') == hashlib.sha256(executable.read_bytes()).hexdigest():
                checked = subprocess.run(['codesign', '--verify', '--strict', str(bundle)], capture_output=True)
                profile = checkout / 'contrib/release/macos'
                identity_check = subprocess.run([str(profile / 'check-profile-authorises-cert.sh'), str(profile / 'embedded.provisionprofile'), '--signed-bundle', str(bundle)], capture_output=True)
                authority = subprocess.run(['codesign', '-d', '--verbose=4', str(bundle)], capture_output=True)
                if (checked.returncode == identity_check.returncode == authority.returncode == 0
                        and ('Authority=' + IDENTITY).encode() in authority.stderr
                        and b'TeamIdentifier=8UVR249AD5' in authority.stderr):
                    return executable, receipt
        env = dict(os.environ, BREEZ_API_KEY='native-recovery-fixture-placeholder',
                   COINCUBE_API_URL='https://api.coincube.io',
                   COINCUBE_NATIVE_RECOVERY_SOURCE=binding['head'], COINCUBE_NATIVE_RECOVERY_PREREQUISITE='1',
                   CARGO_TARGET_DIR=str(checkout / 'target'), CARGO_INCREMENTAL='0', CARGO_BUILD_JOBS='8')
        env['PATH'] = '/opt/homebrew/opt/rustup/bin:' + env.get('PATH', '')
        release = checkout / 'contrib/release/macos'
        try:
            with (local / 'build.log').open('wb') as log:
                def run(argv):
                    subprocess.run([str(a) for a in argv], cwd=checkout, env=env, stdout=log, stderr=log,
                                   check=True, timeout=1800)
                run(['cargo', 'build', '--locked', '-p', 'coincube-gui', '--features', 'integration-tests', '--example', 'recovery_kit_native'])
                run([release / 'check-profile-authorises-cert.sh', release / 'embedded.provisionprofile', '--identity', IDENTITY])
                executable.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                import plistlib
                with (bundle / 'Contents/Info.plist').open('wb') as f:
                    plistlib.dump({'CFBundleIdentifier': 'io.coincube.tenshu', 'CFBundleExecutable': 'recovery_kit_native',
                                  'CFBundleName': 'NativeRecoveryQA', 'CFBundlePackageType': 'APPL', 'CFBundleVersion': '1'}, f)
                shutil.copy2(checkout / 'target/debug/examples/recovery_kit_native', executable)
                shutil.copy2(release / 'embedded.provisionprofile', bundle / 'Contents/embedded.provisionprofile')
                run(['codesign', '-f', '-s', IDENTITY, '--options', 'runtime', '--entitlements', release / 'coincube.entitlements', bundle])
                run(['codesign', '--verify', '--strict', bundle])
                run([release / 'check-profile-authorises-cert.sh', release / 'embedded.provisionprofile', '--signed-bundle', bundle])
        except (OSError, subprocess.SubprocessError):
            raise Blocked('NATIVE_RECOVERY_BUILD', 'Native build/signing unavailable; private diagnostics retained.') from None
        require(source(cfg) == binding, 'SOURCE_CHANGED')
        receipt = {'version': 1, 'source': binding, 'sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
                   'signing_identity': IDENTITY, 'provenance': 'native_product_integration'}
        atomic_write(receipt_path, json.dumps(receipt).encode())
        return executable, receipt


def signed_identity(checkout, executable):
    bundle = executable.parents[2]
    release = checkout / 'contrib/release/macos'
    try:
        checked = subprocess.run(['codesign', '--verify', '--strict', str(bundle)], capture_output=True, timeout=30)
        authority = subprocess.run(['codesign', '-d', '--verbose=4', str(bundle)], capture_output=True, timeout=30)
        profile = subprocess.run([str(release / 'check-profile-authorises-cert.sh'), str(release / 'embedded.provisionprofile'),
                                  '--signed-bundle', str(bundle)], capture_output=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        raise Blocked('NATIVE_RECOVERY_HELPER_IDENTITY', 'Signed helper identity could not be verified.') from None
    require(checked.returncode == authority.returncode == profile.returncode == 0
            and ('Authority=' + IDENTITY).encode() in authority.stderr and b'TeamIdentifier=8UVR249AD5' in authority.stderr,
            'HELPER_IDENTITY')


def checked_artifact(cfg, binding, executable, receipt):
    require(source(cfg) == binding and receipt.get('source') == binding
            and hashlib.sha256(executable.read_bytes()).hexdigest() == receipt.get('sha256'), 'BUILD_CHANGED')
    signed_identity(Path(cfg['checkout']), executable)


def checked_process(cfg, binding, executable, receipt, request, directory, *, access_token=None):
    checked_artifact(cfg, binding, executable, receipt)
    try:
        return process(executable, request, directory, access_token=access_token)
    finally:
        checked_artifact(cfg, binding, executable, receipt)


def process(executable, request, directory, *, access_token=None):
    """Wait on fresh process; sample kernel start before providing any action."""
    modes = {'admit_create': '--admit-create', 'observe_cleanup': '--observe-cleanup', 'begin_cleanup': '--begin-cleanup', 'audit_capacity_abandonment': '--audit-capacity-abandonment', 'bind_auth': '--bind-auth', 'refresh_auth': '--refresh-auth', 'retire_orphan_215': '--retire-orphan-215'}
    require(access_token is None or request['action'] in modes, 'AUTH_ROLE')
    argv = [str(executable)] + ([modes[request['action']]] if access_token is not None else [])
    env = dict(os.environ, COINCUBE_API_URL='https://api.coincube.io')
    # Do not allow runtime environment to assert source/dependency provenance.
    env.pop('COINCUBE_NATIVE_RECOVERY_SOURCE', None); env.pop('COINCUBE_NATIVE_RECOVERY_PREREQUISITE', None)
    try:
        with (directory / 'native-stderr.log').open('ab') as errors:
            process_dir = directory / 'native-processes'
            process_dir.mkdir(mode=0o700, exist_ok=True)
            require(not process_dir.is_symlink() and process_dir.stat().st_uid == os.getuid()
                    and process_dir.stat().st_mode & 0o077 == 0, 'PROCESS_DIRECTORY')
            record_path = process_dir / (str(uuid.uuid4()) + '.json')
            identity = {'version': 1, 'pid': None, 'start_id': None, 'action': request['action'],
                        'config_sha256': digest(request['config']), 'payload_sent': False, 'exited': False}
            child = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors,
                                     env=env, start_new_session=True)
            try:
                identity['pid'] = child.pid
                atomic_write(record_path, json.dumps(identity).encode())
                start = subprocess.check_output(['/bin/ps', '-p', str(child.pid), '-o', 'lstart=']).decode().strip()
                require(bool(start), 'KERNEL_PROCESS')
                identity['start_id'] = start
                atomic_write(record_path, json.dumps(identity).encode())
                payload = dict(request)
                if access_token is not None:
                    payload = {'config': request['config'], 'access_token': access_token}
                    if request['action'] in ('audit_capacity_abandonment', 'retire_orphan_215'):
                        payload['old_directory'] = request['old_directory']
                    if request['action'] == 'retire_orphan_215':
                        payload['old_checkpoint_sha256'] = request['old_checkpoint_sha256']
                        payload['old_state_sha256'] = request['old_state_sha256']
                identity['payload_sent'] = True
                atomic_write(record_path, json.dumps(identity).encode())
                output, _ = child.communicate(json.dumps(payload).encode(), timeout=1380 if request['action'] in ('wipe_original', 'retire_remote', 'retire_orphan_215') else 660)
                identity.update(exited=True, returncode=child.returncode)
                atomic_write(record_path, json.dumps(identity).encode())
            except BaseException:
                child.kill(); child.wait()
                identity.update(exited=True, returncode=child.returncode)
                try:
                    atomic_write(record_path, json.dumps(identity).encode())
                except OSError:
                    pass  # Preserve the original failure after reaping the child.
                raise
    except (OSError, subprocess.TimeoutExpired):
        raise Blocked('NATIVE_RECOVERY_PROCESS', 'Native process unavailable or timed out; retain uncertain fixture evidence.') from None
    require(len(output) < 4096, 'PROTOCOL_SIZE')
    reply = json.loads(output)
    if isinstance(reply.get('code'), str) and re.fullmatch('[A-Za-z0-9_]{1,96}', reply['code']):
        identity['code'] = reply['code']; atomic_write(record_path, json.dumps(identity).encode())
    if child.returncode or reply.get('ok') is not True:
        code = reply.get('code')
        require(isinstance(code, str) and re.fullmatch('[A-Za-z0-9_]{1,96}', code), 'PROTOCOL')
        raise NativeActionBlocked(code)
    require(reply == {'ok': True, 'provenance': 'native_product_integration', 'funded': False, 'broadcast': False}, 'PROTOCOL')
    return {'pid': child.pid, 'start_id': start, 'exited': True, 'returncode': child.returncode}


class NativeActionBlocked(Blocked):
    def __init__(self, native_code):
        self.native_code = native_code
        super().__init__('NATIVE_RECOVERY_ACTION', 'Native action stopped: ' + native_code)


def legacy_tools(cfg, binding):
    """Build the actual backup/pending helpers from the frozen reviewed source."""
    require(source(cfg) == binding, 'SOURCE_CHANGED')
    checkout = Path(cfg['checkout'])
    try:
        subprocess.run([str(checkout / 'tests/e2e/build-native')], cwd=checkout,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1800, check=True)
    except (OSError, subprocess.SubprocessError):
        raise Blocked('NATIVE_RECOVERY_CLEANUP_BUILD', 'Pinned backup/pending helpers could not be built and signed.') from None
    local = checkout / 'tests/e2e/.local'
    receipt = json.loads(private_read(local / 'native-build.json'))
    provision = local / 'Provision.app/Contents/MacOS/testnet4_provision'
    signing = local / 'testnet4_connect'
    require(source(cfg) == binding and receipt.get('head') == binding['head']
            and receipt.get('provision_sha256') == hashlib.sha256(provision.read_bytes()).hexdigest()
            and receipt.get('signing_sha256') == hashlib.sha256(signing.read_bytes()).hexdigest(), 'CLEANUP_BUILD')
    signed_identity(checkout, provision)
    return provision, signing, receipt


class NativeCleanupContext:
    """A native consumer's real account, retained history and pinned tools.

    No StrictPolicy impersonation. Full-flow contexts additionally retain the
    actual parent phone observation; standalone contexts have no phone claim.
    """
    def __init__(self, suite, account, owner_id, *, parent=None, workstation=None):
        from . import provisioning
        self.suite, self.parent, self.workstation = suite, parent, workstation
        if parent is not None:
            require(workstation is None and parent.account == account and parent.account_user_id == owner_id, 'CLEANUP_ACCOUNT')
            config, profile = parent.config, parent.profile
            self.journal, self.run_id = parent.journal, parent.run_id
        else:
            require(isinstance(workstation, str) and Path(workstation).is_absolute(), 'CLEANUP_CONFIG')
            self.workstation_raw = private_read(workstation)
            config = provisioning.validate_config(json.loads(self.workstation_raw))
            require(config['account_email'].casefold() == account['email'].casefold(), 'CLEANUP_ACCOUNT')
            profile = Path(config['profile_dir'])
            self.journal, self.run_id = suite.journal, str(uuid.uuid5(uuid.NAMESPACE_URL, 'native-cleanup/' + str(suite.directory)))
        require('qa_cleanup' in config and config['account_email'].casefold() == account['email'].casefold(), 'CLEANUP_CONFIG')
        self.config, self.profile = json.loads(json.dumps(config)), Path(profile)
        self.config_binding = digest(self.config)
        self.account, self.account_user_id = account, owner_id
        self.artifacts = self.journal.data['artifacts']
        expected = digest({'email': account['email'].casefold(), 'user_id': owner_id})
        require(self.artifacts.get('account_identity', expected) == expected, 'CLEANUP_ACCOUNT')
        if parent is None:
            self.journal.artifact('account_identity', expected)
        # No signed smoke-test fixtures are created until real full-capacity
        # admission or a previously selected retirement actually needs tools.
        self.tools = None

    def verify_protection_view(self):
        require(source(self.suite.config) == self.suite.binding, 'SOURCE_CHANGED')
        require(digest(self.config) == self.config_binding, 'CLEANUP_CONFIG_CHANGED')
        if self.parent is not None:
            require(digest(self.parent.config) == self.config_binding, 'CLEANUP_CONFIG_CHANGED')
            require(self.parent.account == self.account and self.parent.account_user_id == self.account_user_id
                    and self.parent.strict is not None and not self.parent.strict.requested, 'CLEANUP_ACCOUNT')
            self.parent.strict.observe()
        else:
            require(private_read(self.workstation) == self.workstation_raw, 'CLEANUP_CONFIG_CHANGED')
        cache = json.loads(private_read(self.profile / 'testnet4/connect.json'))
        matches = [a for a in cache['accounts'] if a.get('email') == cache.get('active_email') == self.account['email']]
        require(matches == [self.account], 'CLEANUP_ACCOUNT')
    def observe(self):
        self.verify_protection_view()
        if self.tools is None:
            from .provisioning import Provision
            self.provision_binary, self.signing_binary, self.tools = legacy_tools(self.suite.config, self.suite.binding)
            self.provision = Provision(self.provision_binary, self.profile, self.journal.directory, self.run_id)
        require(self.tools['head'] == self.suite.binding['head']
                and hashlib.sha256(self.provision_binary.read_bytes()).hexdigest() == self.tools['provision_sha256']
                and hashlib.sha256(Path(self.signing_binary).read_bytes()).hexdigest() == self.tools['signing_sha256'], 'CLEANUP_BUILD_CHANGED')
        signed_identity(Path(self.suite.config['checkout']), self.provision_binary)


class NativeCleanupConsumerV1:
    """Live native BEFORE-CREATE admission, with a real signed observer."""
    def __init__(self, suite, runner, config, account, owner_id, executable, receipt):
        require(config.get('chain') == 'testnet4', 'CLEANUP_CHAIN')
        require(type(runner) is NativeCleanupContext, 'CLEANUP_ADMISSION')
        self.suite, self.runner, self.config = suite, runner, dict(config)
        self.account, self.owner_id = account, owner_id
        self.executable, self.receipt = executable, receipt
        self.consumer_binding = digest(suite.consumer)

    def select(self):
        self.observe()
        checked_process(self.suite.config, self.suite.binding, self.executable, self.receipt,
                        {'action': 'begin_cleanup', 'config': self.config}, self.suite.directory,
                        access_token=self.account['tokens']['access_token'])

    def observe(self):
        suite, runner = self.suite, self.runner
        require(self.config.get('chain') == 'testnet4', 'CLEANUP_CHAIN')
        require(suite.journal.lock is not None and os.fstat(suite.journal.lock).st_uid == os.getuid(), 'CLEANUP_LEASE')
        require(self.consumer_binding == digest(suite.consumer)
                and self.config['owner_id'] == self.owner_id == runner.account_user_id
                and runner.artifacts['account_identity'] == digest({'email': self.account['email'].casefold(), 'user_id': self.owner_id})
                and runner.account == self.account, 'CLEANUP_ACCOUNT')
        records = suite.journal.data['artifacts'].get(self.config['chain'], {})
        require(not records and not suite.journal.data['artifacts'].get('pending_native_action')
                and not Path(self.config['case_root']).exists(), 'CLEANUP_PHASE')
        checked_process(suite.config, suite.binding, self.executable, self.receipt,
                        {'action': 'observe_cleanup', 'config': self.config}, suite.directory,
                        access_token=self.account['tokens']['access_token'])
        runner.observe()  # Pin legacy helpers only after live native admission.


class NativeRecovery:
    def __init__(self, cfg, directory, consumer):
        self.config = options(cfg); self.binding = preflight(self.config)  # BEFORE any run/credential access.
        self.directory = Path(directory).absolute()
        self.journal = Journal(self.directory, {'native_recovery_version': 1, 'config': self.config,
                                              'consumer': consumer, 'source': self.binding})
        self.consumer = consumer

    def close(self):
        self.journal.close()

    def abandon_capacity(self, account, owner_id, old_directory, archived_helper):
        """One reviewed historical failure, never a general receipt importer.

        No old process is executed and no old files/Keychain items are changed.
        New signed code independently observes absence and fixed QA handles.
        """
        require(type(owner_id) is int and owner_id > 0, 'ACCOUNT')
        old = Path(old_directory)
        require(old.is_absolute() and old.resolve() == old and old != self.directory, 'ABANDON_DIRECTORY')
        metadata = old.stat()
        require(metadata.st_uid == os.getuid() and metadata.st_mode & 0o077 == 0, 'ABANDON_DIRECTORY')
        archived = Path(archived_helper)
        require(archived.is_absolute() and archived.name == 'recovery_kit_native', 'ABANDON_HELPER')
        require(hashlib.sha256(archived.read_bytes()).hexdigest() == LEGACY_CAPACITY_HELPER, 'ABANDON_HELPER')
        signed_identity(Path(self.config['checkout']), archived)
        old_checkpoint_raw = private_read(old / 'checkpoint.json')
        old_checkpoint = json.loads(old_checkpoint_raw)
        artifacts = old_checkpoint['artifacts']
        require(artifacts['build']['sha256'] == LEGACY_CAPACITY_HELPER
                and artifacts['build']['source']['head'] == LEGACY_CAPACITY_SOURCE
                and set(artifacts) <= {'build', 'testnet4', 'pending_native_action'}, 'ABANDON_CHECKPOINT')
        scope = str(uuid.uuid5(uuid.NAMESPACE_URL, 'native-recovery/' + str(old) + '/testnet4'))
        old_case = old / scope
        raw = private_read(old_case / 'state.json')
        old_config = json.loads(raw)['config']
        require(old_config == {'version': 1, 'case_root': str(old_case), 'run_id': scope, 'chain': 'testnet4',
                'auth_account': hashlib.sha256(f'recovery-native-v1:{old_case}:{scope}:testnet4:auth'.encode()).hexdigest(),
                'owner_id': owner_id, 'source_head': LEGACY_CAPACITY_SOURCE}, 'ABANDON_CONFIG')
        require(set(artifacts['testnet4']) == {'preflight', 'bind_auth'}
                and artifacts['pending_native_action'] == {'chain': 'testnet4', 'action': 'create_export', 'config_sha256': digest(old_config)}, 'ABANDON_CHECKPOINT')
        executable, receipt = build(self.config, self.binding)
        scope = str(uuid.uuid5(uuid.NAMESPACE_URL, 'native-recovery/' + str(self.directory.resolve()) + '/testnet4'))
        case = self.directory / scope
        config = {'version': 1, 'case_root': str(case), 'run_id': scope, 'chain': 'testnet4',
                  'auth_account': hashlib.sha256(f'recovery-native-v1:{case}:{scope}:testnet4:auth'.encode()).hexdigest(),
                  'owner_id': owner_id, 'source_head': self.binding['head']}
        marker = {'version': 1, 'kind': 'abandon_no_cube_capacity', 'old_source': LEGACY_CAPACITY_SOURCE,
                  'old_helper_sha256': LEGACY_CAPACITY_HELPER, 'old_directory': str(old),
                  'old_checkpoint_sha256': hashlib.sha256(old_checkpoint_raw).hexdigest(),
                  'old_state_sha256': hashlib.sha256(raw).hexdigest(), 'new_source': self.binding,
                  'new_helper_sha256': receipt['sha256'], 'new_consumer': self.consumer,
                  'account': digest({'email': account['email'].casefold(), 'user_id': owner_id}),
                  'all_old_material_retained': True, 'native_acceptance': False}
        previous = self.journal.data['artifacts'].get('capacity_abandonment')
        require(previous is None or previous['binding'] == marker, 'ABANDON_CHANGED')
        require(previous is not None or not any(chain in self.journal.data['artifacts'] for chain in CHAINS), 'ABANDON_NEW_NOT_FRESH')
        observed = checked_process(self.config, self.binding, executable, receipt,
                                  {'action': 'audit_capacity_abandonment', 'config': config, 'old_directory': str(old)}, self.directory,
                                  access_token=account['tokens']['access_token'])
        require(private_read(old / 'checkpoint.json') == old_checkpoint_raw
                and private_read(old_case / 'state.json') == raw
                and hashlib.sha256(archived.read_bytes()).hexdigest() == LEGACY_CAPACITY_HELPER, 'ABANDON_CHANGED')
        signed_identity(Path(self.config['checkout']), archived)
        if previous is None:
            self.journal.artifact('capacity_abandonment', {'binding': marker, 'observer': observed})
        return marker

    def retire_orphan_215(self, account, owner_id, old_directory, archived_helper, workstation):
        """Exact215 remote metadata only. Old lifecycle stays BLOCKED/intact."""
        from . import qa_cleanup, qa_funding
        context = NativeCleanupContext(self, account, owner_id, workstation=workstation)
        context.verify_protection_view()  # Real configured/global QA protections.
        old = Path(old_directory)
        require(old.is_absolute() and old.resolve() == old and not self.directory.is_relative_to(old)
                and not old.is_relative_to(self.directory), 'ORPHAN_DIRECTORY')
        archive = Path(archived_helper)
        require(archive.is_absolute() and archive.name == 'recovery_kit_native'
                and hashlib.sha256(archive.read_bytes()).hexdigest() == ORPHAN_HELPER, 'ORPHAN_HELPER')
        signed_identity(Path(self.config['checkout']), archive)
        require(subprocess.run(['git','merge-base','--is-ancestor',ORPHAN_SOURCE,self.binding['head']],
                              cwd=self.config['checkout'],capture_output=True).returncode == 0, 'ORPHAN_SOURCE')
        cp_raw = private_read(old/'checkpoint.json'); cp = json.loads(cp_raw)
        artifacts = cp['artifacts']
        scope = str(uuid.uuid5(uuid.NAMESPACE_URL, 'native-recovery/'+str(old)+'/testnet4'))
        old_case = old/scope; raw = private_read(old_case/'state.json'); state = json.loads(raw)
        config = state['config']
        require(artifacts['build']['source']['head'] == ORPHAN_SOURCE and artifacts['build']['sha256'] == ORPHAN_HELPER
                and set(artifacts.get('testnet4',{})) == {'preflight','bind_auth','create_export'}
                and 'bitcoin-blake2b-testnet4' not in artifacts and 'native_recovery_acceptance' not in artifacts
                and artifacts['pending_native_action'] == {'chain':'testnet4','action':'wipe_original','config_sha256':digest(config)}
                and config['case_root'] == str(old_case) and config['run_id'] == scope and config['source_head'] == ORPHAN_SOURCE
                and config['owner_id'] == owner_id and config['chain'] == 'testnet4'
                and artifacts['account_identity'] == digest({'email':account['email'].casefold(),'user_id':owner_id}), 'ORPHAN_ORIGIN')
        remote = state['identity']['remote_cube_id']
        require(type(remote) is int and remote > 0, 'ORPHAN_ORIGIN')
        executable, receipt = build(self.config,self.binding)
        operation = str(uuid.uuid5(uuid.NAMESPACE_URL,'native-orphan-215/'+str(self.directory.resolve())+'/'+scope))
        root = self.directory/operation
        current = {'version':1,'case_root':str(root),'run_id':operation,'chain':'testnet4','owner_id':owner_id,
                   'source_head':self.binding['head'],
                   'auth_account':hashlib.sha256(f'recovery-native-v1:{root}:{operation}:testnet4:auth'.encode()).hexdigest()}
        request = {'action':'retire_orphan_215','config':current,'old_directory':str(old),
                   'old_checkpoint_sha256':hashlib.sha256(cp_raw).hexdigest(),'old_state_sha256':hashlib.sha256(raw).hexdigest()}
        with qa_funding.pool() as data:
            # Hold the actual funding registry lease through the signed audit,
            # oracle and DELETE so no managed source can be selected concurrently.
            context.verify_protection_view()
            require(remote not in qa_cleanup.protected_ids(context,data), 'ORPHAN_PROTECTED')
            self.journal.artifact('pending_orphan_remote', {'request_sha256':digest(request),'helper_sha256':receipt['sha256']})
            observed = checked_process(self.config,self.binding,executable,receipt,request,self.directory,
                                       access_token=account['tokens']['access_token'])
            context.verify_protection_view()
            require(remote not in qa_cleanup.protected_ids(context,data)
                    and private_read(old/'checkpoint.json') == cp_raw and private_read(old_case/'state.json') == raw,
                    'ORPHAN_RETAINED_CHANGED')
            result = json.loads(private_read(root/'orphan.json'))
            require(result['config'] == current and result['phase'] == 'complete' and result['remote_id'] == remote
                    and result['old_checkpoint_sha256'] == request['old_checkpoint_sha256']
                    and result['old_state_sha256'] == request['old_state_sha256'], 'ORPHAN_RESULT')
            outcome = {'version':1,'origin_source':ORPHAN_SOURCE,'origin_helper_sha256':ORPHAN_HELPER,
                       'executor_source':self.binding,'executor_helper_sha256':receipt['sha256'],
                       'remote_id':remote,'origin_uuid':scope,'old_checkpoint_sha256':request['old_checkpoint_sha256'],
                       'old_state_sha256':request['old_state_sha256'],'operation':operation,'observer':observed,
                       'remote_metadata_retired':True,'local_recovery_retained':True,'old_flow':'BLOCKED',
                       'native_acceptance':False,'gui':False,'funded':False,'broadcast':False}
            data.setdefault('native_orphan_retirements',{})[digest({'source':ORPHAN_SOURCE,'uuid':scope,'remote':remote})] = outcome
            qa_funding.save_pool(data)
            self.journal.artifact('orphan_remote_retirement',outcome)
            del self.journal.data['artifacts']['pending_orphan_remote']; self.journal.save()
        return outcome

    def execute(self, account, owner_id, *, cleanup_runner=None, cleanup_workstation=None):
        require(type(owner_id) is int and owner_id > 0, 'ACCOUNT')
        executable, receipt = build(self.config, self.binding)
        previous = self.journal.data['artifacts'].get('build')
        require(previous is None or previous == receipt, 'BUILD_CHANGED')
        checked_artifact(self.config, self.binding, executable, receipt)
        self.journal.artifact('build', receipt)
        cleanup_context = None
        for chain in CHAINS:
            scope = str(uuid.uuid5(uuid.NAMESPACE_URL, 'native-recovery/' + str(self.directory.resolve()) + '/' + chain))
            case = self.directory / scope
            auth_account = hashlib.sha256(f'recovery-native-v1:{case}:{scope}:{chain}:auth'.encode()).hexdigest()
            config = {'version': 1, 'case_root': str(case), 'run_id': scope, 'chain': chain,
                      'auth_account': auth_account, 'owner_id': owner_id, 'source_head': self.binding['head']}
            records = self.journal.data['artifacts'].setdefault(chain, {})
            pending = self.journal.data['artifacts'].get('pending_native_action')
            if pending is not None and 'retire_remote' not in records:
                require(pending == {'chain': chain, 'action': 'retire_remote', 'config_sha256': digest(config)}
                        and all(action in records for action in ('preflight', 'bind_auth', *ACTIONS)), 'UNCERTAIN_RETIREMENT')
            if not records:
                # Read-only and deliberately not journaled as a secret-producing
                # action: quota refusal must leave no fixture credentials.
                if chain == 'testnet4' and cleanup_context is None and (cleanup_runner is not None or cleanup_workstation is not None):
                    cleanup_context = NativeCleanupContext(self, account, owner_id, parent=cleanup_runner, workstation=cleanup_workstation)
                admission = (NativeCleanupConsumerV1(self, cleanup_context, config, account, owner_id, executable, receipt)
                             if chain == 'testnet4' and cleanup_context is not None else None)
                from . import qa_cleanup
                if admission is not None:
                    qa_cleanup.reconcile_capacity(cleanup_context, admission=admission)
                try:
                    checked_process(self.config, self.binding, executable, receipt,
                                    {'action': 'admit_create', 'config': config}, self.directory,
                                    access_token=account['tokens']['access_token'])
                except NativeActionBlocked as error:
                    if error.native_code != 'native_capacity_blocked' or admission is None:
                        raise
                    qa_cleanup.ensure_capacity(cleanup_context, admission=admission)
                    checked_process(self.config, self.binding, executable, receipt,
                                    {'action': 'admit_create', 'config': config}, self.directory,
                                    access_token=account['tokens']['access_token'])
            for action in ('preflight', 'bind_auth', *ACTIONS):
                if action in records:
                    continue  # Exact same consumer/action only; proofs below revalidated.
                if self.journal.data['artifacts'].get('pending_native_action'):
                    raise Blocked('NATIVE_RECOVERY_UNCERTAIN', 'A native action has uncertain outcome; preserve the sealed fixture and reconcile it.')
                require(source(self.config) == self.binding and hashlib.sha256(executable.read_bytes()).hexdigest() == receipt['sha256'], 'BUILD_CHANGED')
                if action in ('wipe_original', 'restore', 'reopen_sign'):
                    checked_process(self.config, self.binding, executable, receipt, {'action': 'refresh_auth', 'config': config}, self.directory,
                                    access_token=account['tokens']['access_token'])
                self.journal.artifact('pending_native_action', {'chain': chain, 'action': action, 'config_sha256': digest(config)})
                record = checked_process(self.config, self.binding, executable, receipt, {'action': action, 'config': config}, self.directory,
                                 access_token=account['tokens']['access_token'] if action == 'bind_auth' else None)
                records[action] = record
                if action in ('create_export', 'restore'):
                    state = json.loads(private_read(case / 'state.json'))
                    require(state['config'] == config and state['pending'] is None
                            and state['creator' if action == 'create_export' else 'restorer'] == {k: record[k] for k in ('pid', 'start_id')},
                            'PROCESS_BINDING')
                del self.journal.data['artifacts']['pending_native_action']; self.journal.save()
            state_raw = private_read(case / 'state.json')
            if 'retire_remote' in records:
                require(hashlib.sha256(state_raw).hexdigest() == records['state_sha256'], 'RETAINED_STATE_CHANGED')
            state = json.loads(state_raw)
            require(state['config'] == config and state['stage'] in ('signed_unfunded', 'retired_unfunded') and state['pending'] in (None, 'retire_remote')
                    and state['creator']['pid'] != state['restorer']['pid'] != records['reopen_sign']['pid']
                    and state['original_factor'] != state['restored_factor']
                    and not (case / 'original').exists() and (case / 'quarantined-original').is_dir(), 'RESTORED_BINDING')
            raw = private_read(case / 'unfunded-key-control.psbt')
            require(hashlib.sha256(raw).hexdigest() == state['proof_sha256'], 'EXPORT_CHANGED')
            proof = verify(raw, state['identity'], chain)
            records['proof'] = proof; records['state_sha256'] = hashlib.sha256(private_read(case / 'state.json')).hexdigest()
            self.journal.save()
            if 'retire_remote' not in records:
                checked_process(self.config, self.binding, executable, receipt, {'action': 'refresh_auth', 'config': config}, self.directory,
                                access_token=account['tokens']['access_token'])
                self.journal.artifact('pending_native_action', {'chain': chain, 'action': 'retire_remote', 'config_sha256': digest(config)})
                records['retire_remote'] = checked_process(self.config, self.binding, executable, receipt, {'action': 'retire_remote', 'config': config}, self.directory)
                retired = json.loads(private_read(case / 'state.json'))
                require(retired['stage'] == 'retired_unfunded' and retired['pending'] is None and retired['proof_sha256'] == proof['psbt_sha256'], 'RETIREMENT')
                records['state_sha256'] = hashlib.sha256(private_read(case / 'state.json')).hexdigest()
                del self.journal.data['artifacts']['pending_native_action']; self.journal.save()
        acceptance = {'version': 1, 'consumer': self.consumer, 'source': self.binding, 'helper_sha256': receipt['sha256'],
                      'chains': {chain: self.journal.data['artifacts'][chain]['proof'] for chain in CHAINS},
                      'gui': False, 'funded': False, 'broadcast': False, 'remote_retirement': 'verified_native_fixture_only'}
        checked_artifact(self.config, self.binding, executable, receipt)
        self.journal.artifact('native_recovery_acceptance', acceptance)
        checked_artifact(self.config, self.binding, executable, receipt)
        return acceptance



def gate(runner):
    cfg = runner.config.get('native_recovery')
    if cfg is None:
        runner.journal.artifact('recovery_coverage', 'legacy_mobile_only')
        return
    suite = NativeRecovery(cfg, runner.journal.directory / 'native-recovery',
                           {'run_id': runner.run_id, 'binding': runner.journal.data['binding']})
    try:
        proof = suite.execute(runner.account, runner.account_user_id, cleanup_runner=runner if 'qa_cleanup' in runner.config else None)
        runner.journal.artifact('native_recovery_acceptance_version', 1)
        runner.journal.artifact('native_recovery_acceptance', proof)
    finally:
        suite.close()
