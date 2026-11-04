"""TEST ONLY: bounded fixture archive and evidence checks, no cluster calls."""
import hashlib
import importlib.util
import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

SOURCE = Path(__file__).parent / "fixtures/task-check.py"


class FixtureTests(unittest.TestCase):
    def api(self):
        self.assertTrue(SOURCE.exists(), "bounded task fixture helper missing")
        spec = importlib.util.spec_from_file_location("task_check", SOURCE)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def archive(self, root, name="model-server", kind=tarfile.REGTYPE):
        path = root / "fixture.tar"
        with tarfile.open(path, "w") as archive:
            item = tarfile.TarInfo(name)
            item.type = kind
            item.size = 0 if kind != tarfile.REGTYPE else 3
            archive.addfile(item, io.BytesIO(b"abc") if item.size else None)
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_workspace_task_uses_custom_locked_image_to_trigger_native_init(self):
        api = self.api()
        rows = api.task_documents("http://fixture.fixture-local.svc:8080", "busybox@sha256:abc")
        self.assertEqual(["ai", "ai", "container"], [r["spec"]["type"] for r in rows])
        self.assertNotIn("sessionRef", rows[0]["spec"])
        self.assertTrue(rows[1]["spec"]["sessionRef"]["create"])
        self.assertEqual("http://fixture.fixture-local.svc:8080/repo/", rows[2]["spec"]["workspace"]["gitRepo"])
        self.assertEqual("main", rows[2]["spec"]["workspace"]["ref"])
        self.assertEqual("busybox@sha256:abc", rows[2]["spec"]["image"])
        for row in rows:
            self.assertEqual(0, row["spec"]["retryPolicy"]["maxRetries"])
            self.assertNotIn("pushBranch", row["spec"].get("workspace", {}))

    def test_evidence_uses_uid_ownership_not_name_prefix(self):
        api = self.api()
        task = {"metadata": {"name": "fixture-ai", "uid": "task-uid"}, "status": {"phase": "Succeeded"}}
        job = {"metadata": {"uid": "job-uid", "ownerReferences": [{"uid": "task-uid", "controller": True}]}}
        pod = {"metadata": {"uid": "pod-uid", "ownerReferences": [{"uid": "job-uid", "controller": True}]},
               "status": {"containerStatuses": [{"name": "worker", "imageID": "sha256:" + "a" * 64,
                           "state": {"terminated": {"exitCode": 0}}}]}}
        result = api.task_evidence(task, [job], [pod], {"sha256:" + "a" * 64}, {})
        self.assertEqual("pod-uid", result["podUID"])
        pod["metadata"]["ownerReferences"][0]["uid"] = "unrelated"
        with self.assertRaises(ValueError):
            api.task_evidence(task, [job], [pod], {"sha256:" + "a" * 64}, {})

    def test_run_submits_once_and_returns_uid_linked_actual_status(self):
        api = self.api()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            api.STATE = root / "reference-fixture"
            bundle = root / "bundle"
            bundle.mkdir()
            rows = [{"component": role, "repository": "example/" + role,
                     "manifestDigest": "sha256:" + char * 64, "configDigest": "sha256:" + char * 64}
                    for role, char in [("session-loader", "a"), ("ai-worker", "b"), ("general-worker", "c")]]
            (bundle / "install.json").write_text(json.dumps({"images": rows}))
            archive = root / "fixture.tar"
            with tarfile.open(archive, "w") as stream:
                for name in ["model-server", "repo/HEAD", "repo/info/refs", "repo/refs/heads/main"]:
                    item = tarfile.TarInfo(name)
                    item.size = 3
                    stream.addfile(item, io.BytesIO(b"abc"))
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            (root / "bootstrap.json").write_text(json.dumps({"phase": "infrastructure-ready", "nodeIds": ["d" * 64]}))
            created = []
            original_read = Path.read_bytes
            original_exists = Path.exists

            def read(path):
                if str(path) == "/run/bridge/reference.json":
                    return json.dumps({"caseId": "offline-tasks", "fixtureSha256": digest}).encode()
                return original_read(path)

            def command(args, **kw):
                if args[:2] == ["docker", "inspect"]:
                    result = [{"Id": "d" * 64, "Config": {"Labels": {"io.x-k8s.kind.cluster": "owned"}}}]
                elif args[0] == "docker":
                    result = {}
                elif "create" in args:
                    doc = json.loads(kw["input"])
                    if doc['kind'] == 'Pod':
                        self.assertEqual('Never', doc['spec']['containers'][0]['imagePullPolicy'])
                        account = doc['spec'].get('serviceAccountName')
                        self.assertTrue(account and any(d['kind'] == 'ServiceAccount' and d['metadata']['name'] == account for d in created), 'fixture must not race default ServiceAccount creation')
                    created.append(doc)
                    result = {**doc, "metadata": {**doc["metadata"], "uid": doc["metadata"]["name"]}}
                else:
                    resource = args[args.index("get") + 1]
                    name = created[-1]["metadata"]["name"]
                    if resource == "pod/fixture":
                        result = {"status": {"conditions": [{"type": "Ready", "status": "True"}]}}
                    elif resource.startswith("tasks."):
                        result = {"metadata": {"name": name, "uid": name}, "status": {"phase": "Succeeded"}}
                    elif resource == "jobs":
                        result = {"items": [{"metadata": {"uid": "job", "ownerReferences": [{"uid": name, "controller": True}]}}]}
                    else:
                        def status(label, char):
                            return {"name": label, "imageID": "sha256:" + char * 64, "state": {"terminated": {"exitCode": 0}}}
                        inits = ([status("fetch-session", "a")] if name == "fixture-session" else
                                 [status("prepare-workspace", "c")] if name == "fixture-workspace" else [])
                        result = {"items": [{"metadata": {"uid": "pod", "ownerReferences": [{"uid": "job", "controller": True}]},
                                             "status": {"containerStatuses": [status("worker", "a" if name == "fixture-workspace" else "b")],
                                                        "initContainerStatuses": inits}}]}
                return SimpleNamespace(returncode=0, stdout=json.dumps(result).encode())

            with patch.object(Path, "read_bytes", read), patch.object(Path, "exists", lambda p: str(p) == "/dev/disk/by-id/virtio-bridge-test" or original_exists(p)), patch.object(api.os, "geteuid", return_value=0), patch.object(api.subprocess, "run", side_effect=command):
                result = api.run({"KIND_CLUSTER": "owned"}, "kind-owned", "d" * 64, archive, digest, bundle)
            self.assertEqual(3, len([d for d in created if d["kind"] == "Task"]))
            self.assertEqual(3, len(result["tasks"]))
            self.assertEqual("prepare-workspace", result["tasks"][2]["initContainers"][0]["name"])
            self.assertNotIn("unused-offline-fixture", json.dumps(result))

    def test_run_refuses_host_before_any_command(self):
        api = self.api()
        with patch.object(Path, "exists", return_value=False), patch.object(api.subprocess, "run") as command:
            with self.assertRaisesRegex(ValueError, "disposable guest"):
                api.run({}, "host", "bad", Path("missing"), "0" * 64, Path("missing"))
            command.assert_not_called()

    def test_import_has_no_execution(self):
        self.assertTrue(callable(self.api().run))

    def test_validated_members_are_whitelisted(self):
        with tempfile.TemporaryDirectory() as tmp:
            path, digest = self.archive(Path(tmp))
            self.assertEqual(["model-server"], self.api().seed_members(path, digest))

    def test_bad_hash_paths_links_and_unexpected_files_refused(self):
        api = self.api()
        with tempfile.TemporaryDirectory() as tmp:
            for name, kind in [("../escape", tarfile.REGTYPE),
                               ("/escape", tarfile.REGTYPE),
                               ("repo/../escape", tarfile.REGTYPE),
                               ("repo/hooks/post-update", tarfile.REGTYPE),
                               ("repo/HEAD", tarfile.SYMTYPE),
                               ("unexpected", tarfile.REGTYPE)]:
                path, digest = self.archive(Path(tmp), name, kind)
                with self.assertRaises(ValueError):
                    api.seed_members(path, digest)
            path, digest = self.archive(Path(tmp))
            with self.assertRaises(ValueError):
                api.seed_members(path, "0" * 64)

    def test_actual_status_evidence_requires_terminated_success_and_digest(self):
        api = self.api()
        row = {"name": "fetch-session", "imageID": "containerd://sha256:" + "a" * 64,
               "state": {"terminated": {"exitCode": 0}}}
        self.assertEqual(0, api.init_evidence(row, {"sha256:" + "a" * 64})["exitCode"])
        for bad in [{**row, "imageID": ""}, {**row, "state": {"waiting": {}}},
                    {**row, "state": {"terminated": {"exitCode": 1}}}]:
            with self.assertRaises(ValueError):
                api.init_evidence(bad, {"sha256:" + "a" * 64})


if __name__ == "__main__":
    unittest.main()
