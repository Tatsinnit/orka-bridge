"""Stream provisional H1 OS packages; hashes establish integrity, not boot readiness."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import zlib

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from images.guest import lock as guest

MAX_CHUNK = 1024 ** 3
BLOCK = 1024 ** 2
MAX_JSON = 8 * BLOCK
HEX = r"[0-9a-f]{64}"
ID = r"[a-z0-9][a-z0-9.-]{0,62}"
VERSION = r"[A-Za-z0-9][A-Za-z0-9._+-]{0,95}"
TARGET = ("platform", "architecture", "format", "minimumHostVersion", "minimumAppVersion")
ROOT = ("schemaVersion", "publicationReady", "imageId", "buildSourceCommit", "stateCompatibilityId", "target", "components", "layout", "artifact")
require, shape, matches = guest.require, guest.shape, guest.matches


def canonical(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def read_json(path, expected_sha256=None):
    path = Path(path)
    require(not path.is_symlink() and path.is_file() and path.stat().st_size <= MAX_JSON, "invalid JSON file or size")
    with path.open("rb") as stream:
        data = stream.read(MAX_JSON + 1)
    require(len(data) <= MAX_JSON, "JSON exceeds size limit")
    if expected_sha256 is not None:
        require(matches(expected_sha256, HEX) and hashlib.sha256(data).hexdigest() == expected_sha256, "manifest checksum mismatch")
    return guest.read_json(data)


def target_valid(value):
    shape(value, TARGET)
    require((value["platform"], value["architecture"], value["format"]) in
            (("macos", "arm64", "raw"), ("windows", "amd64", "qcow2")), "unsupported platform/architecture/format")
    require(matches(value["minimumHostVersion"], r"[0-9]+\.[0-9]+(?:\.[0-9]+)?"), "invalid minimum host version")
    require(matches(value["minimumAppVersion"], r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?"), "invalid minimum app version")


def layout(fmt):
    return {"contractVersion": "H1/v1",
            "osDisk": {"role": "os-disk", "format": fmt, "virtualCapacityBytes": 17179869184,
                       "blockSerial": "bridge-os", "partitionTable": "gpt", "boot": "uefi", "privateWritableCopy": True},
            "stateDisk": {"role": "state-disk", "operation": "create-empty", "format": fmt,
                          "virtualCapacityBytes": 34359738368, "blockSerial": "bridge-state", "filesystem": "ext4",
                          "partitioning": "whole-device", "formatPolicy": "new-only"},
            "devices": {"contractVersion": "H1/v1", "diskBus": "virtio", "network": "nat", "entropy": True,
                        "clock": "utc", "controlPort": "ai.orka.bridge.control", "diagnostics": "separate"},
            "transport": {"protocolMajor": 1, "framing": "json-lines"},
            "testResources": {"vcpus": 4, "memoryBytes": 6442450944}}


def components_valid(c):
    shape(c, ("inputSchemaVersion", "inputIdentity", "canonicalLockSha256", "versionPolicySha256", "orka",
              "kmxProfileSha256", "kind", "kubernetes", "vekil", "tools"))
    require(c["inputSchemaVersion"] == "bridge-guest-inputs/v1", "unsupported component lock schema")
    for key in ("inputIdentity", "canonicalLockSha256", "versionPolicySha256"):
        require(matches(c[key], HEX), "invalid component identity")
    require(c["kmxProfileSha256"] is None or matches(c["kmxProfileSha256"], HEX), "invalid kmx profile")
    shape(c["orka"], ("mode", "identity", "sourceCommit", "chartSha256"))
    o = c["orka"]
    require(o["mode"] in ("source", "release") and matches(o["identity"], VERSION), "invalid Orka identity")
    require(matches(o["sourceCommit"], r"[0-9a-f]{40}") and matches(o["chartSha256"], HEX), "invalid Orka source")
    require(o["mode"] != "source" or o["identity"] == o["sourceCommit"], "Orka source identity mismatch")
    shape(c["kubernetes"], ("version", "provenance"))
    require(matches(c["kubernetes"]["version"], VERSION) and c["kubernetes"]["provenance"] == "policy-declared", "invalid Kubernetes policy")
    shape(c["vekil"], ("manifestDigest", "configDigest"))
    require(all(matches(v, r"sha256:" + HEX) for v in c["vekil"].values()), "invalid Vekil identity")
    require(isinstance(c["tools"], list) and 0 < len(c["tools"]) <= 128, "invalid tools")
    names = []
    for tool in c["tools"]:
        shape(tool, ("name", "version", "sha256"))
        require(matches(tool["name"], guest.NAME) and matches(tool["version"], VERSION) and matches(tool["sha256"], HEX), "invalid tool identity")
        names.append(tool["name"])
    require(names == sorted(set(names)) and "kind" in names, "invalid tool order or closure")
    kind = next(t for t in c["tools"] if t["name"] == "kind")
    require(c["kind"] == {k: kind[k] for k in ("version", "sha256")}, "kind identity mismatch")


def validate_metadata(value):
    shape(value, ("imageId", "buildSourceCommit", *TARGET, "stateCompatibilityId", "transportProtocolMajor", "componentLock"))
    target = {key: value[key] for key in TARGET}
    target_valid(target)
    require(matches(value["imageId"], ID) and ".." not in value["imageId"] and matches(value["stateCompatibilityId"], ID), "invalid image/state compatibility ID")
    require(matches(value["buildSourceCommit"], r"[0-9a-f]{40}"), "invalid build source commit")
    require(type(value["transportProtocolMajor"]) is int and value["transportProtocolMajor"] == 1, "unsupported transport major")
    lock = value["componentLock"]
    guest.validate_lock(lock, production=False)
    require(lock["architecture"] == target["architecture"], "component architecture mismatch")
    tools = sorted([{k: t[k] for k in ("name", "version", "sha256")} for t in lock["tools"]], key=lambda t: t["name"])
    kind = next(t for t in tools if t["name"] == "kind")
    vekil = next(t for t in lock["components"] if t["name"] == "vekil")
    policy_bytes = Path(guest.__file__).with_name("versions.json").read_bytes()
    c = {"inputSchemaVersion": lock["schemaVersion"], "inputIdentity": guest.input_identity(lock),
         "canonicalLockSha256": hashlib.sha256(canonical(lock)).hexdigest(),
         "versionPolicySha256": hashlib.sha256(policy_bytes).hexdigest(),
         "orka": {**{k: lock["orka"][k] for k in ("mode", "identity", "chartSha256")}, "sourceCommit": lock["sourceCommit"]},
         "kmxProfileSha256": lock["kmxProfileSha256"], "kind": {k: kind[k] for k in ("version", "sha256")},
         "kubernetes": {"version": json.loads(policy_bytes)["kubernetes"], "provenance": "policy-declared"},
         "vekil": {k: vekil[k] for k in ("manifestDigest", "configDigest")}, "tools": tools}
    components_valid(c)
    return {"schemaVersion": "bridge-image/v1", "publicationReady": False,
            **{k: value[k] for k in ("imageId", "buildSourceCommit", "stateCompatibilityId")},
            "target": target, "components": c, "layout": layout(target["format"])}


def prefix(manifest):
    return "-".join([manifest["imageId"], manifest["target"]["platform"], manifest["target"]["architecture"]])


class ChunkWriter:
    def __init__(self, out, stem, limit):
        self.out, self.stem, self.limit = out, stem, limit
        self.entries, self.file = [], None
        self.total, self.size = 0, 0
        self.whole = hashlib.sha256()

    def write(self, data):
        self.whole.update(data); self.total += len(data)
        view = memoryview(data)
        while view:
            if self.file is None:
                require(len(self.entries) < 50000, "too many chunks")
                self.name = f"{self.stem}.part-{len(self.entries):06d}"
                self.file = (self.out / self.name).open("xb")
                self.part, self.size = hashlib.sha256(), 0
            n = min(len(view), self.limit - self.size)
            self.file.write(view[:n]); self.part.update(view[:n]); self.size += n
            view = view[n:]
            if self.size == self.limit: self.finish()
        return len(data)

    def finish(self):
        if self.file is not None:
            self.file.close(); self.file = None
            self.entries.append({"order": len(self.entries), "name": self.name, "bytes": self.size, "sha256": self.part.hexdigest()})


def create(stream, metadata, out, chunk_bytes=MAX_CHUNK):
    require(type(chunk_bytes) is int and 0 < chunk_bytes <= MAX_CHUNK, "invalid chunk size")
    manifest = validate_metadata(metadata)
    first = stream.read(BLOCK)
    require(isinstance(first, bytes) and first, "empty or nonbinary payload")
    out = Path(out); out.mkdir()
    writer = ChunkWriter(out, prefix(manifest), chunk_bytes)
    original, size = hashlib.sha256(), 0
    try:
        with gzip.GzipFile(filename="", mode="wb", fileobj=writer, mtime=0) as compressed:
            data = first
            while data:
                original.update(data); size += len(data); compressed.write(data)
                data = stream.read(BLOCK)
    finally:
        writer.finish()
    manifest["artifact"] = {"role": "os-disk", "format": manifest["target"]["format"], "compression": "gzip",
                            "uncompressedBytes": size, "uncompressedSha256": original.hexdigest(),
                            "compressedBytes": writer.total, "compressedSha256": writer.whole.hexdigest(), "chunks": writer.entries}
    encoded = canonical(manifest)
    require(len(encoded) <= MAX_JSON, "manifest exceeds size limit")
    partial = out / "manifest.json.partial"
    partial.write_bytes(encoded); partial.rename(out / "manifest.json")
    return manifest


def validate_manifest(m):
    shape(m, ROOT)
    require(m["schemaVersion"] == "bridge-image/v1" and m["publicationReady"] is False, "unsupported manifest schema or publication claim")
    require(matches(m["imageId"], ID) and ".." not in m["imageId"] and matches(m["stateCompatibilityId"], ID), "invalid image/state ID")
    require(matches(m["buildSourceCommit"], r"[0-9a-f]{40}"), "invalid build commit")
    target_valid(m["target"]); components_valid(m["components"])
    require(canonical(m["layout"]) == canonical(layout(m["target"]["format"])), "invalid H1 layout")
    a = m["artifact"]
    shape(a, ("role", "format", "compression", "uncompressedBytes", "uncompressedSha256", "compressedBytes", "compressedSha256", "chunks"))
    require(a["role"] == "os-disk" and a["format"] == m["target"]["format"] and a["compression"] == "gzip", "invalid artifact role or format")
    for k in ("uncompressedBytes", "compressedBytes"):
        require(type(a[k]) is int and a[k] > 0, "invalid artifact size")
    for k in ("uncompressedSha256", "compressedSha256"):
        require(matches(a[k], HEX), "invalid artifact checksum")
    require(isinstance(a["chunks"], list) and 0 < len(a["chunks"]) <= 50000, "invalid chunks")
    total, previous = 0, None
    for i, c in enumerate(a["chunks"]):
        shape(c, ("order", "name", "bytes", "sha256"))
        require(type(c["order"]) is int and c["order"] == i and c["name"] == f"{prefix(m)}.part-{i:06d}", "invalid chunk name or order")
        require(type(c["bytes"]) is int and 0 < c["bytes"] <= MAX_CHUNK and matches(c["sha256"], HEX), "invalid chunk size/checksum")
        if previous is not None:
            require(previous == a["chunks"][0]["bytes"] and c["bytes"] <= previous, "invalid chunk boundary")
        previous = c["bytes"]; total += c["bytes"]
    require(total == a["compressedBytes"], "compressed size mismatch")


class ChunkReader(io.RawIOBase):
    def __init__(self, root, entries):
        self.root, self.entries, self.index, self.file = root, entries, 0, None
        self.whole, self.total = hashlib.sha256(), 0

    def readable(self):
        return True

    def readinto(self, buffer):
        while self.index < len(self.entries):
            c = self.entries[self.index]
            if self.file is None:
                path = self.root / c["name"]
                require(not path.is_symlink() and path.stat().st_size == c["bytes"], "missing, linked or wrong-sized chunk")
                self.file = path.open("rb"); self.remaining = c["bytes"]; self.part = hashlib.sha256()
            if self.remaining == 0:
                require(not self.file.read(1) and self.part.hexdigest() == c["sha256"], "chunk checksum/length mismatch")
                self.file.close(); self.file = None; self.index += 1
                continue
            n = self.file.readinto(memoryview(buffer)[:min(len(buffer), self.remaining)])
            require(n > 0, "truncated chunk")
            self.part.update(memoryview(buffer)[:n]); self.whole.update(memoryview(buffer)[:n])
            self.remaining -= n; self.total += n
            return n
        return 0

    def close(self):
        if self.file is not None: self.file.close()
        super().close()


def verify(manifest_path, *, manifest_sha256=None, out=None):
    manifest_path = Path(manifest_path)
    m = read_json(manifest_path, manifest_sha256)
    validate_manifest(m)
    a = m["artifact"]
    require({p.name for p in manifest_path.parent.iterdir()} == {manifest_path.name, *(c["name"] for c in a["chunks"])}, "missing or extra package files")
    destination, partial, output = None, None, None
    if out is not None:
        destination = Path(out)
        require(not destination.exists() and not destination.is_symlink(), "output already exists")
        require(destination.parent.resolve() != manifest_path.parent.resolve(), "output must be outside package directory")
        partial = destination.with_name(destination.name + ".partial"); output = partial.open("xb")
    reader = ChunkReader(manifest_path.parent, a["chunks"])
    original, size = hashlib.sha256(), 0
    try:
        with reader, io.BufferedReader(reader) as buffered, gzip.GzipFile(fileobj=buffered, mode="rb") as decompressed:
            while data := decompressed.read(BLOCK):
                size += len(data)
                require(size <= a["uncompressedBytes"], "uncompressed size exceeds manifest")
                original.update(data)
                if output is not None: output.write(data)
            require(reader.index == len(a["chunks"]) and reader.total == a["compressedBytes"] and reader.whole.hexdigest() == a["compressedSha256"], "compressed payload checksum mismatch")
        require(size == a["uncompressedBytes"] and original.hexdigest() == a["uncompressedSha256"], "uncompressed payload checksum mismatch")
    except (EOFError, zlib.error, gzip.BadGzipFile) as error:
        raise ValueError("invalid or truncated gzip payload") from error
    finally:
        if output is not None: output.close()
    if partial is not None:
        # Publish without replacing a path created by another writer during verification.
        os.link(partial, destination)
        partial.unlink()
    return m


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--input", type=Path); mode.add_argument("--verify", type=Path)
    parser.add_argument("--metadata", type=Path); parser.add_argument("--out", type=Path)
    parser.add_argument("--chunk-bytes", type=int, default=MAX_CHUNK)
    parser.add_argument("--manifest-sha256")
    args = parser.parse_args()
    try:
        if args.verify:
            require(args.metadata is None, "metadata is not a verify option")
            result = verify(args.verify, manifest_sha256=args.manifest_sha256, out=args.out)
        else:
            require(args.metadata is not None and args.out is not None and args.manifest_sha256 is None, "packaging requires metadata and a new output directory")
            require(not args.input.is_symlink(), "input symlink is not allowed")
            with args.input.open("rb") as stream:
                result = create(stream, read_json(args.metadata), args.out, args.chunk_bytes)
    except (ValueError, OSError, TypeError) as error:
        print("image package: " + str(error), file=sys.stderr)
        return 2
    print(f"verified bytes: {result['artifact']['compressedBytes']} compressed, {result['artifact']['uncompressedBytes']} uncompressed; not boot or publication readiness")
    return 0


if __name__ == "__main__":
    sys.exit(main())
