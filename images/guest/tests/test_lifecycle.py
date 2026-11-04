import importlib
import json
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch

from guest.offline import stage_bundle
from guest.tests.fixtures import encoded
from guest.tests.test_offline import native_inputs


class LifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.value, self.manifest = native_inputs(self.root)
        self.bundle = self.root / 'bundle'
        stage_bundle(self.value, source=self.root, out=self.bundle)
        self.lock = json.loads((self.bundle / 'inputs.json').read_bytes())
        self.state = self.root / 'state'
        self.state.mkdir()
        self.installation = str(uuid.uuid4())
        (self.state / 'identity.json').write_bytes(encoded({'schemaVersion': 'bridge-state/v1', 'installationId': self.installation}))
        self.calls = []
        self.node_id = 'a' * 64
        self.has_node = False
        self.running = True
        self.fail_command = None
        self.create_then_fail = False
        try:
            self.api = importlib.import_module('guest.lifecycle')
        except ModuleNotFoundError:
            self.fail('guest lifecycle missing')
        self.addCleanup(patch.stopall)
        patch.object(self.api, 'BUNDLE', self.bundle).start()
        patch.object(self.api, 'execute', self.execute).start()

    def execute(self, argv, *, env=None, stdin=None):
        self.calls.append((argv, env))
        if self.fail_command and self.fail_command in argv:
            raise ValueError('injected phase failure')
        if argv[:2] == ['docker', 'ps']:
            return self.node_id if self.has_node else ''
        if argv[:2] == ['docker', 'inspect']:
            return json.dumps([{'Id': self.node_id, 'Image': self.manifest['node']['configDigest'], 'Config': {'Labels': {'io.x-k8s.kind.cluster': 'bridge-' + self.installation.replace('-', '')}}, 'State': {'Running': self.running}}])
        if argv[:2] == ['docker', 'start']:
            self.running = True
        if argv[0] == 'kmx' and 'up' in argv:
            self.has_node = True
            if self.create_then_fail:
                raise ValueError('partial create retained')
        if 'inspecti' in argv:
            reference = argv[-1]
            row = next(r for r in self.manifest['images'] if reference == r['repository'] + '@' + r['manifestDigest'])
            return json.dumps({'status': {'id': row['configDigest'], 'repoDigests': [reference]}})
        return ''

    def test_only_exact_offline_routes_and_no_provider_or_shadow_installer(self):
        result = self.api.bootstrap(self.lock, self.state)
        self.assertEqual('infrastructure-ready', result['phase'])
        commands = [args for args, _ in self.calls]
        kmx = [args for args in commands if args[0] == 'kmx']
        context = 'kind-bridge-' + self.installation.replace('-', '')
        self.assertEqual([
            ['kmx', '--context', context, 'cluster', 'up', '--step', 'cluster', '--offline-bundle', str(self.bundle)],
            ['kmx', '--context', context, 'cluster', 'orka', 'install', '--provider', '-', '--offline-bundle', str(self.bundle)],
        ], kmx)
        for args, env in self.calls:
            self.assertNotIn(args[0], ('helm', 'kind', 'ollama'))
            self.assertFalse(any(flag in args for flag in ('delete', '--force', '--force-conflicts', 'uninstall')))
            if args[0] == 'kmx':
                self.assertEqual(str(self.state / 'kubeconfig'), env['KUBECONFIG'])
                self.assertEqual('bridge-' + self.installation.replace('-', ''), env['KIND_CLUSTER'])
        self.assertEqual(self.node_id, result['nodeIds'][0])

    def test_resume_rechecks_and_starts_only_exact_journaled_ids(self):
        self.api.bootstrap(self.lock, self.state)
        self.calls.clear()
        self.running = False
        self.api.resume(self.lock, self.state)
        commands = [args for args, _ in self.calls]
        self.assertIn(['docker', 'start', self.node_id], commands)
        self.assertTrue(any('inspecti' in a for a in commands))
        self.assertTrue(any(a[0] == 'kmx' and 'install' in a for a in commands))
        self.node_id = 'b' * 64
        self.calls.clear()
        with self.assertRaisesRegex(ValueError, 'node identity'):
            self.api.resume(self.lock, self.state)
        self.assertFalse(any('start' in a or a[0] == 'kmx' for a, _ in self.calls))

    def test_missing_owned_node_cannot_recreate_cluster(self):
        self.api.bootstrap(self.lock, self.state)
        self.has_node = False
        self.calls.clear()
        with self.assertRaisesRegex(ValueError, 'node identity'):
            self.api.resume(self.lock, self.state)
        self.assertFalse(any(a[0] == 'kmx' for a, _ in self.calls))

    def test_failed_install_keeps_cache_phase_and_propagates_refusal(self):
        self.fail_command = 'install'
        with self.assertRaisesRegex(ValueError, 'injected'):
            self.api.bootstrap(self.lock, self.state)
        journal = json.loads((self.state / 'bootstrap.json').read_bytes())
        self.assertEqual('cache-verified', journal['phase'])
        self.assertEqual([self.node_id], journal['nodeIds'])
        self.calls.clear()
        with self.assertRaises(ValueError):
            self.api.resume(self.lock, self.state)
        self.assertFalse(any('delete' in a or '--force' in a for a, _ in self.calls))

    def test_failed_creation_without_visible_nodes_is_not_replayed(self):
        self.fail_command = 'up'
        with self.assertRaises(ValueError):
            self.api.bootstrap(self.lock, self.state)
        self.fail_command = None
        self.calls.clear()
        with self.assertRaisesRegex(ValueError, 'uncertain'):
            self.api.bootstrap(self.lock, self.state)
        self.assertFalse(any(args[0] == 'kmx' for args, _ in self.calls))

    def test_partial_create_is_retained_not_adopted_on_retry(self):
        self.create_then_fail = True
        with self.assertRaises(ValueError):
            self.api.bootstrap(self.lock, self.state)
        self.create_then_fail = False
        self.calls.clear()
        with self.assertRaisesRegex(ValueError, 'unjournaled'):
            self.api.bootstrap(self.lock, self.state)
        self.assertFalse(any(a[0] == 'kmx' for a, _ in self.calls))

    def test_corrupt_or_other_input_journal_blocks_before_commands(self):
        self.api.bootstrap(self.lock, self.state)
        self.calls.clear()
        (self.state / 'bootstrap.json').write_text('{}')
        with self.assertRaises(ValueError):
            self.api.bootstrap(self.lock, self.state)
        self.assertEqual([], self.calls)

    def test_concurrent_bootstrap_is_refused_without_commands(self):
        with self.api.operation_lock(self.state), self.assertRaisesRegex(ValueError, 'busy'):
            self.api.bootstrap(self.lock, self.state)
        self.assertEqual([], self.calls)

    def test_cli_retains_guard_reason_but_not_raw_system_errors(self):
        import io
        import subprocess
        for error, visible in ((ValueError('uncertain cluster creation retained; manual recovery required'), 'uncertain cluster creation'), (OSError('secret-system-detail'), 'OSError'), (subprocess.CalledProcessError(2, ['secret-command']), 'CalledProcessError')):
            stream = io.StringIO()
            with patch('sys.argv', ['lifecycle', 'bootstrap', '--lock', 'unused']), patch.object(self.api, 'load_lock', return_value=self.lock), patch.object(self.api, 'bootstrap', side_effect=error), patch('sys.stderr', stream):
                self.assertEqual(2, self.api.main())
            self.assertIn(visible, stream.getvalue())
            self.assertNotIn('secret-', stream.getvalue())

    def test_altered_archive_blocks_before_commands(self):
        (self.bundle / 'components/controller.oci.tar').write_bytes(b'wrong')
        with self.assertRaises(ValueError):
            self.api.bootstrap(self.lock, self.state)
        self.assertEqual([], self.calls)
