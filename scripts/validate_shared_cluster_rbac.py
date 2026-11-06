#!/usr/bin/env python3
"""Validate the shared-cluster RBAC design in an owned kind cluster.

This is an authorization test, not an Orka installation or conformance test. It
uses small validation-only Agent and Task CRDs so allowed and denied Kubernetes
requests exercise the same group/resource names as Orka without downloading or
running Orka.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
VERSIONS_FILE = ROOT / "images" / "guest" / "versions.json"
RBAC_FILE = ROOT / "docs" / "shared-clusters" / "rbac.yaml"
EXPECTED_KIND = "v0.33.0"
EXPECTED_KUBERNETES_MINOR = "1.35"
NODE_IMAGE = (
    "kindest/node:v1.35.8@"
    "sha256:07b2536e30b803ed61d1677a79df6115f798ce64c80f9e22f6ed45afd09323c0"
)
OUTPUT_LIMIT = 1024 * 1024
COMMAND_TIMEOUT = 120
CLUSTER_TIMEOUT = 300
MARKER_NAME = "bridge-shared-cluster-rbac"
VALIDATION_LABEL = "app.kubernetes.io/part-of"
VALIDATION_VALUE = "orka-bridge-rbac-validation"

CREATOR_A_NAMESPACE = "bridge-creator-a"
CREATOR_B_NAMESPACE = "bridge-creator-b"
UNSAFE_NAMESPACE = "bridge-shared-unsafe"
CREATOR_A = f"system:serviceaccount:{CREATOR_A_NAMESPACE}:bridge-creator"
CREATOR_B = f"system:serviceaccount:{CREATOR_B_NAMESPACE}:bridge-creator"
OBSERVER_A = f"system:serviceaccount:{CREATOR_A_NAMESPACE}:bridge-observer"
OBSERVER_B = f"system:serviceaccount:{CREATOR_B_NAMESPACE}:bridge-observer"


class ValidationError(RuntimeError):
    """A validation precondition or assertion failed."""


class CommandError(ValidationError):
    """A bounded child command failed."""


class Runner:
    """Use file-backed output so inherited pipes cannot delay timeout cleanup."""

    def __init__(self, env: dict[str, str] | None = None):
        self.env = os.environ.copy()
        if env:
            self.env.update(env)

    def run(
        self,
        args: list[str],
        *,
        input_bytes: bytes | None = None,
        timeout: int = COMMAND_TIMEOUT,
        check: bool = True,
    ) -> subprocess.CompletedProcess[bytes]:
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            try:
                result = subprocess.run(
                    args, input=input_bytes,
                    stdin=subprocess.DEVNULL if input_bytes is None else None,
                    stdout=stdout, stderr=stderr,
                    env=self.env, timeout=timeout, check=False,
                )
            except subprocess.TimeoutExpired as exc:
                raise CommandError(f"command timed out after {timeout}s: {args[0]}") from exc
            except OSError as exc:
                raise CommandError(f"cannot start command: {args[0]}") from exc
            # Child output is opaque: never include it in failure diagnostics.
            if check and result.returncode != 0:
                raise CommandError(f"command failed ({result.returncode}): {args[0]}")
            outputs = []
            for name, stream in (("stdout", stdout), ("stderr", stderr)):
                stream.seek(0)
                value = stream.read(OUTPUT_LIMIT + 1)
                if len(value) > OUTPUT_LIMIT:
                    raise CommandError(f"command {name} exceeded {OUTPUT_LIMIT} bytes: {args[0]}")
                outputs.append(value)
            return subprocess.CompletedProcess(args, result.returncode, *outputs)


class Kube:
    """kubectl scoped to one private kubeconfig and exact context."""

    def __init__(self, runner: Runner, executable: str, kubeconfig: Path, context: str):
        self.runner = runner
        self.executable = executable
        self.kubeconfig = kubeconfig
        self.context = context

    def command(self, *args: str, as_user: str | None = None) -> list[str]:
        command = [
            self.executable,
            "--kubeconfig",
            str(self.kubeconfig),
            "--context",
            self.context,
        ]
        if as_user:
            command.extend(["--as", as_user])
            for group in service_account_groups(as_user):
                command.extend(["--as-group", group])
        command.extend(args)
        return command

    def run(
        self,
        *args: str,
        as_user: str | None = None,
        input_value: Any | None = None,
        check: bool = True,
        timeout: int = COMMAND_TIMEOUT,
    ) -> subprocess.CompletedProcess[bytes]:
        payload = None
        if input_value is not None:
            payload = (json.dumps(input_value, separators=(",", ":")) + "\n").encode()
        return self.runner.run(
            self.command(*args, as_user=as_user),
            input_bytes=payload,
            check=check,
            timeout=timeout,
        )

    def json(
        self,
        *args: str,
        as_user: str | None = None,
        input_value: Any | None = None,
    ) -> Any:
        result = self.run(*args, as_user=as_user, input_value=input_value)
        try:
            return json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise ValidationError(f"kubectl returned invalid JSON for {args[0]}") from exc

    def apply(self, value: Any) -> None:
        self.run("apply", "-f", "-", input_value=value)


@dataclass(frozen=True)
class AccessCheck:
    name: str
    user: str
    namespace: str
    group: str
    resource: str
    verb: str
    allowed: bool
    subresource: str = ""
    resource_name: str = ""


def service_account_groups(user: str) -> tuple[str, ...]:
    parts = user.split(":")
    if len(parts) != 4 or parts[:2] != ["system", "serviceaccount"] or not parts[2] or not parts[3]:
        return ()
    return ("system:serviceaccounts", f"system:serviceaccounts:{parts[2]}", "system:authenticated")


def load_profile(path: Path = VERSIONS_FILE) -> dict[str, Any]:
    try:
        profile = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValidationError(f"cannot read {path}") from exc
    if profile.get("kind") != EXPECTED_KIND:
        raise ValidationError(
            f"validator expects kind {EXPECTED_KIND}, profile selects {profile.get('kind')!r}"
        )
    kubernetes = str(profile.get("kubernetes", ""))
    if not kubernetes.startswith(f"v{EXPECTED_KUBERNETES_MINOR}."):
        raise ValidationError(
            "validator node image must stay in the profile's Kubernetes minor release"
        )
    return profile


def resolve_tool(value: str) -> str:
    if os.sep in value or (os.altsep and os.altsep in value):
        path = Path(value).expanduser().resolve()
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValidationError(f"tool is not executable: {value}")
        return str(path)
    resolved = shutil.which(value)
    if not resolved:
        raise ValidationError(f"required tool not found: {value}")
    return resolved


def parse_kind_version(output: bytes) -> str:
    match = re.search(rb"\b(v\d+\.\d+\.\d+)\b", output)
    if not match:
        raise ValidationError("could not read kind version")
    return match.group(1).decode()


def parse_kubectl_minor(output: bytes) -> str:
    try:
        value = json.loads(output)
        version = value["clientVersion"]["gitVersion"]
    except (json.JSONDecodeError, KeyError, TypeError) as exc:
        raise ValidationError("could not read kubectl version") from exc
    match = re.match(r"^v(\d+)\.(\d+)\.", version)
    if not match:
        raise ValidationError(f"unexpected kubectl version: {version!r}")
    return f"{match.group(1)}.{match.group(2)}"


def preflight(runner: Runner, kind: str, kubectl: str, provider: str) -> None:
    load_profile()
    version = parse_kind_version(runner.run([kind, "version"]).stdout)
    if version != EXPECTED_KIND:
        raise ValidationError(f"kind {EXPECTED_KIND} is required; found {version}")
    minor = parse_kubectl_minor(
        runner.run([kubectl, "version", "--client", "--output=json"]).stdout
    )
    if minor != EXPECTED_KUBERNETES_MINOR:
        raise ValidationError(
            f"kubectl {EXPECTED_KUBERNETES_MINOR}.x is required; found {minor}.x"
        )
    if provider == "podman":
        runner.run([resolve_tool("podman"), "info", "--format", "json"])
    else:
        runner.run([resolve_tool("docker"), "info", "--format", "{{json .ServerVersion}}"])


def validation_crds() -> dict[str, Any]:
    items = []
    for plural, singular, kind in (
        ("agents", "agent", "Agent"),
        ("tasks", "task", "Task"),
    ):
        items.append(
            {
                "apiVersion": "apiextensions.k8s.io/v1",
                "kind": "CustomResourceDefinition",
                "metadata": {
                    "name": f"{plural}.core.orka.ai",
                    "labels": {VALIDATION_LABEL: VALIDATION_VALUE},
                },
                "spec": {
                    "group": "core.orka.ai",
                    "scope": "Namespaced",
                    "names": {
                        "plural": plural,
                        "singular": singular,
                        "kind": kind,
                        "listKind": f"{kind}List",
                    },
                    "versions": [
                        {
                            "name": "v1alpha1",
                            "served": True,
                            "storage": True,
                            "schema": {
                                "openAPIV3Schema": {
                                    "type": "object",
                                    "properties": {
                                        "spec": {
                                            "type": "object",
                                            "x-kubernetes-preserve-unknown-fields": True,
                                        },
                                        "status": {
                                            "type": "object",
                                            "x-kubernetes-preserve-unknown-fields": True,
                                        },
                                    },
                                }
                            },
                            "subresources": {"status": {}},
                        }
                    ],
                },
            }
        )
    return {"apiVersion": "v1", "kind": "List", "items": items}


def unsafe_namespace_objects() -> dict[str, Any]:
    labels = {VALIDATION_LABEL: VALIDATION_VALUE}
    items: list[dict[str, Any]] = [
        {
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {"name": UNSAFE_NAMESPACE, "labels": labels},
        }
    ]
    for name, subject_namespace in (
        ("creator-a", CREATOR_A_NAMESPACE),
        ("creator-b", CREATOR_B_NAMESPACE),
    ):
        items.append(
            {
                "apiVersion": "rbac.authorization.k8s.io/v1",
                "kind": "RoleBinding",
                "metadata": {
                    "name": name,
                    "namespace": UNSAFE_NAMESPACE,
                    "labels": labels,
                },
                "roleRef": {
                    "apiGroup": "rbac.authorization.k8s.io",
                    "kind": "ClusterRole",
                    "name": "bridge-orka-creator",
                },
                "subjects": [
                    {
                        "kind": "ServiceAccount",
                        "name": "bridge-creator",
                        "namespace": subject_namespace,
                    }
                ],
            }
        )
    return {"apiVersion": "v1", "kind": "List", "items": items}


def marker(run_id: str) -> dict[str, Any]:
    return {
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {
            "name": MARKER_NAME,
            "namespace": "kube-system",
            "labels": {VALIDATION_LABEL: VALIDATION_VALUE},
        },
        "data": {"runId": run_id},
    }


def access_matrix() -> list[AccessCheck]:
    checks: list[AccessCheck] = []

    def add(
        name: str,
        user: str,
        namespace: str,
        resource: str,
        verb: str,
        allowed: bool,
        *,
        group: str = "core.orka.ai",
        subresource: str = "",
        resource_name: str = "",
    ) -> None:
        checks.append(
            AccessCheck(
                name,
                user,
                namespace,
                group,
                resource,
                verb,
                allowed,
                subresource,
                resource_name,
            )
        )

    for label, user, own, other in (
        ("creator-a", CREATOR_A, CREATOR_A_NAMESPACE, CREATOR_B_NAMESPACE),
        ("creator-b", CREATOR_B, CREATOR_B_NAMESPACE, CREATOR_A_NAMESPACE),
    ):
        for resource, verbs in (
            ("agents", ("get", "list", "watch", "create", "update", "patch", "delete")),
            ("tasks", ("get", "list", "watch", "create", "update", "patch", "delete")),
            ("sessions", ("get", "list", "create", "update", "delete")),
        ):
            for verb in verbs:
                add(f"{label} {verb} own {resource}", user, own, resource, verb, True)
        for resource in ("agents", "tasks"):
            add(f"{label} get own {resource} status", user, own, resource, "get", True, subresource="status")
            add(f"{label} denied update own {resource} status", user, own, resource, "update", False, subresource="status")
            add(f"{label} denied update own {resource} finalizers", user, own, resource, "update", False, subresource="finalizers")
        add(f"{label} decide own approval", user, own, "tasks", "update", True, subresource="approvals")
        add(f"{label} create own chat", user, own, "chats", "create", True)
        add(f"{label} read own chat config", user, own, "chats", "get", True, subresource="config")
        for resource in ("providers", "tools", "skills"):
            add(f"{label} read own {resource}", user, own, resource, "list", True)
            add(f"{label} denied create own {resource}", user, own, resource, "create", False)
        for resource in ("agents", "tasks", "sessions"):
            for verb in ("get", "create", "patch", "delete"):
                add(f"{label} denied {verb} foreign {resource}", user, other, resource, verb, False)
        for verb in ("get", "list"):
            add(f"{label} denied {verb} secrets", user, own, "secrets", verb, False, group="")
        for resource, group, verb, subresource in (
            ("pods", "", "create", ""),
            ("jobs", "batch", "create", ""),
            ("deployments", "apps", "create", ""),
            ("serviceaccounts", "", "create", ""),
            ("serviceaccounts", "", "create", "token"),
            ("roles", "rbac.authorization.k8s.io", "create", ""),
            ("rolebindings", "rbac.authorization.k8s.io", "create", ""),
            ("roles", "rbac.authorization.k8s.io", "escalate", ""),
        ):
            add(
                f"{label} denied {verb} {resource}{('/' + subresource) if subresource else ''}",
                user,
                own,
                resource,
                verb,
                False,
                group=group,
                subresource=subresource,
            )
        add(
            f"{label} denied bind creator role",
            user,
            own,
            "clusterroles",
            "bind",
            False,
            group="rbac.authorization.k8s.io",
            resource_name="bridge-orka-creator",
        )
        add(
            f"{label} denied impersonate users",
            user,
            "",
            "users",
            "impersonate",
            False,
            group="",
            resource_name="admin",
        )
        add(
            f"{label} denied impersonate groups",
            user,
            "",
            "groups",
            "impersonate",
            False,
            group="",
            resource_name="system:masters",
        )
        add(
            f"{label} denied impersonate serviceaccounts",
            user,
            own,
            "serviceaccounts",
            "impersonate",
            False,
            group="",
            resource_name="bridge-observer",
        )

    for label, user, namespace, other in (
        ("observer-a", OBSERVER_A, CREATOR_A_NAMESPACE, CREATOR_B_NAMESPACE),
        ("observer-b", OBSERVER_B, CREATOR_B_NAMESPACE, CREATOR_A_NAMESPACE),
    ):
        for verb in ("get", "list", "watch"):
            add(f"{label} {verb} agents", user, namespace, "agents", verb, True)
            add(f"{label} {verb} tasks", user, namespace, "tasks", verb, True)
        for resource in ("agents", "tasks"):
            add(f"{label} get {resource} status", user, namespace, resource, "get", True, subresource="status")
        for verb in ("get", "list"):
            add(f"{label} {verb} sessions", user, namespace, "sessions", verb, True)
        for resource in ("agents", "tasks", "sessions"):
            for verb in ("create", "patch", "delete"):
                add(f"{label} denied {verb} {resource}", user, namespace, resource, verb, False)
            add(f"{label} denied foreign {resource}", user, other, resource, "get", False)
        add(f"{label} denied secrets", user, namespace, "secrets", "list", False, group="")
        add(f"{label} denied cluster namespace list", user, "", "namespaces", "list", False, group="")

    for label, user in (("creator-a", CREATOR_A), ("creator-b", CREATOR_B)):
        add(f"unsafe control: {label} can patch shared Agent", user, UNSAFE_NAMESPACE, "agents", "patch", True)
    return checks


def subject_access_review(check: AccessCheck) -> dict[str, Any]:
    attributes: dict[str, Any] = {
        "group": check.group,
        "resource": check.resource,
        "verb": check.verb,
    }
    if check.namespace:
        attributes["namespace"] = check.namespace
    if check.subresource:
        attributes["subresource"] = check.subresource
    if check.resource_name:
        attributes["name"] = check.resource_name
    spec: dict[str, Any] = {"user": check.user, "resourceAttributes": attributes}
    groups = service_account_groups(check.user)
    if groups:
        spec["groups"] = list(groups)
    return {
        "apiVersion": "authorization.k8s.io/v1",
        "kind": "SubjectAccessReview",
        "spec": spec,
    }


def validate_access_matrix(kube: Kube, checks: Iterable[AccessCheck]) -> None:
    for check in checks:
        if check.group == "core.orka.ai" and check.resource not in ("agents", "tasks"):
            # can-i relies on discovery and loses the group for resources absent
            # from Kubernetes. Orka's virtual APIs need an explicit review.
            response = kube.json("create", "-f", "-", "-o", "json", input_value=subject_access_review(check))
            status = response.get("status", {})
            if status.get("evaluationError") or not isinstance(status.get("allowed"), bool) or status.get("denied") is True:
                raise ValidationError(f"{check.name}: invalid authorization verdict")
            actual = status["allowed"]
        else:
            result = kube.run(*can_i_args(check), as_user=check.user, check=False)
            verdict = result.stdout.strip()
            if (result.returncode, verdict) not in ((0, b"yes"), (1, b"no")):
                raise ValidationError(f"{check.name}: invalid can-i verdict")
            actual = verdict == b"yes"
        if actual != check.allowed:
            raise ValidationError(
                f"{check.name}: expected {'allowed' if check.allowed else 'denied'}, "
                f"got {'allowed' if actual else 'denied'}"
            )
        print(f"PASS authorization: {check.name}")


def can_i_args(check: AccessCheck) -> list[str]:
    resource = check.resource + (f".{check.group}" if check.group else "")
    if check.resource_name:
        resource += f"/{check.resource_name}"
    args = ["auth", "can-i", check.verb, resource]
    args += ["--namespace", check.namespace] if check.namespace else ["--all-namespaces"]
    if check.subresource:
        args += ["--subresource", check.subresource]
    return args


def custom_resource(kind: str, name: str, namespace: str, owner: str) -> dict[str, Any]:
    return {
        "apiVersion": "core.orka.ai/v1alpha1",
        "kind": kind,
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": {"bridge.orka.ai/validation-owner": owner},
        },
        "spec": {"validationOwner": owner},
    }


def validate_shared_namespace_mutation(kube: Kube) -> None:
    shared = custom_resource("Agent", "shared-agent", UNSAFE_NAMESPACE, "creator-a")
    kube.run("create", "-f", "-", as_user=CREATOR_A, input_value=shared)
    kube.run(
        "patch",
        "agent",
        "shared-agent",
        "-n",
        UNSAFE_NAMESPACE,
        "--type=merge",
        "-p",
        '{"metadata":{"labels":{"bridge.orka.ai/changed-by":"creator-b"}}}',
        as_user=CREATOR_B,
    )
    changed = kube.json("get", "agent", "shared-agent", "-n", UNSAFE_NAMESPACE, "-o", "json")
    if changed.get("metadata", {}).get("labels", {}).get("bridge.orka.ai/changed-by") != "creator-b":
        raise ValidationError("unsafe shared-namespace control did not demonstrate cross-creator mutation")
    print("PASS negative control: creator B changed creator A's shared-namespace Agent")

    kube.run("delete", "agent", "shared-agent", "-n", UNSAFE_NAMESPACE, as_user=CREATOR_B)


def verify_cleanup_marker(kube: Kube, run_id: str) -> None:
    value = kube.json("get", "configmap", MARKER_NAME, "-n", "kube-system", "-o", "json")
    if value.get("data", {}).get("runId") != run_id:
        raise ValidationError("refusing cleanup: validation ownership marker does not match")
    if value.get("metadata", {}).get("labels", {}).get(VALIDATION_LABEL) != VALIDATION_VALUE:
        raise ValidationError("refusing cleanup: validation ownership label is missing")


def install_validation_resources(kube: Kube, run_id: str) -> None:
    kube.apply(marker(run_id))
    kube.run("apply", "-f", str(RBAC_FILE))
    kube.apply(validation_crds())
    for name in ("agents.core.orka.ai", "tasks.core.orka.ai"):
        kube.run(
            "wait",
            "--for=condition=Established",
            f"customresourcedefinition/{name}",
            "--timeout=60s",
        )
    kube.apply(unsafe_namespace_objects())


def create_cluster(runner: Runner, kind: str, name: str, kubeconfig: Path) -> None:
    existing = runner.run([kind, "get", "clusters"]).stdout.decode().splitlines()
    if name in existing:
        raise ValidationError(f"refusing to adopt existing kind cluster {name}")
    runner.run(
        [
            kind,
            "create",
            "cluster",
            "--name",
            name,
            "--image",
            NODE_IMAGE,
            "--kubeconfig",
            str(kubeconfig),
            "--wait",
            "180s",
        ],
        timeout=CLUSTER_TIMEOUT,
    )
    kubeconfig.chmod(0o600)


def delete_cluster(runner: Runner, kind: str, name: str, kubeconfig: Path) -> None:
    runner.run(
        [kind, "delete", "cluster", "--name", name, "--kubeconfig", str(kubeconfig)],
        timeout=CLUSTER_TIMEOUT,
    )


def validate(args: argparse.Namespace) -> None:
    kind = resolve_tool(args.kind)
    kubectl = resolve_tool(args.kubectl)
    provider_env = {"KIND_EXPERIMENTAL_PROVIDER": args.provider}
    runner = Runner(provider_env)
    preflight(runner, kind, kubectl, args.provider)
    print(f"PASS preflight: kind {EXPECTED_KIND}, kubectl {EXPECTED_KUBERNETES_MINOR}.x, {args.provider}")

    run_id = secrets.token_hex(16)
    cluster_name = f"bridge-rbac-{run_id[:10]}"
    run_dir = Path(tempfile.mkdtemp(prefix="bridge-shared-rbac-"))
    run_dir.chmod(0o700)
    kubeconfig = run_dir / "kubeconfig"
    context = f"kind-{cluster_name}"
    created = False
    keep = args.keep_cluster
    failure: BaseException | None = None

    try:
        # Set this before invoking kind so an interrupted create retains the private
        # kubeconfig instead of orphaning a cluster we can no longer identify safely.
        created = True
        create_cluster(runner, kind, cluster_name, kubeconfig)
        kube = Kube(runner, kubectl, kubeconfig, context)
        current = kube.run("config", "current-context").stdout.decode().strip()
        if current != context:
            raise ValidationError(f"private kubeconfig selected {current!r}, expected {context!r}")
        install_validation_resources(kube, run_id)
        validate_access_matrix(kube, access_matrix())
        validate_shared_namespace_mutation(kube)
        print("PASS shared-cluster RBAC validation")
    except BaseException as exc:  # Cleanup also runs for Ctrl-C.
        failure = exc
    finally:
        if created and keep:
            print(f"KEPT cluster {cluster_name}")
            print(f"KEPT kubeconfig {kubeconfig}")
        elif created:
            try:
                verify_cleanup_marker(Kube(runner, kubectl, kubeconfig, context), run_id)
                delete_cluster(runner, kind, cluster_name, kubeconfig)
                print(f"PASS cleanup: deleted owned cluster {cluster_name}")
            except BaseException as cleanup_error:
                print(f"error: cleanup failed: {cleanup_error}", file=sys.stderr)
                print(f"cluster retained: {cluster_name}", file=sys.stderr)
                print(f"kubeconfig retained: {kubeconfig}", file=sys.stderr)
                if failure is None:
                    failure = cleanup_error
                keep = True
        if not keep:
            shutil.rmtree(run_dir)

    if failure is not None:
        raise failure


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(description=__doc__)
    value.add_argument("--provider", required=True, choices=("docker", "podman"))
    value.add_argument("--kind", default="kind", help="kind v0.33.0 executable")
    value.add_argument("--kubectl", default="kubectl", help="kubectl v1.35.x executable")
    value.add_argument(
        "--keep-cluster",
        action="store_true",
        help="retain the owned cluster and private kubeconfig for diagnosis",
    )
    return value


def main(argv: list[str] | None = None) -> int:
    try:
        validate(parser().parse_args(argv))
        return 0
    except KeyboardInterrupt:
        print("error: interrupted", file=sys.stderr)
        return 130
    except ValidationError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
