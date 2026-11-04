"""Image-owned service guards, not the runtime's early formatter or protocol."""
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOTFS = Path(__file__).resolve().parents[1] / "rootfs"


class StateGuardTests(unittest.TestCase):
    def file(self, name):
        path = ROOTFS / name
        self.assertTrue(path.is_file(), "missing image integration file: " + name)
        return path

    def run_guard(self, *, filesystem="ext4", credentials="tmpfs", state_uuid="install", machine_uuid="install", machine_mount="/etc/machine-id", descriptor=True, expected_uuid="install", legacy_uuid="install", bind_source="1:2", stat_fail=False):
        script = self.file("check-state")
        with tempfile.TemporaryDirectory() as temp:
            trace = Path(temp) / "trace"
            # Replace only the OS-command boundary in a disposable shell. These
            # functions and values never enter the installed image/service.
            wrapper = r'''
jq() { printf '%s\n' "$EXPECTED_UUID"; }
stat() { [ "$STAT_FAIL" = no ] || return 1; case "$*" in *'/var/lib/docker') printf '%s\n' "$BIND_SOURCE";; *) printf '%s\n' '1:2';; esac; }
findmnt() {
    printf '%s\n' "$*" >> "$TRACE"
    case "$*" in
      *'/var/lib/bridge -o FSTYPE') printf '%s\n' "$STATE_FS" ;;
      *'/var/lib/bridge -o UUID') printf '%s\n' "$STATE_UUID" ;;
      *'/run/bridge/credentials -o FSTYPE') printf '%s\n' "$CREDENTIAL_FS" ;;
      *'/var/lib/docker -o UUID'|*'/var/lib/containerd -o UUID') printf '%s\n' "$LEGACY_UUID" ;;
      *'/etc/machine-id -o UUID') printf '%s\n' "$MACHINE_UUID" ;;
      *'/etc/machine-id -o TARGET') printf '%s\n' "$MACHINE_MOUNT" ;;
      *) return 2 ;;
    esac
}
test() {
    case "$*" in
      '-s /run/bridge/boot.json') command test "$DESCRIPTOR" = yes ;;
      *) command test "$@" ;;
    esac
}
. "$GUARD"
'''
            environment = {"GUARD": str(script), "TRACE": str(trace), "STATE_FS": filesystem,
                           "STATE_UUID": state_uuid, "CREDENTIAL_FS": credentials,
                           "MACHINE_UUID": machine_uuid, "MACHINE_MOUNT": machine_mount,
                           "DESCRIPTOR": "yes" if descriptor else "no", "EXPECTED_UUID": expected_uuid, "LEGACY_UUID": legacy_uuid, "BIND_SOURCE": bind_source, "STAT_FAIL": "yes" if stat_fail else "no"}
            result = subprocess.run(["/bin/sh", "-c", wrapper], env=environment, capture_output=True, text=True, check=False)
            calls = trace.read_text() if trace.exists() else ""
            self.assertNotIn("mkfs", calls)
            self.assertFalse(any(line.startswith("docker ") for line in calls.splitlines()))
            return result

    def test_matched_state_identity_and_tmpfs_pass_guard(self):
        result = self.run_guard()
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertEqual("", result.stdout)

    def test_per_start_guard_matches_accepted_installation(self):
        result = self.run_guard(expected_uuid="other")
        self.assertNotEqual(0, result.returncode)

    def test_per_start_guard_requires_legacy_path_binds(self):
        result = self.run_guard(legacy_uuid="root-os")
        self.assertNotEqual(0, result.returncode)

    def test_failed_identity_reads_block_services(self):
        self.assertNotEqual(0, self.run_guard(stat_fail=True).returncode)

    def test_wrong_same_uuid_bind_source_blocks_services(self):
        self.assertNotEqual(0, self.run_guard(bind_source="9:9").returncode)

    def test_runtime_owns_persisted_identity_validation(self):
        script = self.file("initramfs/scripts/local-bottom/bridge-state").read_text()
        self.assertIn('/usr/lib/bridge/bridge-guest --early || fail', script)
        self.assertNotIn("mke2fs", script)
        self.assertNotIn("mkfs", script)

    def test_terminal_failure_stays_closed_if_logging_or_shutdown_fail(self):
        handler = next(line for line in self.file("initramfs/scripts/local-bottom/bridge-state").read_text().splitlines() if line.startswith("fail()"))
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for broken_console in (True, False):
                trace = root / "wait"
                if trace.exists():
                    trace.unlink()
                console = root if broken_console else root / "console"
                isolated = handler.replace("/dev/console", str(console))
                # Only this terminal function runs: no device selection, mounts,
                # formatter or real shutdown command from the early hook.
                program = 'set -eu\npoweroff() { return 1; }\nsleep() { echo waiting > "$1"; exit 99; }\n' + isolated.replace("sleep 60", 'sleep "$TRACE"') + '\nfail refused\necho continued'
                result = subprocess.run(["/bin/sh", "-c", program], env={"TRACE": str(trace)}, capture_output=True, timeout=2, check=False)
                self.assertEqual(99, result.returncode, result.stderr)
                self.assertEqual("waiting\n", trace.read_text())
                self.assertNotIn(b"continued", result.stdout)

    def test_missing_existing_state_never_formats(self):
        result = self.run_guard(filesystem="")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("state mount", result.stderr)

    def test_existing_state_uuid_mismatch_blocks_services(self):
        result = self.run_guard(machine_uuid="other")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("machine identity", result.stderr)

    def test_plain_credentials_directory_blocks_services(self):
        result = self.run_guard(credentials="ext4")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("credential tmpfs", result.stderr)

    def test_machine_id_must_be_a_persisted_file_mount(self):
        result = self.run_guard(machine_mount="/")
        self.assertNotEqual(0, result.returncode)

    def test_empty_filesystem_uuid_is_not_an_identity(self):
        result = self.run_guard(state_uuid="", machine_uuid="")
        self.assertNotEqual(0, result.returncode)

    def test_serve_without_early_descriptor_blocks_services(self):
        result = self.run_guard(descriptor=False)
        self.assertNotEqual(0, result.returncode)
        self.assertIn("early descriptor", result.stderr)

    def test_guard_does_not_log_private_identifiers(self):
        result = self.run_guard(state_uuid="private-state", machine_uuid="private-machine")
        self.assertNotIn("private-state", result.stderr)
        self.assertNotIn("private-machine", result.stderr)

    def test_both_daemons_check_on_every_automatic_restart(self):
        unit = self.file("daemon-state.conf").read_text()
        self.assertIn("Requires=bridge-state.service", unit)
        self.assertIn("After=bridge-state.service", unit)
        self.assertIn("ExecStartPre=/usr/lib/bridge/check-state", unit)
        # The same drop-in is installed separately for Docker and containerd.
        from guest.build import ROOTFS_FILES
        self.assertEqual("daemon-state.conf", ROOTFS_FILES["/etc/systemd/system/docker.service.d/bridge-state.conf"])
        self.assertEqual("daemon-state.conf", ROOTFS_FILES["/etc/systemd/system/containerd.service.d/bridge-state.conf"])

    def test_both_storage_roots_are_on_state(self):
        import json

        import tomllib
        docker = json.loads(self.file("daemon.json").read_text())
        containerd = tomllib.loads(self.file("containerd.toml").read_text())
        self.assertEqual("/var/lib/bridge/docker", docker["data-root"])
        self.assertEqual("/var/lib/bridge/containerd", containerd["root"])
        self.assertEqual("/run/containerd", containerd["state"])

    def test_initramfs_hook_owns_early_identity_before_systemd(self):
        script = self.file("initramfs/scripts/local-bottom/bridge-state").read_text()
        self.assertIn('[ "$rootmnt" = /root ] || fail', script)
        self.assertIn('/usr/lib/bridge/bridge-guest --early || fail', script)
        hook = self.file("initramfs/hooks/bridge-state").read_text()
        for path in ("/sbin/blkid", "/sbin/blockdev", "/sbin/mkfs.ext4", "/bin/mount", "/usr/bin/findmnt"):
            self.assertIn(path, hook)

    def test_credential_mount_has_no_persistent_backing(self):
        unit = self.file("bridge-guest.service").read_text()
        self.assertIn("After=bridge-state.service", unit)
        self.assertIn("ExecStart=/usr/lib/bridge/bridge-guest --serve", unit)
        self.assertNotIn("Restart=", unit)
