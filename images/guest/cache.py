"""Collect a verified local OCI closure. No registry I/O, boot or publication."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile

if __package__:
    from .lock import checked_file, load_lock, matches, read_json, require, validate_lock, DIGEST
else:
    from lock import checked_file, load_lock, matches, read_json, require, validate_lock, DIGEST

JSON_LIMIT = 4 * 1024 * 1024
MANIFESTS = {"application/vnd.oci.image.manifest.v1+json", "application/vnd.docker.distribution.manifest.v2+json"}
CONFIGS = {"application/vnd.oci.image.config.v1+json", "application/vnd.docker.container.image.v1+json"}
LAYERS = {"application/vnd.oci.image.layer.v1.tar", "application/vnd.oci.image.layer.v1.tar+gzip", "application/vnd.oci.image.layer.v1.tar+zstd", "application/vnd.docker.image.rootfs.diff.tar.gzip"}


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def small_json(tar, member):
    require(member.size <= JSON_LIMIT, "OCI JSON exceeds size limit")
    with tar.extractfile(member) as stream:
        return read_json(stream.read(JSON_LIMIT + 1))


def inspect_archive(component: dict, source: Path, architecture: str) -> dict:
    path = checked_file(source, component["archive"], component["archiveSha256"])
    if "indexDigest" in component:
        index_path = checked_file(source, component["indexPath"], component["indexDigest"][7:])
        require(index_path.stat().st_size <= JSON_LIMIT, "release index exceeds size limit")
        index = read_json(index_path.read_bytes())
        require(isinstance(index, dict) and isinstance(index.get("manifests"), list), "invalid release index")
        selected = []
        for descriptor in index["manifests"]:
            require(isinstance(descriptor, dict), "invalid release index descriptor")
            platform = descriptor.get("platform")
            if isinstance(platform, dict) and descriptor.get("digest") == component["manifestDigest"] and platform.get("os") == "linux" and platform.get("architecture") == architecture:
                selected.append(descriptor)
        require(len(selected) == 1, "release index platform membership mismatch")
    try:
        with tarfile.open(path, "r:*") as tar:
            members = {}
            for member in tar:
                name = member.name
                if member.isdir() and name in ("blobs", "blobs/sha256"):
                    continue
                require(member.isfile(), "non-file OCI archive member")
                require(name in ("oci-layout", "index.json") or matches(name, r"blobs/sha256/[0-9a-f]{64}"), "unsafe or extra OCI archive member")
                require(name not in members, "duplicate OCI archive member")
                members[name] = member
            require({"oci-layout", "index.json"} <= members.keys(), "missing OCI layout or index")
            require(small_json(tar, members["oci-layout"]) == {"imageLayoutVersion": "1.0.0"}, "unsupported OCI layout")
            index = small_json(tar, members["index.json"])
            roots = index.get("manifests", [])
            require(index.get("schemaVersion") == 2 and len(roots) == 1, "expected one selected platform manifest")
            root = roots[0]
            require(root.get("digest") == component["manifestDigest"] and root.get("mediaType") in MANIFESTS, "manifest identity mismatch")
            used = {"oci-layout", "index.json"}

            def blob(descriptor):
                digest = descriptor.get("digest")
                require(matches(digest, DIGEST), "invalid OCI descriptor digest")
                name = "blobs/sha256/" + digest[7:]
                require(name in members, "missing OCI blob")
                member = members[name]
                require(type(descriptor.get("size")) is int and descriptor["size"] == member.size, "OCI descriptor size mismatch")
                with tar.extractfile(member) as stream:
                    actual = hashlib.file_digest(stream, "sha256").hexdigest()
                require(actual == digest[7:], "OCI blob hash mismatch")
                used.add(name)
                return member

            manifest = small_json(tar, blob(root))
            require(manifest.get("schemaVersion") == 2, "unsupported image manifest")
            config = manifest.get("config", {})
            require(config.get("digest") == component["configDigest"] and config.get("mediaType") in CONFIGS, "config identity mismatch")
            platform = small_json(tar, blob(config))
            require(platform.get("architecture") == architecture and platform.get("os") == "linux", "OCI config platform mismatch")
            layers = manifest.get("layers")
            require(isinstance(layers, list), "missing image layers")
            for layer in layers:
                require(layer.get("mediaType") in LAYERS, "unsupported layer type")
                blob(layer)
            require(used == members.keys(), "extra unreferenced OCI blob")
            return {"blobs": sorted(used - {"oci-layout", "index.json"}), "descriptor": root}
    except (tarfile.TarError, EOFError, KeyError, AttributeError, TypeError) as error:
        raise ValueError("invalid OCI archive: " + component["name"]) from error


def write_archive(component, inspected, source, output, reference):
    # Rewrite only the root index annotation; the selected manifest/config/layers
    # retain their original bytes and digests, including the original repository.
    root = {key: inspected["descriptor"][key] for key in ("mediaType", "digest", "size")}
    root["annotations"] = {"org.opencontainers.image.ref.name": reference}
    index = encoded({"schemaVersion": 2, "manifests": [root]})
    with tarfile.open(checked_file(source, component["archive"], component["archiveSha256"]), "r:*") as incoming, tarfile.open(output, "w", format=tarfile.PAX_FORMAT) as outgoing:
        for name in ["oci-layout", "index.json"] + inspected["blobs"]:
            if name in ("oci-layout", "index.json"):
                data = encoded({"imageLayoutVersion": "1.0.0"}) if name == "oci-layout" else index
                stream, size = io.BytesIO(data), len(data)
            else:
                member = incoming.getmember(name)
                stream, size = incoming.extractfile(member), member.size
            with stream:
                info = tarfile.TarInfo(name)
                info.size, info.mode = size, 0o644
                outgoing.addfile(info, stream)


def collect(lock: dict, out: Path, *, source: Path) -> None:
    validate_lock(lock, production=False)
    source = source.resolve(strict=True)
    require(not out.exists() and not out.is_symlink(), "output must not already exist")
    # Verify everything before reserving any output. An interrupted write has no
    # collection.json; only its final appearance denotes a completed collection.
    chart = checked_file(source, lock["orka"]["chartPath"], lock["orka"]["chartSha256"])
    artifacts = [(group, item, checked_file(source, item["path"], item["sha256"])) for group in ("packages", "tools") for item in lock[group]]
    inspected = [inspect_archive(c, source, lock["architecture"]) for c in lock["components"]]
    out.mkdir()
    (out / "components").mkdir()
    (out / "artifacts").mkdir()
    copied = {}
    for group, item, path in artifacts:
        target = "artifacts/" + item["sha256"]
        shutil.copyfile(path, out / target)
        checked_file(out, target, item["sha256"])
        copied.setdefault(group, []).append({**item, "path": target})
    shutil.copyfile(chart, out / "chart.tgz")
    checked_file(out, "chart.tgz", lock["orka"]["chartSha256"])
    components = []
    for component, inspection in zip(lock["components"], inspected):
        reference = lock["nodeLocalRef"] if component["name"] == "kind-node" else component["repository"] + "@" + component["manifestDigest"]
        archive = "components/" + component["name"] + ".oci.tar"
        write_archive(component, inspection, source, out / archive, reference)
        with (out / archive).open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        record = {**component, "archive": archive, "archiveSha256": digest, "reference": reference}
        # Copy index proof without retaining host input paths in the receipt.
        if "indexPath" in record:
            target = "artifacts/" + component["indexDigest"][7:]
            shutil.copyfile(checked_file(source, component["indexPath"], component["indexDigest"][7:]), out / target)
            record["indexPath"] = target
        inspect_archive(record, out, lock["architecture"])
        components.append(record)
    normalized = {**lock, "orka": {**lock["orka"], "chartPath": "chart.tgz"}, **copied, "components": [{k: v for k, v in c.items() if k != "reference"} for c in components]}
    validate_lock(normalized, production=False)
    normalized_bytes = encoded(normalized) + b"\n"
    (out / "inputs.json").write_bytes(normalized_bytes)
    receipt = {"normalizedLockSha256": hashlib.sha256(normalized_bytes).hexdigest(), "schemaVersion": "bridge-guest-collection/v1", "inputSha256": hashlib.sha256(encoded(lock)).hexdigest(), "architecture": lock["architecture"], "sourceCommit": lock["sourceCommit"], "orka": {**lock["orka"], "chartPath": "chart.tgz"}, "kmxProfileSha256": lock["kmxProfileSha256"], "nodeLocalRef": lock["nodeLocalRef"], "components": components, **copied, "publicationReady": False}
    pending = out / "collection.json.partial"
    pending.write_bytes(encoded(receipt) + b"\n")
    pending.replace(out / "collection.json")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        collect(load_lock(args.lock), args.out, source=args.lock.resolve().parent)
    except (ValueError, OSError, TypeError) as error:
        print("guest cache: " + str(error), file=sys.stderr)
        return 2
    print("local closure collected; no boot or publication qualification")
    return 0


if __name__ == "__main__":
    sys.exit(main())
