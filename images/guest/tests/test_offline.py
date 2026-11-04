import importlib
import json
import tarfile
import tempfile
import unittest
from pathlib import Path

from guest.offline import member
from guest.tests.fixtures import encoded, lock, sha


def native_inputs(root):
    value = lock(root)
    value['kmxProfileSha256'] = 'a' * 64
    for name in ('kmx', 'kubectl', 'helm'):
        (root / name).write_bytes(name.encode())
        value['tools'].append({'name': name, 'path': name, 'version': 'v1', 'sha256': sha(name.encode())})
    (root / 'images.json').write_bytes(b'{}')
    value['tools'].append({'name': 'orka-images', 'path': 'images.json', 'version': 'v1', 'sha256': sha(b'{}')})
    rows = []
    for component in value['components'][:6]:
        name = component['name']
        with tarfile.open(root / component['archive']) as archive:
            index_value = json.load(member(archive, 'index.json'))
            index_value['manifests'][0]['platform'] = {'os': 'linux', 'architecture': 'amd64'}
            index = encoded(index_value)
        (root / (name + '-index.json')).write_bytes(index)
        component.update(indexPath=name + '-index.json', indexDigest='sha256:' + sha(index))
        rows.append({'component': name, 'repository': component['repository'], 'manifestDigest': component['manifestDigest'], 'configDigest': component['configDigest'], 'indexPath': name + '-index.json', 'indexDigest': component['indexDigest'], 'manifestPath': name + '-manifest.json', 'configPath': name + '-config.json', 'pullPolicy': 'IfNotPresent'})
    node = {k: v for k, v in rows.pop(0).items() if k not in ('component', 'pullPolicy')}
    node['localRef'] = value['nodeLocalRef']
    manifest = {'schemaVersion': 'kmx-offline/v1', 'profile': 'native-fixture', 'profileSha256': value['kmxProfileSha256'], 'platform': {'os': 'linux', 'architecture': 'amd64'}, 'kmx': {'version': 'v1', 'revision': '1' * 40, 'sha256': sha(b'kmx')}, 'orka': {'mode': 'source', 'identity': value['sourceCommit']}, 'chart': {'path': 'chart.tgz', 'sha256': value['orka']['chartSha256']}, 'artifacts': {'path': 'images.json', 'sha256': sha(b'{}')}, 'node': node, 'images': rows, 'components': {}}
    (root / 'install.json').write_bytes(encoded(manifest))
    value['tools'].append({'name': 'kmx-offline-manifest', 'path': 'install.json', 'version': 'v1', 'sha256': sha(encoded(manifest))})
    return value, manifest


class OfflineTests(unittest.TestCase):
    def api(self):
        try:
            return importlib.import_module('guest.offline')
        except ModuleNotFoundError:
            self.fail('offline bundle staging missing')

    def test_native_bundle_binds_original_kmx_schema_and_omits_future_fixtures(self):
        api = self.api()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            value, manifest = native_inputs(root)
            out = root / 'out'
            result = api.stage_bundle(value, source=root, out=out)
            self.assertEqual(manifest, json.loads((out / 'install.json').read_bytes()))
            self.assertEqual({'kind-node', 'controller', 'ai-worker', 'general-worker', 'publisher', 'session-loader'}, {p.name.removesuffix('.oci.tar') for p in (out / 'components').iterdir()})
            self.assertFalse((out / 'components/telegram.oci.tar').exists())
            self.assertEqual(sha(b'kmx'), sha((out / 'tools/kmx').read_bytes()))
            with tarfile.open(out / 'node.docker.tar') as archive:
                loaded = json.load(member(archive, 'manifest.json'))
                self.assertEqual([value['nodeLocalRef']], loaded[0]['RepoTags'])
                self.assertEqual('synthetic non-bootable layer', member(archive, loaded[0]['Layers'][0]).read().decode())
            runtime_lock = json.loads((out / 'inputs.json').read_bytes())
            node = next(t for t in runtime_lock['tools'] if t['name'] == 'kind-node-docker')
            self.assertEqual(node['sha256'], sha((out / node['path']).read_bytes()))
            self.assertIn('install.json', result)

    def test_kmx_short_node_repository_maps_to_explicit_docker_registry(self):
        api = self.api()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            value, manifest = native_inputs(root)
            value['components'][0]['repository'] = 'docker.io/kindest/node'
            manifest['node']['repository'] = 'kindest/node'
            (root / 'install.json').write_bytes(encoded(manifest))
            value['tools'][-1]['sha256'] = sha(encoded(manifest))
            api.stage_bundle(value, source=root, out=root / 'out')
            self.assertTrue((root / 'out/inputs.json').is_file())

    def test_bundle_path_collision_is_rejected_before_output(self):
        api = self.api()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            value, manifest = native_inputs(root)
            manifest['chart']['path'] = 'tools/kmx'
            (root / 'install.json').write_bytes(encoded(manifest))
            value['tools'][-1]['sha256'] = sha(encoded(manifest))
            with self.assertRaisesRegex(ValueError, 'bundle destination collision'):
                api.stage_bundle(value, source=root, out=root / 'out')
            self.assertFalse((root / 'out').exists())

    def test_converter_rejects_replaced_config_blob_bytes(self):
        import io
        api = self.api()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            value, _ = native_inputs(root)
            node = value['components'][0]
            path = root / node['archive']
            with tarfile.open(path) as archive:
                entries = [(m.name, member(archive, m.name).read()) for m in archive if m.isfile()]
            with tarfile.open(path, 'w') as archive:
                for name, data in entries:
                    if name == 'blobs/sha256/' + node['configDigest'][7:]:
                        changed = json.loads(data)
                        changed['config'] = {'Labels': {'replaced': 'true'}}
                        data = encoded(changed)
                    entry = tarfile.TarInfo(name)
                    entry.size = len(data)
                    archive.addfile(entry, io.BytesIO(data))
            with self.assertRaisesRegex(ValueError, 'node config identity'):
                api.docker_archive(node, root, root / 'node.docker.tar', value['nodeLocalRef'])

    def test_mismatched_manifest_refuses_before_staging(self):
        api = self.api()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            value, manifest = native_inputs(root)
            manifest['node']['configDigest'] = 'sha256:' + 'b' * 64
            (root / 'install.json').write_bytes(encoded(manifest))
            value['tools'][-1]['sha256'] = sha(encoded(manifest))
            with self.assertRaisesRegex(ValueError, 'identity'):
                api.stage_bundle(value, source=root, out=root / 'out')
            self.assertFalse((root / 'out').exists())
