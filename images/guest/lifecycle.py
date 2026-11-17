"""Create or resume this guest's offline infrastructure through kmx only."""
import argparse
import fcntl
import json
import os
import re
import subprocess
import sys
import time
import uuid
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import TYPE_CHECKING

if __package__ or TYPE_CHECKING:
    from .cache import encoded
    from .lock import (
        checked_file,
        input_identity,
        load_lock,
        matches,
        model_images,
        read_json,
        require,
        validate_lock,
    )
else:
    from cache import encoded
    from lock import (
        checked_file,
        input_identity,
        load_lock,
        matches,
        model_images,
        read_json,
        require,
        validate_lock,
    )

BUNDLE = Path('/opt/bridge/bundle')
PHASES = ('identity-prepared', 'cluster-ready', 'cache-verified', 'orka-ready', 'infrastructure-ready')
# A controller that starts before Service routing works crashes; kubelet back-off doubles from 10 s.
# Leave room for several such restarts on a slow host; the unit's 900 s start timeout still bounds the run.
SERVING_SECONDS = 600
# CoreDNS and the only Orka workloads kmx's offline profile renders.
WORKLOADS = (('kube-system', 'coredns'), ('orka-system', 'orka-api-controller'), ('orka-system', 'orka-api-workspace-publisher'), ('orka-system', 'orka-api-scm-egress-proxy'))
ORKA_READYZ = '/api/v1/namespaces/orka-system/services/http:orka-api:api/proxy/readyz'


def refusal(stderr):
    """Bounded kmx context and closing lines, without terminal escapes or its confirmation box."""
    if len(stderr) > 64 * 1024:
        stderr = stderr[:48 * 1024] + b'\n... stderr omitted ...\n' + stderr[-16 * 1024:]
    text = re.sub(r'\x1b\[[0-9;?]*[A-Za-z]', '', stderr.decode(errors='replace'))
    lines = [''.join(c for c in line if c.isprintable()).strip() for line in text.splitlines()]
    lines = [line for line in lines if line and line[0] not in '\u2502\u256d\u2570\u250c\u2514\u2500']
    joined = ' | '.join(lines)
    if len(lines) <= 43 and len(joined) <= 8192:
        return joined
    # A kind/kubeadm stack can bury the cause far above kmx's closing summary.
    return ' | '.join(lines[:40])[:6144] + ' | ... | ' + ' | '.join(lines[-3:])[-2039:]


def execute(argv, *, env=None, stdin=None, timeout=600):
    with open(stdin, 'rb') if stdin is not None else open(os.devnull, 'rb') as stream:
        result = subprocess.run(argv, env=env, stdin=stream, capture_output=True, timeout=timeout, check=False)
    if result.returncode != 0:
        name = Path(argv[0]).name
        # kmx states why it refused; other children's raw output stays out of the journal.
        reason = refusal(result.stderr) if name == 'kmx' else ''
        raise ValueError('command failed: ' + name + (': ' + reason if reason else ''))
    require(len(result.stdout) <= 1024 * 1024, 'command output exceeds bound')
    return result.stdout.decode()


@contextmanager
def operation_lock(state):
    fd = os.open(state / 'bootstrap.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError('bootstrap busy') from error
        yield
    finally:
        os.close(fd)


def save(state, journal):
    path = state / 'bootstrap.json'
    temporary = state / 'bootstrap.json.partial'
    fd = os.open(temporary, os.O_CREAT | os.O_TRUNC | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(encoded(journal) + b'\n')
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)
    directory = os.open(state, os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def environment(state, installation):
    cluster = 'bridge-' + uuid.UUID(installation).hex
    return {'PATH': '/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin', 'HOME': '/root', 'LANG': 'C.UTF-8',
            'KIND_CLUSTER': cluster, 'CONTAINER_ENGINE': 'docker', 'KIND_EXPERIMENTAL_PROVIDER': 'docker',
            'KMX_HOME': str(state / 'kmx'), 'KUBECONFIG': str(state / 'kubeconfig'),
            'DOCKER_HOST': 'unix:///var/run/docker.sock'}


def node_inventory(env, manifest):
    ids = execute(['docker', 'ps', '-a', '--no-trunc', '--filter', 'label=io.x-k8s.kind.cluster=' + env['KIND_CLUSTER'], '--format', '{{.ID}}'], env=env).splitlines()
    require(len(ids) <= 1 and all(matches(id_, r'[0-9a-f]{64}') for id_ in ids), 'unexpected node inventory')
    if not ids:
        return []
    nodes = json.loads(execute(['docker', 'inspect', *ids], env=env))
    require(len(nodes) == len(ids), 'node inspection incomplete')
    for node in nodes:
        require(node['Id'] in ids and node['Image'] == manifest['node']['configDigest'] and node['Config']['Labels']['io.x-k8s.kind.cluster'] == env['KIND_CLUSTER'], 'node identity mismatch')
    return nodes


def seconds(value):
    """Whole seconds of a Docker or Kubernetes UTC timestamp."""
    require(matches(value, r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{1,9})?Z'), 'invalid timestamp')
    try:
        return int(datetime.strptime(value[:19], '%Y-%m-%dT%H:%M:%S').replace(tzinfo=timezone.utc).timestamp())
    except ValueError as error:
        raise ValueError('invalid timestamp') from error


def ready_since(pod, since):
    """Whether the kubelet reported this pod and every container ready after the `since` second."""
    status = pod.get('status') or {}
    conditions = [c for c in status.get('conditions') or [] if c.get('type') == 'Ready']
    containers = status.get('containerStatuses') or []
    if pod['metadata'].get('deletionTimestamp') or len(conditions) != 1 or conditions[0].get('status') != 'True' or not containers:
        return False
    started = [(c.get('state') or {}).get('running', {}).get('startedAt') for c in containers]
    return (seconds(conditions[0].get('lastTransitionTime')) > since and all(c.get('ready') is True for c in containers)
            and all(s is not None and seconds(s) > since for s in started))


def not_serving(kubectl, env, since):
    """Why CoreDNS or Orka is not yet serving since the node started, or None once it is."""
    step = 'Orka API'
    try:
        for namespace, step in WORKLOADS:
            spec = read_json(execute([*kubectl, '-n', namespace, 'get', 'deployment', step, '-o', 'json'], env=env, timeout=30))['spec']
            selector = spec['selector'].get('matchLabels') or {}
            require(selector and not spec['selector'].get('matchExpressions'), 'unsupported workload selector')
            labels = ','.join(key + '=' + value for key, value in sorted(selector.items()))
            pods = read_json(execute([*kubectl, '-n', namespace, 'get', 'pods', '-l', labels, '-o', 'json'], env=env, timeout=30))['items']
            if sum(ready_since(pod, since) for pod in pods) < max(spec.get('replicas', 1), 1):
                return step + ' not ready'
        step = 'Orka API'
        answer = read_json(execute([*kubectl, 'get', '--raw=' + ORKA_READYZ], env=env, timeout=30))
    except (ValueError, LookupError, TypeError, AttributeError, subprocess.SubprocessError) as error:
        # A failed or unexpected read is not serving yet; keep only fixed guard text or the type.
        return step + ': ' + (str(error) if type(error) is ValueError else type(error).__name__)
    # Only Orka's API answers this JSON; a stale endpoint can reach another :8080 server.
    return None if isinstance(answer, dict) and answer.get('status') == 'ok' else 'Orka API not ready'


def wait_serving(kubectl, env, since):
    # Status written before the node last started is stale after a restart, as is kmx's rollout check.
    deadline = time.monotonic() + SERVING_SECONDS
    while (reason := not_serving(kubectl, env, since)) is not None:
        require(time.monotonic() < deadline, 'infrastructure not serving since node start: ' + reason)
        time.sleep(2)


def verified_inputs(lock):
    validate_lock(lock, production=False)
    tools = {t['name']: t for t in lock['tools']}
    for name in ('kind', 'kmx', 'kubectl', 'helm', 'kind-node-docker', 'kmx-offline-manifest', 'orka-images'):
        checked_file(BUNDLE, tools[name]['path'], tools[name]['sha256'])
    manifest = read_json((BUNDLE / tools['kmx-offline-manifest']['path']).read_bytes())
    require(manifest['profileSha256'] == lock['kmxProfileSha256'] and manifest['kmx']['sha256'] == tools['kmx']['sha256'], 'bundle identity mismatch')
    components = {c['name']: c for c in lock['components']}
    for name, row in [('kind-node', manifest['node'])] + [(r['component'], r) for r in manifest['images']]:
        component = components[name]
        checked_file(BUNDLE, component['archive'], component['archiveSha256'])
        repository = 'docker.io/kindest/node' if name == 'kind-node' and row['repository'] == 'kindest/node' else row['repository']
        require(repository == component['repository'] and all(row[k] == component[k] for k in ('manifestDigest', 'configDigest')), 'native image identity mismatch')
        for path, digest in ((row['manifestPath'], row['manifestDigest']), (row['configPath'], row['configDigest']), (row['indexPath'], row['indexDigest'])):
            checked_file(BUNDLE, path, digest[7:])
    for component in model_images(lock):
        checked_file(BUNDLE, component['archive'], component['archiveSha256'])
        if 'indexPath' in component:
            checked_file(BUNDLE, component['indexPath'], component['indexDigest'][7:])
    checked_file(BUNDLE, manifest['chart']['path'], manifest['chart']['sha256'])
    return manifest, tools, components


def bootstrap(lock: dict, state: Path) -> dict:
    with operation_lock(state):
        return initialize(lock, state, require_existing=False)


def resume(lock: dict, state: Path) -> dict:
    with operation_lock(state):
        return initialize(lock, state, require_existing=True)


def initialize(lock, state, *, require_existing):
    manifest, tools, components = verified_inputs(lock)
    identity = read_json((state / 'identity.json').read_bytes())
    require(identity['schemaVersion'] == 'bridge-state/v1', 'invalid state identity')
    installation = str(uuid.UUID(identity['installationId']))
    journal_path = state / 'bootstrap.json'
    require(not journal_path.is_symlink(), 'invalid bootstrap journal')
    expected = {'schemaVersion': 'bridge-infrastructure/v1', 'installationId': installation, 'inputIdentity': input_identity(lock)}
    if journal_path.exists():
        journal = read_json(journal_path.read_bytes())
        require(set(journal) == set(expected) | {'phase', 'nodeIds'} and all(journal[k] == v for k, v in expected.items()), 'invalid bootstrap journal identity')
        require(journal['phase'] in PHASES and isinstance(journal['nodeIds'], list) and all(matches(id_, r'[0-9a-f]{64}') for id_ in journal['nodeIds']), 'invalid bootstrap journal phase')
        require(journal['nodeIds'] or journal['phase'] == 'identity-prepared', 'missing journaled node identity')
    else:
        require(not require_existing, 'resume needs existing bootstrap journal')
        journal = {**expected, 'phase': 'identity-prepared', 'nodeIds': []}
    env = environment(state, installation)
    execute(['/usr/lib/bridge/check-state'], env=env)
    (state / 'kmx').mkdir(mode=0o700, exist_ok=True)
    nodes = node_inventory(env, manifest)
    ids = sorted(n['Id'] for n in nodes)
    if journal['nodeIds']:
        require(ids == sorted(journal['nodeIds']), 'owned node identity missing or replaced')
    else:
        require(not nodes, 'unjournaled cluster retained; manual recovery required')
        require(not (state / 'cluster-create.intent').exists(), 'uncertain cluster creation retained; manual recovery required')
    if not journal_path.exists():
        save(state, journal)
    for node in nodes:
        if not node['State']['Running']:
            execute(['docker', 'start', node['Id']], env=env)
    execute(['docker', 'load', '--input', str(BUNDLE / tools['kind-node-docker']['path'])], env=env)
    context = 'kind-' + env['KIND_CLUSTER']
    kmx = ['kmx', '--context', context, 'cluster']
    if not journal['nodeIds']:
        fd = os.open(state / 'cluster-create.intent', os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
        os.fsync(fd)
        os.close(fd)
        save(state, journal)  # Flush the directory containing the durable creation intent.
    # Select at creation; kmx validates existing mounts on resume, never repairs them.
    mount = ['--credential-mount', 'copilot-v1'] if model_images(lock) else []
    execute([*kmx, 'up', '--step', 'cluster', '--offline-bundle', str(BUNDLE), *mount], env=env)
    nodes = node_inventory(env, manifest)
    require(len(nodes) == 1 and nodes[0]['State']['Running'], 'cluster node unavailable')
    require(not journal['nodeIds'] or journal['nodeIds'] == [nodes[0]['Id']], 'node identity changed')
    journal.update(nodeIds=[nodes[0]['Id']], phase='cluster-ready')
    save(state, journal)
    (state / 'cluster-create.intent').unlink(missing_ok=True)
    images = [(row['component'], row) for row in manifest['images']]
    images += [(c['name'], c) for c in model_images(lock)]
    for name, row in images:
        node = journal['nodeIds'][0]
        component = components[name]
        execute(['docker', 'exec', '-i', node, 'ctr', '--namespace', 'k8s.io', 'images', 'import', '--all-platforms', '--digests', '-'], env=env, stdin=BUNDLE / component['archive'])
        reference = row['repository'] + '@' + row['manifestDigest']
        # The normalized archive must supply this exact reference; no fallback tag.
        status = json.loads(execute(['docker', 'exec', node, 'crictl', 'inspecti', '-o', 'json', reference], env=env))['status']
        require(status['id'] == row['configDigest'] and reference in status['repoDigests'], 'exact CRI image unavailable')
    journal['phase'] = 'cache-verified'
    save(state, journal)
    execute([*kmx, 'orka', 'install', '--provider', '-', '--offline-bundle', str(BUNDLE)], env=env)
    journal['phase'] = 'orka-ready'
    save(state, journal)
    kubectl = ['kubectl', '--context', context]
    execute([*kubectl, 'get', '--raw=/readyz'], env=env)
    execute([*kubectl, '-n', 'kube-system', 'rollout', 'status', 'deployment/coredns', '--timeout=120s'], env=env)
    execute([*kubectl, '-n', 'orka-system', 'wait', 'pvc', '--all', '--for=jsonpath={.status.phase}=Bound', '--timeout=120s'], env=env)
    nodes = node_inventory(env, manifest)
    require([n['Id'] for n in nodes] == journal['nodeIds'] and nodes[0]['State']['Running'], 'cluster node unavailable')
    wait_serving([*kubectl, '--request-timeout=10s'], env, seconds(nodes[0]['State']['StartedAt']))
    journal['phase'] = 'infrastructure-ready'
    save(state, journal)
    return journal


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('bootstrap', 'resume'))
    parser.add_argument('--lock', type=Path, required=True)
    parser.add_argument('--state', type=Path, default=Path('/var/lib/bridge'))
    args = parser.parse_args()
    try:
        result = (bootstrap if args.action == 'bootstrap' else resume)(load_lock(args.lock), args.state)
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        reason = str(error) if type(error) is ValueError else type(error).__name__
        print('guest infrastructure refused: ' + reason + '; retained state', file=sys.stderr)
        return 2
    print(json.dumps({'phase': result['phase'], 'publicationReady': False}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
