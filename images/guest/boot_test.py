"""Disposable Linux/KVM base proof, never a native or release qualification."""
import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path
from typing import TYPE_CHECKING

if __package__ or TYPE_CHECKING:
    from .build import STATE_BYTES, encoded, image_info, run
    from .lock import file_hash, load_lock, matches, require
    from .reference_host import control_worker
    from .test_seed import write_seed
else:
    from build import STATE_BYTES, encoded, image_info, run
    from lock import file_hash, load_lock, matches, require
    from reference_host import control_worker
    from test_seed import write_seed

FIXTURES = Path(__file__).with_name("tests") / "fixtures"
FIRMWARE = Path("/usr/share/OVMF")


def option_path(path):
    return str(path).replace(",", ",,")


def qemu_argv(root, installation_id, include_state):
    mac = "02:" + ":".join(installation_id.replace("-", "")[i:i + 2] for i in range(2, 12, 2))
    args = ["qemu-system-x86_64", "-accel", "kvm", "-machine", "pc-q35-7.2", "-cpu", "host", "-smp", "4", "-m", "6144",
            "-nodefaults", "-display", "none", "-monitor", "none", "-no-reboot", "-uuid", installation_id, "-rtc", "base=utc",
            "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=" + str(FIRMWARE / "OVMF_CODE_4M.fd"),
            "-drive", "if=pflash,format=raw,unit=1,file=" + option_path(root / "efi.fd"),
            "-drive", "if=none,id=os,format=qcow2,cache=writeback,file=" + option_path(root / "os.qcow2"),
            "-device", "virtio-blk-pci,drive=os,serial=bridge-os",
            "-drive", "if=none,id=test,format=raw,readonly=on,file=" + option_path(root / "seed.iso"),
            "-device", "virtio-blk-pci,drive=test,serial=bridge-test",
            "-device", "virtio-rng-pci", "-netdev", "user,id=net,restrict=on", "-device", "virtio-net-pci,netdev=net,mac=" + mac,
            "-device", "virtio-serial-pci", "-chardev", "socket,id=control,path=" + option_path(root / "control.sock") + ",server=on,wait=off",
            "-device", "virtserialport,chardev=control,name=ai.orka.bridge.control", "-chardev", "file,id=diagnostics,path=" + option_path(root / "serial.log"), "-serial", "chardev:diagnostics"]
    if include_state:
        args += ["-drive", "if=none,id=state,format=qcow2,cache=writeback,file=" + option_path(root / "state.qcow2"),
                 "-device", "virtio-blk-pci,drive=state,serial=bridge-state"]
    return args


def parse_result(serial, installation_id):
    lines = [line[len("BRIDGE_BASE "):] for line in serial.splitlines() if line.startswith("BRIDGE_BASE ")]
    require(len(lines) == 1, "missing or duplicate base result")
    result = json.loads(lines[0])
    require(result.get("schemaVersion") == "bridge-guest-base/v1" and result.get("installationId") == installation_id and result.get("stateUuid") == installation_id, "base installation mismatch")
    require(all(result.get(k) is True for k in ("sentinel", "stateBeforeDaemons", "machineIdBeforeSystemd", "controlPort")), "base assertions failed")
    require(result.get("credentialsType") == "tmpfs" and result.get("dockerRoot") == "/var/lib/bridge/docker" and result.get("containerdRoot") == "/var/lib/bridge/containerd", "base storage mismatch")
    require(matches(result.get("machineId"), r"[0-9a-f]{32}"), "invalid machine identity")
    return result


def instrument(image, case_name='base'):
    commands = []
    for name, target in (("reference-seed", "/etc/initramfs-tools/scripts/local-bottom/00-bridge-test"),
                         ("reference-check", "/usr/lib/bridge/reference-check"),
                         ("reference-check.service", "/etc/systemd/system/reference-check.service")):
        commands += ["--upload", str(FIXTURES / name) + ":" + target]
    if case_name != 'base':
        for name, target in (('infrastructure-check', '/usr/lib/bridge/infrastructure-check'), ('infrastructure-check.service', '/etc/systemd/system/reference-check.service')):
            commands += ['--upload', str(FIXTURES / name) + ':' + target]
    if case_name == 'offline-tasks':
        commands += ['--upload', str(FIXTURES / 'task-check.py') + ':/usr/lib/bridge/task-check.py']
    extra = 'chmod 755 /usr/lib/bridge/infrastructure-check; ' if case_name != 'base' else ''
    if case_name in ('base', 'interrupted'):
        extra += 'ln -sf /dev/null /etc/systemd/system/bridge-infrastructure.service; '
    run(["virt-customize", "--no-network", "-a", image, *commands, "--run-command",
         extra + "chmod 755 /etc/initramfs-tools/scripts/local-bottom/00-bridge-test /usr/lib/bridge/reference-check; chmod 644 /etc/systemd/system/reference-check.service; systemctl enable reference-check.service; update-initramfs -u -k all"])


def boot_case(factory, root, installation_id, mode, state=None, missing=False, copy_state=False, *, case_name='base', fixture=None, prior=None):
    require(mode != "new" or state is None and not missing, "new authority requires exclusive state creation")
    root.mkdir()
    run(['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', factory.resolve(), root / 'os.qcow2'])
    instrument(root / "os.qcow2", case_name)
    if state is None and not missing:
        run(["qemu-img", "create", "-f", "qcow2", root / "state.qcow2", str(STATE_BYTES)])
    elif state is not None:
        if copy_state:
            run(["qemu-img", "convert", "-O", "qcow2", state, root / "state.qcow2"])
        else:
            # Same stopped state inode across OS replacement, not a restored copy.
            (root / "state.qcow2").symlink_to(state.resolve())
    seed = write_seed(root / "seed.iso", installation_id, mode, case_name=case_name, fixture=fixture)
    efi_source = state.parent / "efi.fd" if state is not None else FIRMWARE / "OVMF_VARS_4M.fd"
    if state is not None and not copy_state:
        (root / "efi.fd").symlink_to(efi_source.resolve())
    else:
        shutil.copyfile(efi_source, root / "efi.fd")
    hooks = ['reference-seed', 'reference-check', 'reference-check.service']
    if case_name != 'base':
        hooks += ['infrastructure-check', 'infrastructure-check.service']
    if case_name == 'offline-tasks':
        hooks += ['task-check.py']
    before = {"baseSha256": file_hash(factory), "instrumentedSha256": file_hash(root / "os.qcow2"),
              "seedSha256": file_hash(root / "seed.iso"), "seed": seed, "efiBeforeSha256": file_hash(root / "efi.fd"),
              "testHookSha256": {p: file_hash(FIXTURES / p) for p in hooks},
              'runnerSha256': {p: file_hash(Path(__file__).with_name(p)) for p in ('boot_test.py', 'reference_host.py', 'test_seed.py')}}
    start = time.monotonic()
    boot = {"installationId": installation_id, "generation": 1, "bootNonce": str(uuid.uuid4()), "stateCreation": mode,
            "stateFsUuid": installation_id, "stateBytes": STATE_BYTES, "hostUtc": int(time.time())}
    frames, errors, stop = [], [], threading.Event()
    timeout = 180 if case_name == 'base' else 900
    argv = qemu_argv(root, installation_id, not missing)
    if case_name != 'base':
        argv += ['-object', 'filter-dump,id=trace,netdev=net,file=' + option_path(root / 'network.pcap')]
    if prior is not None:
        argv += ['-drive', 'if=none,id=prior,format=qcow2,readonly=on,file=' + option_path(prior.resolve()), '-device', 'virtio-blk-pci,drive=prior,serial=bridge-prior']
    with (root / "qemu.log").open("w") as log, subprocess.Popen(argv, stdout=log, stderr=log) as process:
        worker = threading.Thread(target=control_worker, args=(root, boot, process, stop, frames, errors, timeout), daemon=True)
        worker.start()
        try:
            require(process.wait(timeout=timeout) == 0, "QEMU reference VM failed")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            stop.set()
            worker.join(2)
        require(not worker.is_alive() and not errors, "runtime host exchange failed: " + "; ".join(errors))
    before["runtimeFrames"] = frames
    before["bootDescriptor"] = boot
    serial = root / "serial.log"
    require(serial.stat().st_size <= 4 * 1024**2, "serial output exceeds bound")
    text = serial.read_text(errors="replace")
    metrics = {name: {"fileBytes": (root / name).stat().st_size, "allocatedBytes": (root / name).stat().st_blocks * 512}
               for name in ("os.qcow2", "efi.fd", "seed.iso", "state.qcow2") if (root / name).exists()}
    receipt = {**before, "elapsedSeconds": time.monotonic() - start, "diskBytes": metrics}
    (root / "case.json").write_bytes(encoded(receipt) + b"\n")
    return {**receipt, "serial": text}


def base(image, lock_path, out):
    lock = load_lock(lock_path)
    require(lock["architecture"] == "amd64", "this reference runner requires amd64 KVM")
    require(Path("/dev/kvm").exists(), "KVM is required")
    require(not out.exists() and not out.is_symlink(), "output must not already exist")
    build = json.loads(image.with_name("build.json").read_text())
    require(isinstance(build, dict) and build.get("schemaVersion") == "bridge-guest-build/v1" and build.get("architecture") == lock["architecture"], "unsupported build record")
    require(build.get("sha256") == file_hash(image), "factory hash mismatch")
    require(build.get("inputSha256") == hashlib.sha256(encoded(lock)).hexdigest(), "input lock mismatch")
    require(image_info(image)["virtual-size"] == 16 * 1024**3, "wrong OS capacity")
    out.mkdir()
    installation = str(uuid.uuid4())
    first = boot_case(image, out / "new", installation, "new")
    a = parse_result(first.pop("serial"), installation)
    second = boot_case(image, out / "replacement", installation, "existing", out / "new/state.qcow2")
    b = parse_result(second.pop("serial"), installation)
    require(a["machineId"] == b["machineId"], "OS replacement changed machine identity")
    for case in (first, second):
        require(len(case["runtimeFrames"]) == 6 and case["runtimeFrames"][-1]["body"]["body"]["op"] == "ready", "native runtime handoff incomplete")
    absent = boot_case(image, out / "missing", installation, "existing", missing=True)
    missing_serial = absent.pop("serial")
    require("BRIDGE_EARLY_ERROR native endpoint refused" in missing_serial and "systemd[1]" not in missing_serial and "BRIDGE_BASE" not in missing_serial, "missing-state case did not refuse before services")
    mismatch = boot_case(image, out / "mismatch", str(uuid.uuid4()), "existing", out / "replacement/state.qcow2", copy_state=True)
    mismatch_serial = mismatch.pop("serial")
    require("BRIDGE_EARLY_ERROR native endpoint refused" in mismatch_serial and "systemd[1]" not in mismatch_serial and "BRIDGE_BASE" not in mismatch_serial, "UUID mismatch did not refuse before services")
    run(["qemu-img", "compare", out / "replacement/state.qcow2", out / "mismatch/state.qcow2"])
    blank = boot_case(image, out / "blank-existing", installation, "existing")
    blank_serial = blank.pop("serial")
    require("BRIDGE_EARLY_ERROR native endpoint refused" in blank_serial and "systemd[1]" not in blank_serial, "blank existing state did not refuse")
    empty = out / "expected-empty.qcow2"
    run(["qemu-img", "create", "-f", "qcow2", empty, str(STATE_BYTES)])
    run(["qemu-img", "compare", empty, out / "blank-existing/state.qcow2"])
    for case in (absent, mismatch, blank):
        require(len(case["runtimeFrames"]) == 2 and case["runtimeFrames"][-1]["body"]["body"] == {"op": "error", "body": {"code": "incompatibleState"}}, "native early refusal missing")
    result = {"schemaVersion": "bridge-guest-boot/v1", "case": "base", "backend": "qemu-kvm", "qualificationOnly": True,
              "publicationReady": False, "build": build, "new": {**first, "result": a}, "replacement": {**second, "result": b},
              "missingStateRefused": True, "uuidMismatchRefusedWithoutMutation": True, "missing": absent, "mismatch": mismatch,
              "blankExistingRefusedWithoutMutation": True, "blank": blank,
              "firmware": {name: file_hash(FIRMWARE / name) for name in ("OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd")},
              "qemuVersion": run(["qemu-system-x86_64", "--version"]), "machine": "pc-q35-7.2"}
    pending = out / "boot.json.partial"
    pending.write_bytes(encoded(result) + b"\n")
    pending.replace(out / "boot.json")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=['base', 'infrastructure', 'offline-tasks', 'restart', 'interrupted'], required=True)
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--lock", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument('--fixture', type=Path)
    parser.add_argument('--from-receipt', type=Path)
    args = parser.parse_args()
    try:
        if args.case == 'base':
            base(args.image.resolve(), args.lock.resolve(), args.out.absolute())
        else:
            if __package__ or TYPE_CHECKING:
                from .infrastructure_test import infrastructure
            else:
                from infrastructure_test import infrastructure
            infrastructure(args.image.resolve(), args.lock.resolve(), args.out.absolute(), args.case, args.fixture, args.from_receipt)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print("guest base: " + str(error) + "\n" + (getattr(error, "stderr", "") or ""), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
