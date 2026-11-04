"""Tiny synthetic inputs. These are never a bootable or publication-ready guest."""
import hashlib
import io
import json
from pathlib import Path
import tarfile

SOURCE = "59703d6eac1760aa2dc5fed4dee9dcb74c6e5186"
ROLES = ["kind-node", "controller", "ai-worker", "general-worker", "publisher", "session-loader", "vekil", "telegram", "teams"]

def sha(data):
    return hashlib.sha256(data).hexdigest()

def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()

def archive(root: Path, name="controller", architecture="amd64", extra=None):
    layer = b"synthetic non-bootable layer"
    config = encoded({"architecture": architecture, "os": "linux", "rootfs": {"type": "layers", "diff_ids": ["sha256:" + sha(layer)]}})
    manifest = encoded({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json", "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": "sha256:" + sha(config), "size": len(config)}, "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar", "digest": "sha256:" + sha(layer), "size": len(layer)}]})
    digest = "sha256:" + sha(manifest)
    index = encoded({"schemaVersion": 2, "manifests": [{"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": digest, "size": len(manifest)}]})
    entries = {"oci-layout": encoded({"imageLayoutVersion": "1.0.0"}), "index.json": index, "blobs/sha256/" + sha(manifest): manifest, "blobs/sha256/" + sha(config): config, "blobs/sha256/" + sha(layer): layer}
    if extra:
        entries.update(extra)
    path = root / (name + ".tar")
    with tarfile.open(path, "w") as tar:
        for entry, data in entries.items():
            info = tarfile.TarInfo(entry)
            info.size, info.mtime = len(data), 123
            tar.addfile(info, io.BytesIO(data))
    return {"name": name, "repository": "registry.example.com/fixture/" + name, "manifestDigest": digest, "configDigest": "sha256:" + sha(config), "archive": path.name, "archiveSha256": sha(path.read_bytes())}

def lock(root: Path):
    (root / "chart.tgz").write_bytes(b"synthetic-chart")
    (root / "base.qcow2").write_bytes(b"synthetic-base")
    (root / "kind").write_bytes(b"synthetic-kind")
    value = {"schemaVersion": "bridge-guest-inputs/v1", "architecture": "amd64", "sourceCommit": SOURCE,
            "orka": {"mode": "source", "identity": SOURCE, "chartPath": "chart.tgz", "chartSha256": sha(b"synthetic-chart")},
            "components": [archive(root, name) for name in ROLES],
            "packages": [{"name": "debian-base", "version": "20261026-2643", "path": "base.qcow2", "sha256": sha(b"synthetic-base")}],
            "tools": [{"name": "kind", "version": "v0.33.0", "path": "kind", "sha256": sha(b"synthetic-kind")}],
            "nodeLocalRef": "", "kmxProfileSha256": None}
    value["nodeLocalRef"] = "kindest/node:bridge-" + value["components"][0]["manifestDigest"][7:23] + "-amd64"
    return value
