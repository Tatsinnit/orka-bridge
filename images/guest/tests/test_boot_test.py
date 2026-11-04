import importlib
import unittest
import uuid
from pathlib import Path


class ReferenceTests(unittest.TestCase):
    def module(self, name):
        try:
            return importlib.import_module("guest." + name)
        except ModuleNotFoundError:
            self.fail(name + " not implemented")

    def test_seed_explicitly_authorizes_new_or_existing(self):
        api = self.module("test_seed")
        id_ = str(uuid.uuid4())
        self.assertEqual("new", api.descriptor(id_, "new")["stateCreation"])
        self.assertEqual("existing", api.descriptor(id_, "existing")["stateCreation"])
        for mode in ("", "auto", "missing"):
            with self.assertRaises(ValueError):
                api.descriptor(id_, mode)
        with self.assertRaises(ValueError):
            api.descriptor("not-a-uuid", "new")

    def test_native_reference_cases_are_fixed_seed_metadata(self):
        api = self.module('test_seed')
        for case in ('infrastructure', 'offline-tasks', 'restart', 'interrupted'):
            self.assertEqual(case, api.descriptor(str(uuid.uuid4()), 'new', case_name=case)['caseId'])
        with self.assertRaises(ValueError):
            api.descriptor(str(uuid.uuid4()), 'new', case_name='shell')

    def test_qemu_devices_are_identified_and_egress_restricted(self):
        api = self.module("boot_test")
        args = api.qemu_argv(Path("/owned"), str(uuid.uuid4()), True)
        text = " ".join(map(str, args))
        for required in ("-accel kvm", "serial=bridge-os", "serial=bridge-state", "serial=bridge-test", "ai.orka.bridge.control", "restrict=on", "-no-reboot"):
            self.assertIn(required, text)
        for forbidden in ("hostfwd", "-nographic", "-snapshot", "cache=unsafe"):
            self.assertNotIn(forbidden, text)
        self.assertIn("readonly=on", text)

    def test_output_path_cannot_inject_qemu_options(self):
        api = self.module("boot_test")
        text = " ".join(map(str, api.qemu_argv(Path("/owned,a=b"), str(uuid.uuid4()), True)))
        self.assertIn("file=/owned,,a=b/os.qcow2", text)
        self.assertIn("path=/owned,,a=b/control.sock", text)

    def test_only_disposable_base_or_interruption_overlay_disables_auto_bootstrap(self):
        from unittest.mock import patch
        api = self.module('boot_test')
        for case in ('base', 'interrupted'):
            with patch.object(api, 'run') as command:
                api.instrument(Path('/owned/os.qcow2'), case)
            self.assertIn('ln -sf /dev/null /etc/systemd/system/bridge-infrastructure.service', command.call_args.args[0][-1])

    def test_missing_state_case_has_no_state_device(self):
        api = self.module("boot_test")
        text = " ".join(map(str, api.qemu_argv(Path("/owned"), str(uuid.uuid4()), False)))
        self.assertNotIn("serial=bridge-state", text)

    def test_nonhex_machine_id_is_not_a_valid_result(self):
        api = self.module("boot_test")
        import json
        result = {"schemaVersion": "bridge-guest-base/v1", "installationId": "id", "stateUuid": "id", "machineId": "z" * 32,
                  "sentinel": True, "stateBeforeDaemons": True, "machineIdBeforeSystemd": True, "controlPort": True,
                  "credentialsType": "tmpfs", "dockerRoot": "/var/lib/bridge/docker", "containerdRoot": "/var/lib/bridge/containerd"}
        with self.assertRaises(ValueError):
            api.parse_result("BRIDGE_BASE " + json.dumps(result), "id")

    def test_network_receipt_counts_external_http_attempts(self):
        import struct
        import tempfile
        api = self.module('infrastructure_test')
        self.assertTrue(callable(getattr(api, 'network_evidence', None)), 'pcap evidence missing')
        frame = b'\x00' * 12 + b'\x08\x00' + b'\x45' + b'\x00' * 8 + b'\x06' + b'\x00' * 2 + b'\x0a\x00\x02\x0f' + b'\x08\x08\x08\x08' + b'\x00' * 20
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'network.pcap'
            path.write_bytes(struct.pack('<IHHIIII', 0xa1b2c3d4, 2, 4, 0, 0, 65535, 1) + struct.pack('<IIII', 0, 0, len(frame), len(frame)) + frame)
            self.assertEqual(1, api.network_evidence(path)['externalIpv4TcpPackets'])

    def test_lan_tcp_is_not_exempt_from_egress_count(self):
        import struct
        import tempfile
        api = self.module('infrastructure_test')
        frame = b'\x00' * 12 + b'\x08\x00' + b'\x45' + b'\x00' * 8 + b'\x06' + b'\x00' * 2 + b'\x0a\x00\x02\x0f' + b'\xc0\xa8\x01\x0a' + b'\x00' * 20
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'network.pcap'
            path.write_bytes(struct.pack('<IHHIIII', 0xa1b2c3d4, 2, 4, 0, 0, 65535, 1) + struct.pack('<IIII', 0, 0, len(frame), len(frame)) + frame)
            result = api.network_evidence(path)
            self.assertEqual(1, result.get('externalIpv4TcpPackets', result.get('externalTcpPackets')))
            self.assertIn('externalIpv4TcpPackets', result)

    def checker_helpers(self):
        import ast
        import json
        import subprocess
        source = (Path(self.module('boot_test').__file__).with_name('tests') / 'fixtures/infrastructure-check').read_text()
        names = {'HelmRefusal', 'failed_helm_probe', 'refusal_phase', 'require_service_success'}
        nodes: list[ast.stmt] = [node for node in ast.parse(source).body if isinstance(node, (ast.FunctionDef, ast.ClassDef)) and node.name in names]
        self.assertEqual(names, {node.name for node in nodes if isinstance(node, (ast.FunctionDef, ast.ClassDef))}, 'isolated fixed checker guards missing')
        namespace = {'json': json, 'subprocess': subprocess}
        # Only fixed function/class definitions; never execute the guest's top-level actions.
        exec(compile(ast.Module(body=nodes, type_ignores=[]), '<isolated checker guards>', 'exec'), namespace)  # noqa: S102
        return namespace

    def test_checker_requires_successful_unit_and_specific_helm_refusal(self):
        import tempfile
        from types import SimpleNamespace
        from unittest.mock import patch
        api = self.checker_helpers()
        for fields, accepted in ((b'ActiveState=active\nResult=success\n', True), (b'ActiveState=failed\nResult=exit-code\n', False)):
            with patch('subprocess.run', return_value=SimpleNamespace(stdout=fields)):
                if accepted:
                    api['require_service_success']({})
                else:
                    with self.assertRaises(AssertionError):
                        api['require_service_success']({})
        for text, expected in ((b'offline Orka release identity/status differs; refusing adoption, overwrite or partial retry', api['HelmRefusal']), (b'other kmx failure', AssertionError)):
            with patch('subprocess.run', return_value=SimpleNamespace(returncode=1, stdout=b'', stderr=text)), self.assertRaises(expected):
                api['failed_helm_probe'](['kmx'], {})
        with tempfile.TemporaryDirectory() as temp:
            state = Path(temp)
            (state / 'bootstrap.json').write_text('{"phase":"cluster-ready"}')
            with self.assertRaises(AssertionError):
                api['refusal_phase'](state)
            (state / 'bootstrap.json').write_text('{"phase":"cache-verified"}')
            self.assertEqual('cache-verified', api['refusal_phase'](state))

    def test_task_receipt_is_not_a_clean_reuse_capability_and_dns_is_rejected(self):
        import hashlib
        import json
        import tempfile
        from unittest.mock import patch
        api = self.module('infrastructure_test')
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            image = root / 'os.qcow2'
            image.write_bytes(b'factory')
            lock = {'architecture': 'amd64'}
            build = {'sha256': hashlib.sha256(b'factory').hexdigest(), 'inputSha256': hashlib.sha256(api.encoded(lock)).hexdigest(), 'contract': {}}
            (root / 'build.json').write_text(json.dumps(build))
            fingerprints = {key: 'known' for key in ('nodeUID', 'namespaceUID', 'snapshotUID', 'snapshotDataSha256', 'nodeIds', '/etc/kubernetes/pki/ca.crt', '/etc/kubernetes/pki/sa.pub', '/etc/kubernetes/pki/etcd/ca.crt')}
            prior = root / 'prior.json'
            prior.write_text(json.dumps({'schemaVersion': 'bridge-guest-infrastructure/v1', 'case': 'offline-tasks', 'build': build, 'new': {'bootDescriptor': {'installationId': str(uuid.uuid4())}, 'result': {'phase': 'infrastructure-ready', 'fingerprints': fingerprints}}}))
            with patch.object(api, 'load_lock', return_value=lock), patch.object(api, 'image_info', return_value={'virtual-size': 16 * 1024**3}), patch.object(api, 'boot_case') as boot, self.assertRaisesRegex(ValueError, 'completed infrastructure'):
                api.infrastructure(image, root / 'unused', root / 'reuse', 'restart', from_receipt=prior)
            boot.assert_not_called()
            self.assertFalse((root / 'reuse').exists())
            with patch.object(api, 'load_lock', return_value=lock), patch.object(api, 'image_info', return_value={'virtual-size': 16 * 1024**3}), patch.object(api, 'boot_case', return_value={'serial': 'not reached'}), patch.object(api, 'network_evidence', return_value={'externalIpv4TcpPackets': 0, 'dnsQueries': ['registry.example.com']}), self.assertRaisesRegex(ValueError, 'DNS'):
                api.infrastructure(image, root / 'unused', root / 'dns', 'infrastructure')

    def test_base_receipt_is_rejected_before_boot_or_output_creation(self):
        import hashlib
        import json
        import tempfile
        from unittest.mock import patch
        api = self.module('infrastructure_test')
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            image = root / 'os.qcow2'
            image.write_bytes(b'factory')
            lock = {'architecture': 'amd64'}
            build = {'sha256': hashlib.sha256(b'factory').hexdigest(), 'inputSha256': hashlib.sha256(api.encoded(lock)).hexdigest(), 'contract': {}}
            (root / 'build.json').write_text(json.dumps(build))
            prior = root / 'prior.json'
            prior.write_text(json.dumps({'schemaVersion': 'bridge-guest-boot/v1', 'build': build, 'new': {'bootDescriptor': {'installationId': str(uuid.uuid4())}}}))
            with patch.object(api, 'load_lock', return_value=lock), patch.object(api, 'image_info', return_value={'virtual-size': 16 * 1024**3}), patch.object(api, 'boot_case') as boot, self.assertRaisesRegex(ValueError, 'completed infrastructure'):
                api.infrastructure(image, root / 'unused-lock', root / 'out', 'restart', from_receipt=prior)
            boot.assert_not_called()
            self.assertFalse((root / 'out').exists())

    def test_interrupted_case_refuses_receipt_reuse_before_any_work(self):
        import tempfile
        from unittest.mock import patch
        api = self.module('infrastructure_test')
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with patch.object(api, 'boot_case') as boot, self.assertRaisesRegex(ValueError, 'interrupted.*new state'):
                api.infrastructure(root / 'missing-image', root / 'missing-lock', root / 'out', 'interrupted', from_receipt=root / 'prior.json')
            boot.assert_not_called()
            self.assertFalse((root / 'out').exists())

    def test_failed_or_wrong_install_result_is_not_success(self):
        api = self.module("boot_test")
        with self.assertRaises(ValueError):
            api.parse_result("booted Linux", "id")
        with self.assertRaises(ValueError):
            api.parse_result('BRIDGE_BASE {"schemaVersion":"bridge-guest-base/v1","installationId":"other"}', "id")
