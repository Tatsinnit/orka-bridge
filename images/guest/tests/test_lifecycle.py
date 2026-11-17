import importlib
import json
import re
import subprocess
import tempfile
import unittest
import uuid
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from guest.offline import stage_bundle
from guest.tests.fixtures import encoded
from guest.tests.test_offline import native_inputs, SYNTHETIC_PROFILE
from guest.lock import VERSIONS

ORKA_READYZ = '--raw=/api/v1/namespaces/orka-system/services/http:orka-api:api/proxy/readyz'


class LifecycleTests(unittest.TestCase):
    def setUp(self):
        policy = patch.dict(VERSIONS, {'sourceOfflineProfile': SYNTHETIC_PROFILE})
        policy.start()
        self.addCleanup(policy.stop)
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
        self.node_started = '2026-11-05T00:20:00.123456789Z'
        self.next_start = None
        self.selector_extra = {}
        self.pod_time = '2026-11-05T00:20:30Z'
        self.orka_answer = '{"status":"ok","checks":{"store":"ok","kubernetes":"ok"}}'
        self.now = 0.0
        self.sleeps = []
        self.on_sleep = None
        self.pod_overrides = {}
        self.replicas: dict[tuple[str, str], int | None] = {('kube-system', 'coredns'): 2, ('orka-system', 'orka-api-controller'): 1,
                         ('orka-system', 'orka-api-workspace-publisher'): 1, ('orka-system', 'orka-api-scm-egress-proxy'): 1}
        try:
            self.api = importlib.import_module('guest.lifecycle')
        except ModuleNotFoundError:
            self.fail('guest lifecycle missing')
        self.addCleanup(patch.stopall)
        patch.object(self.api, 'BUNDLE', self.bundle).start()
        patch.object(self.api, 'execute', self.execute).start()
        patch.object(self.api, 'time', SimpleNamespace(monotonic=lambda: self.now, sleep=self.sleep)).start()

    def sleep(self, seconds):
        self.sleeps.append(json.loads((self.state / 'bootstrap.json').read_bytes())['phase'])
        self.now += seconds
        if self.on_sleep:
            self.on_sleep(len(self.sleeps))

    def pod(self, labels=None, *, at=None, ready=True, change=None):
        at = at or self.pod_time
        pod = {'kind': 'Pod', 'metadata': {'name': 'pod', 'labels': {**(labels or {}), 'pod-template-hash': 'x'}},
               'status': {'conditions': [{'type': 'Ready', 'status': 'True' if ready else 'False', 'lastTransitionTime': at}],
                          'containerStatuses': [{'ready': ready, 'state': {'running': {'startedAt': at}}}]}}
        if change:
            change(pod)
        return pod

    def workload(self, argv):
        namespace = argv[argv.index('-n') + 1]
        if 'deployment' in argv:
            name = argv[argv.index('deployment') + 1]
            if (namespace, name) not in self.replicas:
                raise ValueError('command failed: kubectl')
            spec = {'selector': {'matchLabels': {'tier': 'x', 'app': name}, **self.selector_extra.get(name, {})}}
            if self.replicas[(namespace, name)] is not None:
                spec['replicas'] = self.replicas[(namespace, name)]
            return json.dumps({'kind': 'Deployment', 'metadata': {'name': name}, 'spec': spec})
        selector = dict(term.split('=') for term in argv[argv.index('-l') + 1].split(','))
        name = selector['app']
        # Overrides apply to the first replica only; an omitted replica count defaults to one.
        pods = [self.pod(selector, **(self.pod_overrides.get(name, {}) if i == 0 else {})) for i in range(self.replicas[(namespace, name)] or 1)]
        return json.dumps({'kind': 'List', 'items': pods})

    def execute(self, argv, *, env=None, stdin=None, timeout=None):
        self.calls.append((argv, env))
        if self.fail_command and self.fail_command in argv:
            raise ValueError('injected phase failure')
        if argv[:2] == ['docker', 'ps']:
            return self.node_id if self.has_node else ''
        if argv[:2] == ['docker', 'inspect']:
            return json.dumps([{'Id': self.node_id, 'Image': self.manifest['node']['configDigest'], 'Config': {'Labels': {'io.x-k8s.kind.cluster': 'bridge-' + self.installation.replace('-', '')}}, 'State': {'Running': self.running, 'StartedAt': self.node_started}}])
        if argv[:2] == ['docker', 'start']:
            self.running = True
            self.node_started = self.next_start or self.node_started
        if argv[0] == 'kmx' and 'up' in argv:
            self.has_node = True
            if self.create_then_fail:
                raise ValueError('partial create retained')
        if 'inspecti' in argv:
            reference = argv[-1]
            row = next(r for r in self.manifest['images'] if reference == r['repository'] + '@' + r['manifestDigest'])
            return json.dumps({'status': {'id': row['configDigest'], 'repoDigests': [reference]}})
        if argv[0] == 'kubectl' and ('deployment' in argv or 'pods' in argv):
            return self.workload(argv)
        if ORKA_READYZ in argv:
            if isinstance(self.orka_answer, Exception):
                raise self.orka_answer
            return self.orka_answer
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
        kube = ['kubectl', '--context', context, '--request-timeout=10s']
        self.assertIn([*kube, 'get', ORKA_READYZ], commands)
        # Each Deployment's own pods, never a namespace-wide listing.
        self.assertIn([*kube, '-n', 'orka-system', 'get', 'pods', '-l', 'app=orka-api-controller,tier=x', '-o', 'json'], commands)
        self.assertFalse(any('get' in args and 'pods' in args and '-l' not in args for args in commands))
        self.assertEqual([], self.sleeps)

    def restart(self, *, bootstrap=True):
        # The VM restarts: the stopped node's StartedAt changes only when it starts again,
        # status from the earlier run is stale and Orka's Service refuses.
        if bootstrap:
            self.api.bootstrap(self.lock, self.state)
        self.calls.clear()
        self.node_started = '2026-11-05T00:20:00.123456789Z'
        self.running, self.next_start = False, '2026-11-05T00:40:00.5Z'
        self.orka_answer = ValueError('command failed: kubectl')

    def test_restart_waits_until_orka_serves_on_status_since_node_start(self):
        self.restart()
        # Orka already answers: only pod status from before the node started holds the marker back.
        self.orka_answer = '{"status":"ok","checks":{"store":"ok","kubernetes":"ok"}}'
        reasons = []
        original = self.api.not_serving
        def observed(*args):
            reasons.append(original(*args))
            return reasons[-1]
        def kubelet(count):
            if count == 1:
                self.pod_time = '2026-11-05T00:40:20Z'
                self.orka_answer = ValueError('command failed: kubectl')
            if count == 3:
                self.orka_answer = '{"status":"ok","checks":{"store":"ok","kubernetes":"ok"}}'
        self.on_sleep = kubelet
        with patch.object(self.api, 'not_serving', observed):
            self.assertEqual('infrastructure-ready', self.api.resume(self.lock, self.state)['phase'])
        self.assertEqual(['orka-ready'] * 3, self.sleeps)
        self.assertEqual(['coredns not ready', 'Orka API: command failed: kubectl', 'Orka API: command failed: kubectl', None], reasons)
        self.assertIn(['docker', 'start', self.node_id], [args for args, _ in self.calls])

    def test_refusal_names_the_last_reason(self):
        self.restart()
        def kubelet(count):
            if count == 1:
                self.pod_time = '2026-11-05T00:40:20Z'
        self.on_sleep = kubelet
        with self.assertRaisesRegex(ValueError, 'not serving since node start: Orka API: command failed: kubectl$'):
            self.api.resume(self.lock, self.state)

    def test_restart_refuses_within_bound_unless_orka_serves_since_node_start(self):
        fresh, stale, ok = '2026-11-05T00:40:20Z', '2026-11-05T00:20:30Z', '{"status":"ok"}'
        def running(value):
            return lambda pod: pod['status']['containerStatuses'][0]['state'].update(running=value)
        cases = {
            'stale status with an answer': ({}, ok, stale, 'coredns not ready'),
            'Orka Service refused': ({}, ValueError('command failed: kubectl'), fresh, 'Orka API: command failed: kubectl'),
            'Orka Service read timed out': ({}, subprocess.TimeoutExpired(['kubectl'], 30), fresh, 'Orka API: TimeoutExpired'),
            'another server answered': ({}, 'ok\n', fresh, 'Orka API: JSONDecodeError'),
            'Orka not ready': ({}, '{"status":"not ready","checks":{"store":"unhealthy"}}', fresh, 'Orka API not ready'),
            'duplicate status keys': ({}, '{"status":"not ready","status":"ok"}', fresh, 'Orka API: duplicate JSON key'),
            'deeply nested answer': ({}, '[' * 100000 + ']' * 100000, fresh, 'Orka API: JSON nesting exceeds decoder limit'),
            'publisher status stale': ({'orka-api-workspace-publisher': {'at': stale}}, ok, fresh, 'orka-api-workspace-publisher not ready'),
            'SCM egress proxy status stale': ({'orka-api-scm-egress-proxy': {'at': stale}}, ok, fresh, 'orka-api-scm-egress-proxy not ready'),
            'one CoreDNS replica not ready': ({'coredns': {'ready': False}}, ok, fresh, 'coredns not ready'),
            'controller ready in the node start second': ({'orka-api-controller': {'at': '2026-11-05T00:40:00Z'}}, ok, fresh, 'orka-api-controller not ready'),
            'malformed start time': ({'orka-api-controller': {'change': running({'startedAt': 'yesterday'})}}, ok, fresh, 'orka-api-controller: invalid timestamp'),
            'out-of-range start time': ({'orka-api-controller': {'change': running({'startedAt': '2026-13-05T00:40:20Z'})}}, ok, fresh, 'orka-api-controller: invalid timestamp'),
            'unexpected status shape': ({'orka-api-controller': {'change': running(None)}}, ok, fresh, 'orka-api-controller: AttributeError'),
        }
        self.api.bootstrap(self.lock, self.state)
        for name, (overrides, answer, pod_time, reason) in cases.items():
            with self.subTest(name):
                self.restart(bootstrap=False)
                self.now, self.sleeps[:] = 0.0, []
                self.pod_overrides, self.orka_answer, self.pod_time = overrides, answer, pod_time
                with self.assertRaisesRegex(ValueError, 'not serving since node start: ' + re.escape(reason) + '$'):
                    self.api.resume(self.lock, self.state)
                self.assertGreaterEqual(self.now, self.api.SERVING_SECONDS)
                self.assertLessEqual(self.now, self.api.SERVING_SECONDS + 2)
                self.assertEqual({'orka-ready'}, set(self.sleeps))
                self.assertEqual('orka-ready', json.loads((self.state / 'bootstrap.json').read_bytes())['phase'])

    def test_missing_or_unsupported_workload_is_not_serving(self):
        self.restart()
        self.pod_time, self.orka_answer = '2026-11-05T00:40:20Z', '{"status":"ok"}'
        self.selector_extra['orka-api-controller'] = {'matchExpressions': [{'key': 'app', 'operator': 'Exists'}]}
        with self.assertRaisesRegex(ValueError, 'not serving since node start: orka-api-controller: unsupported workload selector$'):
            self.api.resume(self.lock, self.state)
        self.selector_extra.clear()
        del self.replicas[('orka-system', 'orka-api-workspace-publisher')]
        self.now = 0.0
        with self.assertRaisesRegex(ValueError, 'not serving since node start: orka-api-workspace-publisher: command failed: kubectl$'):
            self.api.resume(self.lock, self.state)
        self.assertEqual('orka-ready', json.loads((self.state / 'bootstrap.json').read_bytes())['phase'])

    def test_omitted_replica_count_needs_one_ready_pod(self):
        self.replicas[('kube-system', 'coredns')] = None
        self.assertEqual('infrastructure-ready', self.api.bootstrap(self.lock, self.state)['phase'])
        self.assertEqual([], self.sleeps)
        self.pod_overrides = {'coredns': {'ready': False}}
        with self.assertRaisesRegex(ValueError, 'not serving since node start: coredns not ready$'):
            self.api.resume(self.lock, self.state)

    def test_ready_counts_only_status_reported_after_the_node_start_second(self):
        since = self.api.seconds('2026-11-05T00:40:00.5Z')
        fresh = '2026-11-05T00:40:01Z'
        def variant(change):
            pod = self.pod(at=fresh)
            change(pod)
            return pod
        self.assertTrue(self.api.ready_since(self.pod(at=fresh), since))
        for name, pod in {
            'stale': self.pod(at='2026-11-05T00:20:30Z'),
            'same second as node start': self.pod(at='2026-11-05T00:40:00Z'),
            'not ready': self.pod(at=fresh, ready=False),
            'stale condition': variant(lambda p: p['status']['conditions'][0].update(lastTransitionTime='2026-11-05T00:20:30Z')),
            'container started before node': variant(lambda p: p['status']['containerStatuses'][0]['state']['running'].update(startedAt='2026-11-05T00:20:30Z')),
            'container waiting': variant(lambda p: p['status']['containerStatuses'][0].update(state={'waiting': {'reason': 'CrashLoopBackOff'}})),
            'container not ready': variant(lambda p: p['status']['containerStatuses'][0].update(ready=False)),
            'second container stale': variant(lambda p: p['status']['containerStatuses'].append({'ready': True, 'state': {'running': {'startedAt': '2026-11-05T00:20:30Z'}}})),
            'no containers': variant(lambda p: p['status'].update(containerStatuses=[])),
            'terminating': variant(lambda p: p['metadata'].update(deletionTimestamp=fresh)),
            'no status': variant(lambda p: p.pop('status')),
        }.items():
            with self.subTest(name):
                self.assertFalse(self.api.ready_since(pod, since))
        for value in ('2026-11-05 00:40:01Z', '2026-11-05T00:40:01+00:00', None):
            with self.subTest(value), self.assertRaises(ValueError):
                self.api.seconds(value)

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


class ExecuteTests(unittest.TestCase):
    def setUp(self):
        self.api = importlib.import_module('guest.lifecycle')
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.bin = Path(self.temp.name)

    def tool(self, name, script):
        path = self.bin / name
        path.write_text('#!/bin/sh\n' + script)
        path.chmod(0o700)
        return str(path)

    def test_kmx_refusal_keeps_its_closing_lines_without_escapes_or_box(self):
        kmx = self.tool('kmx', "printf '\\033[33m\\342\\225\\255\\342\\224\\200\\033[m\\r\\n\\033[33m\\342\\224\\202\\033[m   context  kind-secret-box \\033[33m\\342\\224\\202\\033[m\\r\\n"
                                 "\\033[33m\\342\\225\\260\\342\\224\\200\\033[m\\r\\nstarted\\nPHASE  [1/1] Install\\n\\noffline live workload set differs; refusing reinstall\\a\\n' >&2; exit 1\n")
        with self.assertRaises(ValueError) as raised:
            self.api.execute([kmx, 'cluster', 'orka', 'install'])
        self.assertEqual('command failed: kmx: started | PHASE  [1/1] Install | offline live workload set differs; refusing reinstall', str(raised.exception))

    def test_kmx_refusal_keeps_early_cause_before_long_stack_footer(self):
        kmx = self.tool('kmx', "printf 'PHASE Prepare kind cluster\\nERROR: synthetic early cause\\n' >&2; "
                                 "i=0; while [ \"$i\" -lt 100 ]; do printf 'synthetic stack frame %s\\n' \"$i\" >&2; i=$((i+1)); done; "
                                 "printf 'runtime/asm_arm64.s:1268\\nFAILED Prepare kind cluster\\nkind create: exit status 1\\n' >&2; exit 1\n")
        with self.assertRaises(ValueError) as raised:
            self.api.execute([kmx, 'cluster', 'up'])
        message = str(raised.exception)
        self.assertIn('ERROR: synthetic early cause', message)
        self.assertIn(' | ... | ', message)
        self.assertTrue(message.endswith('kind create: exit status 1'))
        self.assertNotIn('synthetic stack frame 70', message)

    def test_kmx_short_multiline_refusal_keeps_leading_context_once(self):
        kmx = self.tool('kmx', "printf 'early context\\nsecond context\\nthird context\\nfinal reason\\n' >&2; exit 1\n")
        with self.assertRaises(ValueError) as raised:
            self.api.execute([kmx])
        self.assertEqual('command failed: kmx: early context | second context | third context | final reason', str(raised.exception))

    def test_kmx_refusal_is_bounded(self):
        kmx = self.tool('kmx', "yes x | head -c 100000 | tr -d '\\n' >&2; echo >&2; echo final reason >&2; exit 1\n")
        with self.assertRaises(ValueError) as raised:
            self.api.execute([kmx])
        message = str(raised.exception)
        self.assertTrue(message.startswith('command failed: kmx: '))
        self.assertTrue(message.endswith(' | final reason'))
        self.assertIn(' | ... | ', message)
        self.assertLessEqual(len(message), len('command failed: kmx: ') + 8192)

    def test_other_commands_keep_only_fixed_text(self):
        kubectl = self.tool('kubectl', "echo secret-child-detail >&2; exit 1\n")
        with self.assertRaises(ValueError) as raised:
            self.api.execute([kubectl, 'get', 'nodes'])
        self.assertEqual('command failed: kubectl', str(raised.exception))

    def test_silent_kmx_failure_keeps_fixed_text(self):
        kmx = self.tool('kmx', "exit 3\n")
        with self.assertRaises(ValueError) as raised:
            self.api.execute([kmx])
        self.assertEqual('command failed: kmx', str(raised.exception))
