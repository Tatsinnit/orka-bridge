#!/usr/bin/python3
"""TEST ONLY disposable guest proof. Never installed in factory output or run on import."""
import hashlib
import json
import os
import re
import subprocess
import tarfile
import time
from pathlib import Path, PurePosixPath

MAX_SEED = 32 * 1024 * 1024
STATE = Path("/var/lib/bridge/reference-fixture")


def seed_members(archive: Path, expected_sha256: str) -> list[str]:
    """Validate before extraction; no links, hooks, traversal or extra files."""
    if archive.is_symlink() or not archive.is_file() or archive.stat().st_size > MAX_SEED:
        raise ValueError("fixture archive size/type refused")
    if not re.fullmatch(r"[0-9a-f]{64}", expected_sha256):
        raise ValueError("fixture digest refused")
    with archive.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != expected_sha256:
            raise ValueError("fixture digest mismatch")
    names, total = [], 0
    with tarfile.open(archive, "r:") as stream:
        for member in stream:
            name = member.name
            path = PurePosixPath(name)
            allowed = name in {"model-server", "repo/HEAD", "repo/config",
                               "repo/description", "repo/info/refs", "repo/objects/info/packs"}
            allowed = allowed or bool(re.fullmatch(
                r"repo/(?:refs/heads/main|objects/[0-9a-f]{2}/[0-9a-f]{38})", name))
            if (not allowed or not member.isfile() or path.is_absolute()
                    or ".." in path.parts or str(path) != name or name in names
                    or member.size < 0 or member.pax_headers):
                raise ValueError("fixture archive member refused")
            total += member.size
            if total > MAX_SEED or len(names) >= 128:
                raise ValueError("fixture archive expanded size/count refused")
            names.append(name)
    if "model-server" not in names:
        raise ValueError("fixture server missing")
    return names


def init_evidence(status: dict, expected_digests: set[str]) -> dict:
    """Return only closed evidence fields, never env, logs, messages or content."""
    image_id = status.get("imageID", "")
    digest = image_id.rsplit("@", 1)[-1].removeprefix("containerd://")
    ended = status.get("state", {}).get("terminated", {})
    if digest not in expected_digests or ended.get("exitCode") != 0:
        raise ValueError("actual init image/completion evidence unavailable")
    return {"name": status["name"], "imageID": image_id, "exitCode": 0}


def task_documents(endpoint, busybox):
    rows = []
    for name in ("fixture-ai", "fixture-session", "fixture-workspace"):
        spec = {"type": "ai", "agentRef": {"name": "fixture-local"},
                "prompt": "Return the offline fixture answer.", "timeout": "100s",
                "retryPolicy": {"maxRetries": 0}}
        if name == "fixture-session":
            spec["sessionRef"] = {"name": "fixture-session", "create": True, "append": True}
        if name == "fixture-workspace":
            # A custom container image selects the native prepare-workspace
            # general-worker init path; no controller annotations are forged.
            spec = {"type": "container", "image": busybox,
                    "command": ["sh", "-c", "test -s /workspace/README.txt"],
                    "workspace": {"gitRepo": endpoint + "/repo/", "branch": "main", "ref": "main"},
                    "timeout": "100s", "retryPolicy": {"maxRetries": 0}}
        rows.append({"apiVersion": "core.orka.ai/v1alpha1", "kind": "Task",
                     "metadata": {"name": name, "namespace": "orka-system"}, "spec": spec})
    return rows


def task_evidence(task, jobs, pods, worker_digests, init_digests):
    def owned(obj, uid):
        return any(r.get("uid") == uid and r.get("controller") is True
                   for r in obj["metadata"].get("ownerReferences", []))
    if task.get("status", {}).get("phase") != "Succeeded":
        raise ValueError("fixture Task did not succeed")
    jobs = [j for j in jobs if owned(j, task["metadata"]["uid"])]
    if len(jobs) != 1:
        raise ValueError("fixture Task Job ownership ambiguous")
    pods = [p for p in pods if owned(p, jobs[0]["metadata"]["uid"])]
    if len(pods) != 1:
        raise ValueError("fixture Task Pod ownership ambiguous")
    pod = pods[0]
    workers = pod.get("status", {}).get("containerStatuses", [])
    if len(workers) != 1:
        raise ValueError("fixture worker status unavailable")
    worker = init_evidence(workers[0], worker_digests)
    inits = {r["name"]: r for r in pod.get("status", {}).get("initContainerStatuses", [])}
    evidence = []
    for name, digests in init_digests.items():
        if name not in inits:
            raise ValueError("required fixture init status missing: " + name)
        evidence.append(init_evidence(inits[name], digests))
    return {"name": task["metadata"]["name"], "uid": task["metadata"]["uid"],
            "phase": "Succeeded", "jobUID": jobs[0]["metadata"]["uid"],
            "podUID": pod["metadata"]["uid"], "worker": worker, "initContainers": evidence}


def run(env, context, node_id, seed_archive: Path, expected_sha256: str, bundle: Path) -> dict:
    """Execute only from the fixed disposable guest proof, never the build host."""
    state = STATE.parent
    if not Path("/dev/disk/by-id/virtio-bridge-test").exists() or os.geteuid() != 0:
        raise ValueError("fixture requires owned disposable guest")
    seed = json.loads(Path("/run/bridge/reference.json").read_bytes())
    journal = json.loads((state / "bootstrap.json").read_bytes())
    if (seed.get("caseId") != "offline-tasks" or seed.get("fixtureSha256") != expected_sha256
            or journal.get("phase") != "infrastructure-ready" or journal.get("nodeIds") != [node_id]
            or not re.fullmatch(r"[0-9a-f]{64}", node_id)
            or context != "kind-" + env.get("KIND_CLUSTER", "")):
        raise ValueError("fixture guest/node/context ownership refused")
    names = seed_members(seed_archive, expected_sha256)
    if not {"repo/HEAD", "repo/info/refs", "repo/refs/heads/main"} <= set(names):
        raise ValueError("fixture repository incomplete")
    manifest = json.loads((bundle / "install.json").read_bytes())
    images = {r["component"]: r for r in manifest["images"]}
    def ref(role):
        row = images[role]
        digest = row["manifestDigest"]
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
            raise ValueError("fixture image digest refused")
        return row["repository"] + "@" + digest
    def digests(role):
        row = images[role]
        return {row[k] for k in ("manifestDigest", "configDigest")}
    busybox = ref("session-loader")
    ref("ai-worker")
    ref("general-worker")

    poll_deadline = None

    def command(args, body=None, timeout=20):
        if poll_deadline is not None:
            remaining = poll_deadline - time.monotonic()
            if remaining <= 0:
                raise ValueError("fixture status polling timeout")
            timeout = min(timeout, remaining)
        result = subprocess.run(args, env=env, input=body, capture_output=True,
                                timeout=timeout, check=False)
        if result.returncode or len(result.stdout) > 1024 * 1024:
            # stderr and API bodies can contain credentials or task content.
            reason = re.search(rb'Error from server \(([A-Za-z]+)\)', result.stderr)
            status = reason.group(1).decode() if reason else 'no-safe-status'
            for token in (b'error looking up service account', b'violates PodSecurity', b'denied the request', b'exceeded quota'):
                if token in result.stderr:
                    status += '/' + token.decode()
            raise ValueError('fixture command failed: ' + Path(args[0]).name + ' ' + status)
        return result.stdout
    kube = ["kubectl", "--context", context, "--request-timeout=10s"]
    def get(resource, namespace="orka-system"):
        return json.loads(command([*kube, "-n", namespace, "get", resource, "-o", "json"]))
    def create(obj):
        try:
            return json.loads(command([*kube, "create", "-f", "-", "-o", "json"], json.dumps(obj).encode()))
        except ValueError as error:
            raise ValueError('fixture create ' + obj['kind'] + '/' + obj['metadata']['name'] + ': ' + str(error)) from error
    def obj(kind, name, spec=None, namespace="fixture-local", **fields):
        value = {"apiVersion": "v1", "kind": kind, "metadata": {"name": name}}
        if kind != "Namespace":
            value["metadata"]["namespace"] = namespace
        if spec is not None:
            value["spec"] = spec
        value.update(fields)
        return value

    # Check exact Docker ownership before copying; all Docker calls are inside run.
    node = json.loads(command(["docker", "inspect", node_id]))[0]
    if (node.get("Id") != node_id or node.get("Config", {}).get("Labels", {}).get(
            "io.x-k8s.kind.cluster") != env["KIND_CLUSTER"]):
        raise ValueError("fixture Docker node ownership refused")
    STATE.mkdir(mode=0o755)  # Exclusive: never merge fixture state from an earlier run.
    with tarfile.open(seed_archive, "r:") as archive:
        for name in names:
            target = STATE / name
            target.parent.mkdir(parents=True, exist_ok=True)
            source = archive.extractfile(name)
            assert source is not None  # The whitelist already established a regular member.
            with source, target.open("xb") as dest:
                dest.write(source.read(MAX_SEED + 1))
            target.chmod(0o755 if name == "model-server" else 0o644)
    command(["docker", "exec", node_id, "mkdir", "-p", str(STATE)])
    command(["docker", "cp", str(STATE) + "/.", node_id + ":" + str(STATE)])
    create(obj("Namespace", "fixture-local"))
    create(obj('ServiceAccount', 'fixture', namespace='fixture-local'))
    endpoint = "http://fixture.fixture-local.svc.cluster.local:8080"
    pod = obj("Pod", "fixture", {
        "automountServiceAccountToken": False, "serviceAccountName": "fixture", "restartPolicy": "Never",
        "containers": [{"name": "model", "image": busybox, "imagePullPolicy": "Never",
                        "command": [str(STATE / "model-server")],
                        "ports": [{"containerPort": 8080}],
                        "readinessProbe": {"httpGet": {"path": "/v1/models", "port": 8080}},
                        "volumeMounts": [{"name": "fixture", "mountPath": str(STATE), "readOnly": True}]}],
        "volumes": [{"name": "fixture", "hostPath": {"path": str(STATE), "type": "Directory"}}]})
    pod["metadata"]["labels"] = {"app": "fixture-local"}
    create(pod)
    create(obj("Service", "fixture", {"type": "ClusterIP", "selector": {"app": "fixture-local"},
                                      "ports": [{"port": 8080, "targetPort": 8080}]}))
    poll_deadline = deadline = time.monotonic() + 30
    while True:
        server = get("pod/fixture", "fixture-local")
        if any(c.get("type") == "Ready" and c.get("status") == "True"
               for c in server.get("status", {}).get("conditions", [])):
            break
        if time.monotonic() >= deadline:
            raise ValueError("fixture server readiness timeout")
        time.sleep(1)
    poll_deadline = None
    create(obj("Secret", "fixture-local", namespace="orka-system",
               stringData={"api-key": "unused-offline-fixture"}))
    for kind, spec in [
        ("Provider", {"type": "openai", "baseURL": endpoint + "/v1", "defaultModel": "offline-fixture",
                      "secretRef": {"name": "fixture-local", "key": "api-key"}}),
        ("Agent", {"providerRef": {"name": "fixture-local"}, "tools": [],
                   "systemPrompt": {"inline": "Answer briefly."}})]:
        resource = obj(kind, "fixture-local", spec, namespace="orka-system")
        resource["apiVersion"] = "core.orka.ai/v1alpha1"
        create(resource)
    results = []
    for document in task_documents(endpoint, busybox):
        created = create(document)  # Exactly one submission, no retry.
        name = document["metadata"]["name"]
        poll_deadline = deadline = time.monotonic() + 120
        while True:
            task = get("tasks.core.orka.ai/" + name)
            if task["metadata"]["uid"] != created["metadata"]["uid"]:
                raise ValueError("fixture Task replaced")
            phase = task.get("status", {}).get("phase")
            if phase == "Succeeded":
                break
            if phase in ("Failed", "Cancelled") or time.monotonic() >= deadline:
                raise ValueError("fixture Task failed/timeout: " + name)
            time.sleep(1)
        poll_deadline = None
        required = {}
        if name == "fixture-session":
            required["fetch-session"] = digests("session-loader")
        if name == "fixture-workspace":
            required["prepare-workspace"] = digests("general-worker")
        results.append(task_evidence(task, get("jobs")["items"], get("pods")["items"],
                                    digests("session-loader" if name == "fixture-workspace" else "ai-worker"),
                                    required))
    return {"schemaVersion": "bridge-task-fixture/v1", "fixtureSha256": expected_sha256,
            "tasks": results}
