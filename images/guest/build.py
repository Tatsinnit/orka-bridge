"""Assemble an uninitialized OS offline; reference instrumentation is separate."""
import argparse
import hashlib
import json
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING

if __package__ or TYPE_CHECKING:
    from .lock import checked_file, file_hash, load_lock, require, validate_lock
else:
    from lock import checked_file, file_hash, load_lock, require, validate_lock

OS_BYTES, STATE_BYTES = 16 * 1024**3, 32 * 1024**3
ROOTFS_FILES = {
    "/usr/lib/bridge/check-state": "check-state",
    "/usr/lib/bridge/bridge-early-state": "bridge-early-state",
    "/etc/initramfs-tools/hooks/bridge-state": "initramfs/hooks/bridge-state",
    "/etc/initramfs-tools/scripts/local-bottom/bridge-state": "initramfs/scripts/local-bottom/bridge-state",
    "/etc/systemd/system/bridge-state.service": "bridge-state.service",
    "/etc/systemd/system/docker.service.d/bridge-state.conf": "daemon-state.conf",
    "/etc/systemd/system/containerd.service.d/bridge-state.conf": "daemon-state.conf",
    "/etc/docker/daemon.json": "daemon.json",
    "/etc/containerd/config.toml": "containerd.toml",
    "/etc/udev/rules.d/61-bridge-control.rules": "61-bridge-control.rules",
    "/etc/default/grub": "grub",
    "/etc/systemd/network/80-bridge.network": "80-bridge.network",
    "/etc/systemd/system.conf.d/bridge.conf": "system.conf",
}


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def disk_contract(architecture):
    require(architecture in ("amd64", "arm64"), "unsupported architecture")
    fmt = "raw" if architecture == "arm64" else "qcow2"
    return {"contractVersion": "H1/v1", "controlPort": "ai.orka.bridge.control",
            "osDisk": {"role": "os-disk", "format": fmt, "virtualBytes": OS_BYTES, "blockSerial": "bridge-os", "privateWritableCopy": True},
            "stateDisk": {"role": "state-disk", "creation": "create-empty", "format": fmt, "virtualBytes": STATE_BYTES, "filesystem": "ext4", "blockSerial": "bridge-state", "formatOnNewOnly": True}}


def verify_os_inputs(lock, *, source):
    validate_lock(lock, production=False)
    files = {group: {a["name"]: checked_file(source, a["path"], a["sha256"]) for a in lock[group]} for group in ("packages", "tools")}
    return {"base": files["packages"]["debian-base"], **files, "contract": disk_contract(lock["architecture"]),
            "componentCacheVerified": False, "publicationReady": False}


def stage_rootfs(lock, *, source, out):
    require(not out.exists() and not out.is_symlink(), "output must not already exist")
    inputs = verify_os_inputs(lock, source=source)
    out.mkdir()
    hashes = {}
    for dest, name in ROOTFS_FILES.items():
        target = out / dest.lstrip("/")
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((Path(__file__).with_name("rootfs") / name).read_bytes())
        target.chmod(0o755 if name in ("check-state", "bridge-early-state") or name.startswith("initramfs/") else 0o644)
        hashes[dest] = file_hash(target)
    target = out / "usr/local/bin/kind"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(inputs["tools"]["kind"], target)
    target.chmod(0o755)
    expected = next(t["sha256"] for t in lock["tools"] if t["name"] == "kind")
    require(file_hash(target) == expected, "kind changed during staging")
    return {"rootfsStaged": True, "diskBuilt": False, "bootVerified": False, "publicationReady": False, "files": hashes}


def run(argv):
    return subprocess.run([str(x) for x in argv], check=True, capture_output=True, text=True).stdout.strip()


def image_info(path):
    info = json.loads(run(["qemu-img", "info", "--output=json", path]))
    require(info["format"] in ("raw", "qcow2"), "unsupported disk format")
    require(not info.get("backing-filename") and not info.get("full-backing-filename"), "backing file is not allowed")
    require(not info.get("format-specific", {}).get("data", {}).get("data-file"), "external data file is not allowed")
    return info


def rootfs_archive(overlay, target):
    # Never carry a non-root builder's uid or restrictive umask into /etc,/usr.
    with tarfile.open(target, "w", format=tarfile.PAX_FORMAT) as tar:
        for path in sorted(overlay.rglob("*")):
            info = tar.gettarinfo(str(path), arcname=str(path.relative_to(overlay)))
            info.uid = info.gid = info.mtime = 0
            info.uname = info.gname = ""
            if info.isdir():
                info.mode = 0o755
                tar.addfile(info)
            else:
                require(info.isfile(), "unexpected rootfs entry")
                with path.open("rb") as stream:
                    tar.addfile(info, stream)


def root_esp(filesystems, root):
    found = dict(line.split(": ", 1) for line in filesystems)
    esp = [path for path, kind in found.items() if kind == "vfat"]
    require(len(esp) == 1, "expected one EFI system partition")
    require(found.get(root) == "ext4", "selected Debian root must be ext4")
    require({path for path, kind in found.items() if kind != "unknown"} == {root, esp[0]}, "extra base filesystems are unsupported")
    return esp[0]


def stage_package(source, target, expected):
    shutil.copyfile(source, target)
    require(file_hash(target) == expected, "package changed during staging")


def customize_commands(packages, architecture):
    target = "x86_64-efi" if architecture == "amd64" else "arm64-efi"
    commands = ["mkdir /var/cache/bridge-packages"]
    commands += ["upload " + str(p) + ":/var/cache/bridge-packages/" + p.name for p in packages]
    commands += [
        "run-command printf '#!/bin/sh\\nexit 101\\n' > /usr/sbin/policy-rc.d; chmod 755 /usr/sbin/policy-rc.d",
        "run-command DEBIAN_FRONTEND=noninteractive dpkg --force-confold -i /var/cache/bridge-packages/*.deb",
        "run-command rm /usr/sbin/policy-rc.d; rm -rf /var/cache/bridge-packages",
        "run-command systemctl mask cloud-init.service cloud-init-local.service cloud-config.service cloud-final.service ssh.service ssh.socket serial-getty@.service getty@.service swap.target systemd-hibernate.service systemd-hybrid-sleep.service systemd-coredump.socket systemd-machine-id-commit.service systemd-suspend-then-hibernate.service",
        "run-command sed -i '/[[:space:]]swap[[:space:]]/d' /etc/fstab; : > /etc/machine-id; rm -f /var/lib/dbus/machine-id /var/lib/systemd/random-seed /etc/ssh/ssh_host_*",
        "touch /etc/growroot-disabled",
        "run-command update-initramfs -u -k all",
        "run-command grub-install --target=" + target + " --efi-directory=/boot/efi --removable --no-nvram",
        "run-command update-grub",
    ]
    return commands


def build(lock, out, *, source):
    out = out.absolute()
    inputs = verify_os_inputs(lock, source=source)
    require({"x86_64": "amd64", "aarch64": "arm64"}.get(platform.machine()) == lock["architecture"], "native architecture builder required")
    require(not out.exists() and not out.is_symlink(), "output must not already exist")
    selected = {t["name"]: t for t in lock["tools"]}
    builder_tools = {}
    for name in ("qemu-img", "guestfish", "virt-resize", "virt-customize"):
        found = shutil.which(name)
        if name not in selected or found is None:
            raise ValueError("missing pinned builder tool: " + name)
        executable = Path(found)
        require(file_hash(executable) == selected[name]["sha256"], "builder tool checksum mismatch: " + name)
        version = run([name, "--version"])
        require(version == selected[name]["version"], "builder tool version mismatch: " + name)
        builder_tools[name] = {"sha256": selected[name]["sha256"], "version": version}
    expected_packages = {p["name"]: p["sha256"] for p in lock["packages"]}
    out.mkdir()
    snapshot = out / "base.input"
    stage_package(inputs["base"], snapshot, expected_packages["debian-base"])
    snapshot.chmod(0o444)
    inputs["base"] = snapshot
    base_info = image_info(snapshot)
    require(base_info["virtual-size"] <= OS_BYTES, "base exceeds OS capacity")
    # Reject initialized installations rather than attempting to sanitize one.
    for path in ("/etc/kubernetes", "/var/lib/kubelet", "/var/lib/bridge"):
        require(run(["guestfish", "--ro", "-a", inputs["base"], "-i", "is-dir", path]) == "false", "base contains installation state")
    root = run(["guestfish", "--ro", "-a", inputs["base"], "run", ":", "inspect-os"]).splitlines()
    require(len(root) == 1 and root[0].startswith("/dev/"), "expected one Linux root filesystem")
    with tempfile.TemporaryDirectory(prefix="bridge-assembly-") as temp:
        stage = Path(temp)
        overlay = stage_rootfs(lock, source=source, out=stage / "rootfs")
        rootfs_archive(stage / "rootfs", stage / "rootfs.tar")
        packages = []
        for index, (name, path) in enumerate(inputs["packages"].items()):
            if name != "debian-base":
                copied = stage / ("package-" + str(index) + ".deb")
                stage_package(path, copied, expected_packages[name])
                packages.append(copied)
        require(packages, "offline OS package closure is required")
        raw = out / "os.raw"
        run(["qemu-img", "create", "-f", "raw", raw, str(OS_BYTES)])
        run(["virt-resize", "--format", base_info["format"], "--output-format", "raw", "--expand", root[0], inputs["base"], raw])
        # virt-resize can renumber GPT slots and replace PARTUUIDs. Bind the
        # guest's fstab to copied filesystem UUIDs, including its actual ESP.
        new_root = run(["guestfish", "--ro", "-a", raw, "run", ":", "inspect-os"])
        filesystems = run(["guestfish", "--ro", "-a", raw, "run", ":", "list-filesystems"]).splitlines()
        esp = root_esp(filesystems, new_root)
        root_uuid = run(["guestfish", "--ro", "-a", raw, "run", ":", "vfs-uuid", new_root])
        esp_uuid = run(["guestfish", "--ro", "-a", raw, "run", ":", "vfs-uuid", esp])
        fstab = "UUID=" + root_uuid + " / ext4 defaults 0 1\\nUUID=" + esp_uuid + " /boot/efi vfat defaults 0 0\\n"
        run(["guestfish", "-a", raw, "-m", new_root, "write", "/etc/fstab", fstab.replace("\\n", "\n")])
        run(["guestfish", "-a", raw, "-i", "tar-in", stage / "rootfs.tar", "/"])
        commands = stage / "commands"
        commands.write_text("\n".join(customize_commands(packages, lock["architecture"])) + "\n")
        run(["virt-customize", "--no-network", "--format", "raw", "-a", raw, "--commands-from-file", commands])
    efi = "BOOTAA64.EFI" if lock["architecture"] == "arm64" else "BOOTX64.EFI"
    require(run(["guestfish", "--ro", "-a", raw, "-i", "is-file", "/boot/efi/EFI/BOOT/" + efi]) == "true", "missing removable EFI loader")
    payload = raw
    if lock["architecture"] == "amd64":
        payload = out / "os.qcow2"
        run(["qemu-img", "convert", "-f", "raw", "-O", "qcow2", raw, payload])
        raw.unlink()
    require(image_info(payload)["virtual-size"] == OS_BYTES, "wrong OS capacity")
    snapshot.unlink()
    result = {"schemaVersion": "bridge-guest-build/v1", "architecture": lock["architecture"], "contract": inputs["contract"],
              "inputSha256": hashlib.sha256(encoded(lock)).hexdigest(), "builderTools": builder_tools,
              "recipeFiles": {"build.py": file_hash(Path(__file__)), "lock.py": file_hash(Path(__file__).with_name("lock.py")), **overlay["files"]},
              "payload": payload.name, "sha256": file_hash(payload), "fileBytes": payload.stat().st_size,
              "allocatedBytes": payload.stat().st_blocks * 512, "componentCacheVerified": False, "bootVerified": False, "publicationReady": False}
    pending = out / "build.json.partial"
    pending.write_bytes(encoded(result) + b"\n")
    pending.replace(out / "build.json")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    try:
        build(load_lock(args.lock), args.out, source=args.lock.resolve().parent)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print("guest build: " + str(error) + "\n" + (getattr(error, "stderr", "") or ""), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
