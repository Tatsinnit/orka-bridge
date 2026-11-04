"""Stage the native closure using kmx's original, hash-selected install manifest."""
import gzip
import hashlib
import io
import json
import shutil
import tarfile
import tempfile
from typing import TYPE_CHECKING

if __package__ or TYPE_CHECKING:
    from .cache import encoded, inspect_archive, write_archive
    from .lock import checked_file, file_hash, read_json, relative_path, require
else:
    from cache import encoded, inspect_archive, write_archive
    from lock import checked_file, file_hash, read_json, relative_path, require

NATIVE = {'kind-node', 'controller', 'ai-worker', 'general-worker', 'publisher', 'session-loader'}


def member(archive, name):
    stream = archive.extractfile(name)
    assert stream is not None  # inspect_archive already established regular-file membership.
    return stream


def docker_archive(component, source, output, reference):
    """Docker's classic store needs Docker-save, not an OCI-layout tar."""
    with tarfile.open(source / component['archive']) as incoming, tarfile.open(output, 'w') as outgoing:
        manifest_bytes = member(incoming, 'blobs/sha256/' + component['manifestDigest'][7:]).read()
        require('sha256:' + hashlib.sha256(manifest_bytes).hexdigest() == component['manifestDigest'], 'node manifest identity mismatch')
        manifest = read_json(manifest_bytes)
        config_bytes = member(incoming, 'blobs/sha256/' + component['configDigest'][7:]).read()
        require('sha256:' + hashlib.sha256(config_bytes).hexdigest() == component['configDigest'], 'node config identity mismatch')
        config = read_json(config_bytes)
        layers = []
        for index, layer in enumerate(manifest['layers']):
            with member(incoming, 'blobs/sha256/' + layer['digest'][7:]) as raw, tempfile.TemporaryFile() as spool:
                require(layer['mediaType'] in ('application/vnd.oci.image.layer.v1.tar', 'application/vnd.oci.image.layer.v1.tar+gzip', 'application/vnd.docker.image.rootfs.diff.tar.gzip'), 'unsupported outer node layer')
                stream = gzip.GzipFile(fileobj=raw) if layer['mediaType'].endswith(('gzip', '+gzip')) else raw
                digest = hashlib.sha256()
                while chunk := stream.read(1024 * 1024):
                    digest.update(chunk)
                    spool.write(chunk)
                require('sha256:' + digest.hexdigest() == config['rootfs']['diff_ids'][index], 'node layer diff identity mismatch')
                name = 'layers/' + str(index) + '.tar'
                entry = tarfile.TarInfo(name)
                entry.size = spool.tell()
                spool.seek(0)
                outgoing.addfile(entry, spool)
                layers.append(name)
        require(len(layers) == len(config['rootfs']['diff_ids']), 'node layer count mismatch')
        for name, data in ((component['configDigest'][7:] + '.json', config_bytes), ('manifest.json', encoded([{'Config': component['configDigest'][7:] + '.json', 'RepoTags': [reference], 'Layers': layers}]))):
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            outgoing.addfile(entry, io.BytesIO(data))


def stage_bundle(lock, *, source, out):
    tools = {t['name']: t for t in lock['tools']}
    paths = {name: checked_file(source, tools[name]['path'], tools[name]['sha256']) for name in ('kmx', 'kind', 'helm', 'kubectl', 'kmx-offline-manifest', 'orka-images')}
    original = paths['kmx-offline-manifest'].read_bytes()
    manifest = read_json(original)
    require(manifest['schemaVersion'] == 'kmx-offline/v1' and manifest['profileSha256'] == lock['kmxProfileSha256'], 'offline profile identity mismatch')
    require(manifest['platform'] == {'os': 'linux', 'architecture': lock['architecture']} and manifest['orka'] == {'mode': lock['orka']['mode'], 'identity': lock['orka']['identity']}, 'offline source identity mismatch')
    require(manifest['node']['localRef'] == lock['nodeLocalRef'], 'node tag identity mismatch')
    require(manifest['kmx']['sha256'] == tools['kmx']['sha256'] and manifest['kmx']['version'] == tools['kmx']['version'], 'kmx identity mismatch')
    require(manifest['chart']['sha256'] == lock['orka']['chartSha256'] and manifest['artifacts']['sha256'] == tools['orka-images']['sha256'], 'chart identity mismatch')
    components = {c['name']: c for c in lock['components'] if c['name'] in NATIVE}
    rows = [('kind-node', manifest['node'])] + [(r['component'], r) for r in manifest['images']]
    require(len(rows) == len(NATIVE) and {n for n, _ in rows} == NATIVE, 'native closure identity mismatch')
    for name, row in rows:
        repository = 'docker.io/kindest/node' if name == 'kind-node' and row['repository'] == 'kindest/node' else row['repository']
        require(repository == components[name]['repository'] and all(row[k] == components[name][k] for k in ('manifestDigest', 'configDigest', 'indexDigest')), 'native image identity mismatch')
        inspect_archive(components[name], source, lock['architecture'])
    destinations = ['install.json', 'inputs.json', 'node.docker.tar', manifest['chart']['path'], manifest['artifacts']['path'], *['tools/' + name for name in ('kmx', 'kind', 'helm', 'kubectl')]]
    for name, row in rows:
        destinations += ['components/' + name + '.oci.tar', *[row[key] for key in ('manifestPath', 'configPath', 'indexPath')]]
    for name in destinations:
        relative_path(name)
    require(len(destinations) == len(set(destinations)), 'bundle destination collision')
    require(not out.exists(), 'bundle output already exists')
    out.mkdir(parents=True)
    (out / 'components').mkdir()
    hashes = {}

    def copy(path, name, expected):
        relative_path(name)
        target = out / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)
        require(file_hash(target) == expected, 'bundle input changed during copy')
        hashes[name] = file_hash(target)

    runtime = json.loads(encoded(lock))
    selected = {c['name']: c for c in runtime['components']}
    for name, row in rows:
        component = components[name]
        dest = 'components/' + name + '.oci.tar'
        reference = lock['nodeLocalRef'] if name == 'kind-node' else row['repository'] + '@' + row['manifestDigest']
        write_archive(component, inspect_archive(component, source, lock['architecture']), source, out / dest, reference)
        hashes[dest] = file_hash(out / dest)
        selected[name].update(archive=dest, archiveSha256=hashes[dest])
        with tarfile.open(source / component['archive']) as archive:
            for field, digest in (('manifestPath', component['manifestDigest']), ('configPath', component['configDigest'])):
                relative_path(row[field])
                target = out / row[field]
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(member(archive, 'blobs/sha256/' + digest[7:]).read())
                require(file_hash(target) == digest[7:], 'metadata identity mismatch')
                hashes[row[field]] = digest[7:]
        copy(checked_file(source, component['indexPath'], component['indexDigest'][7:]), row['indexPath'], component['indexDigest'][7:])
        selected[name]['indexPath'] = row['indexPath']
    copy(checked_file(source, lock['orka']['chartPath'], lock['orka']['chartSha256']), manifest['chart']['path'], lock['orka']['chartSha256'])
    copy(paths['orka-images'], manifest['artifacts']['path'], tools['orka-images']['sha256'])
    copy(paths['kmx-offline-manifest'], 'install.json', tools['kmx-offline-manifest']['sha256'])
    for name in ('kmx', 'kind', 'helm', 'kubectl'):
        dest = 'tools/' + name
        copy(paths[name], dest, tools[name]['sha256'])
        require(hashes[dest] == tools[name]['sha256'], 'tool changed during staging')
        (out / dest).chmod(0o755)
        next(t for t in runtime['tools'] if t['name'] == name)['path'] = dest
    docker_archive(components['kind-node'], source, out / 'node.docker.tar', lock['nodeLocalRef'])
    hashes['node.docker.tar'] = file_hash(out / 'node.docker.tar')
    runtime['tools'].append({'name': 'kind-node-docker', 'version': components['kind-node']['manifestDigest'], 'path': 'node.docker.tar', 'sha256': hashes['node.docker.tar']})
    runtime['orka']['chartPath'] = manifest['chart']['path']
    next(t for t in runtime['tools'] if t['name'] == 'kmx-offline-manifest')['path'] = 'install.json'
    next(t for t in runtime['tools'] if t['name'] == 'orka-images')['path'] = manifest['artifacts']['path']
    for component in runtime['components']:
        if component['name'] in NATIVE:
            inspect_archive(component, out, lock['architecture'])
    (out / 'inputs.json').write_bytes(encoded(runtime) + b'\n')
    hashes['inputs.json'] = file_hash(out / 'inputs.json')
    return hashes
