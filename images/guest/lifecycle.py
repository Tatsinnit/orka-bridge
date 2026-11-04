"""Create or resume this guest's offline infrastructure through kmx only."""
import argparse
import fcntl
import json
import os
import subprocess
import sys
import uuid
from contextlib import contextmanager
from pathlib import Path
from typing import TYPE_CHECKING

if __package__ or TYPE_CHECKING:
    from .cache import encoded
    from .lock import (
        checked_file,
        input_identity,
        load_lock,
        matches,
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
        read_json,
        require,
        validate_lock,
    )

BUNDLE = Path('/opt/bridge/bundle')
PHASES = ('identity-prepared', 'cluster-ready', 'cache-verified', 'orka-ready', 'infrastructure-ready')


def execute(argv, *, env=None, stdin=None):
    with open(stdin, 'rb') if stdin is not None else open(os.devnull, 'rb') as stream:
        result = subprocess.run(argv, env=env, stdin=stream, capture_output=True, timeout=600, check=False)
    require(result.returncode == 0, 'command failed: ' + Path(argv[0]).name)
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
    execute([*kmx, 'up', '--step', 'cluster', '--offline-bundle', str(BUNDLE)], env=env)
    nodes = node_inventory(env, manifest)
    require(len(nodes) == 1 and nodes[0]['State']['Running'], 'cluster node unavailable')
    require(not journal['nodeIds'] or journal['nodeIds'] == [nodes[0]['Id']], 'node identity changed')
    journal.update(nodeIds=[nodes[0]['Id']], phase='cluster-ready')
    save(state, journal)
    (state / 'cluster-create.intent').unlink(missing_ok=True)
    for row in manifest['images']:
        node = journal['nodeIds'][0]
        component = components[row['component']]
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
