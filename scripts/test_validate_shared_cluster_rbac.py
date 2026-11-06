"""Unit tests for the shared-cluster RBAC validator; no cluster calls."""

import importlib.util
import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path


SOURCE = Path(__file__).with_name("validate_shared_cluster_rbac.py")


def load_validator():
    spec = importlib.util.spec_from_file_location("validate_shared_cluster_rbac", SOURCE)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class ValidatorTests(unittest.TestCase):
    def test_profile_requires_pinned_kind_and_kubernetes_minor(self):
        api = load_validator()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "versions.json"
            path.write_text(json.dumps({"kind": "v0.33.0", "kubernetes": "v1.35.0"}))
            self.assertEqual("v0.33.0", api.load_profile(path)["kind"])

            path.write_text(json.dumps({"kind": "v0.32.0", "kubernetes": "v1.35.0"}))
            with self.assertRaisesRegex(api.ValidationError, "kind v0.33.0"):
                api.load_profile(path)

            path.write_text(json.dumps({"kind": "v0.33.0", "kubernetes": "v1.36.0"}))
            with self.assertRaisesRegex(api.ValidationError, "Kubernetes minor"):
                api.load_profile(path)

    def test_tool_versions_are_strictly_parsed(self):
        api = load_validator()
        self.assertEqual("v0.33.0", api.parse_kind_version(b"kind v0.33.0 go1.25 linux/amd64"))
        kubectl = {"clientVersion": {"gitVersion": "v1.35.8"}}
        self.assertEqual("1.35", api.parse_kubectl_minor(json.dumps(kubectl).encode()))
        with self.assertRaises(api.ValidationError):
            api.parse_kind_version(b"development build")
        with self.assertRaises(api.ValidationError):
            api.parse_kubectl_minor(b"{}")

    def test_kubectl_commands_always_use_private_scope(self):
        api = load_validator()
        kube = api.Kube(api.Runner(), "/tools/kubectl", Path("/private/kubeconfig"), "kind-owned")
        self.assertEqual(
            [
                "/tools/kubectl",
                "--kubeconfig",
                "/private/kubeconfig",
                "--context",
                "kind-owned",
                "--as",
                api.CREATOR_A,
                "--as-group",
                "system:serviceaccounts",
                "--as-group",
                "system:serviceaccounts:bridge-creator-a",
                "--as-group",
                "system:authenticated",
                "get",
                "agents",
            ],
            kube.command("get", "agents", as_user=api.CREATOR_A),
        )

    def test_matrix_has_isolation_escalation_and_unsafe_controls(self):
        api = load_validator()
        checks = api.access_matrix()
        names = {check.name: check for check in checks}
        self.assertFalse(names["creator-a denied get foreign agents"].allowed)
        self.assertFalse(names["creator-a denied list secrets"].allowed)
        self.assertFalse(names["creator-a denied update own agents status"].allowed)
        self.assertFalse(names["creator-a denied update own agents finalizers"].allowed)
        self.assertFalse(names["creator-a denied create own providers"].allowed)
        self.assertTrue(names["creator-a create own chat"].allowed)
        self.assertTrue(names["creator-a read own chat config"].allowed)
        self.assertFalse(names["creator-a denied create serviceaccounts/token"].allowed)
        self.assertFalse(names["creator-a denied bind creator role"].allowed)
        self.assertEqual(api.CREATOR_A_NAMESPACE, names["creator-a denied bind creator role"].namespace)
        self.assertEqual(api.CREATOR_B_NAMESPACE, names["creator-b denied bind creator role"].namespace)
        self.assertFalse(names["creator-a denied impersonate users"].allowed)
        self.assertFalse(names["creator-a denied impersonate groups"].allowed)
        self.assertFalse(names["creator-a denied impersonate serviceaccounts"].allowed)
        self.assertTrue(names["observer-a get agents"].allowed)
        self.assertTrue(names["observer-a get tasks"].allowed)
        self.assertTrue(names["observer-a get sessions"].allowed)
        self.assertFalse(names["observer-a denied patch tasks"].allowed)
        self.assertFalse(names["observer-a denied foreign sessions"].allowed)
        self.assertTrue(names["unsafe control: creator-b can patch shared Agent"].allowed)
        self.assertTrue(all(not (c.group == "" and c.resource == "secrets" and c.allowed) for c in checks))

    def test_subject_access_review_preserves_subresource_and_name(self):
        api = load_validator()
        check = api.AccessCheck(
            "named bind",
            api.CREATOR_A,
            api.CREATOR_A_NAMESPACE,
            "rbac.authorization.k8s.io",
            "clusterroles",
            "bind",
            False,
            resource_name="bridge-orka-creator",
        )
        attributes = api.subject_access_review(check)["spec"]["resourceAttributes"]
        self.assertEqual("bridge-orka-creator", attributes["name"])
        self.assertEqual(api.CREATOR_A_NAMESPACE, attributes["namespace"])
        args = api.can_i_args(check)
        self.assertIn("clusterroles.rbac.authorization.k8s.io/bridge-orka-creator", args)
        self.assertEqual(api.CREATOR_A_NAMESPACE, args[args.index("--namespace") + 1])
        self.assertEqual(
            [
                "system:serviceaccounts",
                "system:serviceaccounts:bridge-creator-a",
                "system:authenticated",
            ],
            api.subject_access_review(check)["spec"]["groups"],
        )

        approval = api.AccessCheck(
            "approval",
            api.CREATOR_A,
            api.CREATOR_A_NAMESPACE,
            "core.orka.ai",
            "tasks",
            "update",
            True,
            subresource="approvals",
        )
        self.assertEqual(
            "approvals",
            api.subject_access_review(approval)["spec"]["resourceAttributes"]["subresource"],
        )
        self.assertEqual(["--subresource", "approvals"], api.can_i_args(approval)[-2:])

    def test_cleanup_marker_must_match_run_and_label(self):
        api = load_validator()

        class FakeKube:
            def __init__(self, value):
                self.value = value

            def json(self, *args):
                return self.value

        valid = {
            "metadata": {"labels": {api.VALIDATION_LABEL: api.VALIDATION_VALUE}},
            "data": {"runId": "expected"},
        }
        api.verify_cleanup_marker(FakeKube(valid), "expected")
        with self.assertRaisesRegex(api.ValidationError, "marker does not match"):
            api.verify_cleanup_marker(FakeKube({**valid, "data": {"runId": "other"}}), "expected")
        with self.assertRaisesRegex(api.ValidationError, "label is missing"):
            api.verify_cleanup_marker(
                FakeKube({"metadata": {"labels": {}}, "data": {"runId": "expected"}}),
                "expected",
            )

    def test_can_i_rejects_errors_and_annotated_denials(self):
        api = load_validator()

        class FakeKube:
            def run(self, *args, **kwargs):
                return subprocess.CompletedProcess(args, self.code, self.output, b"")

        kube = FakeKube()
        check = api.AccessCheck("denial", api.CREATOR_A, api.CREATOR_A_NAMESPACE, "", "secrets", "get", False)
        kube.code, kube.output = 1, b"no\n"
        api.validate_access_matrix(kube, [check])
        for kube.code, kube.output in ((1, b""), (1, b"no - evaluation failed\n"), (0, b"no\n")):
            with self.assertRaisesRegex(api.ValidationError, "invalid can-i verdict"):
                api.validate_access_matrix(kube, [check])

    def test_virtual_resources_use_explicit_group_reviews(self):
        api = load_validator()

        class FakeKube:
            def json(self, *args, **kwargs):
                self.review = kwargs["input_value"]
                return {"status": self.status}

        kube = FakeKube()
        check = api.AccessCheck("session", api.OBSERVER_A, api.CREATOR_A_NAMESPACE, "core.orka.ai", "sessions", "get", True)
        kube.status = {"allowed": True}
        api.validate_access_matrix(kube, [check])
        self.assertEqual("core.orka.ai", kube.review["spec"]["resourceAttributes"]["group"])
        for kube.status in ({}, {"allowed": True, "denied": True}, {"allowed": True, "evaluationError": "error"}):
            with self.assertRaisesRegex(api.ValidationError, "invalid authorization verdict"):
                api.validate_access_matrix(kube, [check])

    def test_runner_stops_output_at_the_bound(self):
        api = load_validator()
        with self.assertRaisesRegex(api.CommandError, "stdout exceeded"):
            api.Runner().run(
                [sys.executable, "-c", f"import sys; sys.stdout.write('x' * {api.OUTPUT_LIMIT + 1})"]
            )

    def test_timeout_does_not_wait_for_descendant_output_handles(self):
        api = load_validator()
        start = time.monotonic()
        with self.assertRaisesRegex(api.CommandError, "timed out"):
            api.Runner().run([
                sys.executable, "-c",
                "import subprocess, sys, time; "
                "subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(3)']); "
                "time.sleep(10)",
            ], timeout=0.2)
        self.assertLess(time.monotonic() - start, 2)

    def test_failure_diagnostics_never_include_child_output(self):
        api = load_validator()
        with self.assertRaises(api.CommandError) as raised:
            api.Runner().run([
                sys.executable, "-c",
                "import sys; sys.stderr.write('Authorization: Bearer ' + 'secret-value' * 1000); sys.exit(1)",
            ])
        self.assertNotIn("secret-value", str(raised.exception))
        self.assertNotIn("Authorization", str(raised.exception))

    def test_delete_uses_the_private_kubeconfig(self):
        api = load_validator()

        class FakeRunner:
            def __init__(self):
                self.args = None

            def run(self, args, **kwargs):
                self.args = args
                return subprocess.CompletedProcess(args, 0, b"", b"")

        runner = FakeRunner()
        api.delete_cluster(runner, "/tools/kind", "owned", Path("/private/kubeconfig"))
        self.assertEqual(
            [
                "/tools/kind",
                "delete",
                "cluster",
                "--name",
                "owned",
                "--kubeconfig",
                "/private/kubeconfig",
            ],
            runner.args,
        )

    def test_rbac_fixture_has_no_global_binding_or_privileged_grants(self):
        text = (SOURCE.parents[1] / "docs" / "shared-clusters" / "rbac.yaml").read_text()
        self.assertNotIn("kind: ClusterRoleBinding", text)
        self.assertNotIn('resources: ["secrets"]', text)
        self.assertNotIn('resources: ["pods"]', text)
        self.assertNotIn('resources: ["roles"]', text)
        self.assertNotIn('verbs: ["*"]', text)
        self.assertIn('resources: ["sessions"]', text)

    def test_validation_crds_are_namespaced_and_status_is_separate(self):
        api = load_validator()
        crds = api.validation_crds()["items"]
        self.assertEqual({"agents.core.orka.ai", "tasks.core.orka.ai"}, {row["metadata"]["name"] for row in crds})
        for row in crds:
            self.assertEqual("Namespaced", row["spec"]["scope"])
            self.assertEqual({"status": {}}, row["spec"]["versions"][0]["subresources"])
            self.assertEqual(api.VALIDATION_VALUE, row["metadata"]["labels"][api.VALIDATION_LABEL])


if __name__ == "__main__":
    unittest.main()
