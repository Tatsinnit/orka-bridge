"""Offline recipe checks; actual disk/daemon evidence comes from the base boot."""
import importlib
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from .fixtures import lock


class BuildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.value = lock(self.root)
        from .test_runtime import runtime_inputs
        runtime_inputs(self.root, self.value)
        self.api = importlib.import_module("guest.build")

    def test_runtime_inputs_are_verified_without_claiming_application_cache(self):
        result = self.api.verify_os_inputs(self.value, source=self.root)
        self.assertFalse(result["componentCacheVerified"])
        self.assertFalse(result["publicationReady"])
        self.assertEqual(self.root / "base.qcow2", result["base"])

    def test_h1_capacities_and_formats(self):
        for arch, fmt in (("amd64", "qcow2"), ("arm64", "raw")):
            contract = self.api.disk_contract(arch)
            self.assertEqual(fmt, contract["osDisk"]["format"])
            self.assertEqual(16 * 1024**3, contract["osDisk"]["virtualBytes"])
            self.assertEqual(32 * 1024**3, contract["stateDisk"]["virtualBytes"])
            self.assertEqual("create-empty", contract["stateDisk"]["creation"])

    def test_rootfs_staging_starts_no_services(self):
        with patch("subprocess.Popen", side_effect=AssertionError("no process during staging")):
            result = self.api.stage_rootfs(self.value, source=self.root, out=self.root / "overlay")
        self.assertFalse(result["diskBuilt"])
        self.assertFalse(result["bootVerified"])
        root = self.root / "overlay"
        self.assertTrue((root / "usr/lib/bridge/bridge-guest").is_file())
        self.assertFalse((root / "usr/lib/bridge/bridge-early-state").exists())
        self.assertTrue((root / "etc/systemd/system/multi-user.target.wants/bridge-guest.service").is_symlink())
        self.assertFalse((root / "run/bridge/boot.json").exists())
        self.assertTrue((root / "var/lib/bridge").is_dir())
        self.assertEqual([], list((root / "var/lib/bridge").iterdir()))

    def test_native_payload_installs_only_runtime_modules_and_native_archives(self):
        from .test_offline import native_inputs
        from .test_runtime import runtime_inputs
        value, _ = native_inputs(self.root)
        runtime_inputs(self.root, value)
        self.api.stage_rootfs(value, source=self.root, out=self.root / 'native')
        root = self.root / 'native'
        self.assertTrue((root / 'opt/bridge/guest/lifecycle.py').is_file())
        self.assertTrue((root / 'opt/bridge/bundle/install.json').is_file())
        self.assertTrue((root / 'usr/local/bin/kmx').is_symlink())
        self.assertTrue((root / 'etc/systemd/system/multi-user.target.wants/bridge-infrastructure.service').is_symlink())
        for name in ('boot_test.py', 'test_seed.py', 'reference_host.py'):
            self.assertFalse((root / 'opt/bridge/guest' / name).exists())
        self.assertFalse((root / 'opt/bridge/bundle/components/telegram.oci.tar').exists())
        self.assertEqual([], list((root / 'var/lib/bridge').iterdir()))

    def test_archive_has_guest_root_ownership_not_builder_ownership(self):
        import tarfile
        self.assertTrue(callable(getattr(self.api, "rootfs_archive", None)), "normalized rootfs archive missing")
        overlay = self.root / "overlay"
        self.api.stage_rootfs(self.value, source=self.root, out=overlay)
        (overlay / "etc").chmod(0o700)
        archive = self.root / "rootfs.tar"
        self.api.rootfs_archive(overlay, archive)
        with tarfile.open(archive) as tar:
            self.assertTrue(all(m.uid == m.gid == 0 for m in tar))
            self.assertEqual(0o755, tar.getmember("etc").mode)
            self.assertEqual(0o755, tar.getmember("usr/lib/bridge/check-state").mode)

    def test_initramfs_dependency_copy_failure_fails_build(self):
        import subprocess
        hook = (Path(self.api.__file__).with_name("rootfs") / "initramfs/hooks/bridge-state").read_text()
        hook = hook.replace(". /usr/share/initramfs-tools/hook-functions", "copy_exec() { return 1; }; manual_add_modules() { return 0; }")
        result = subprocess.run(["/bin/sh", "-c", hook, "hook", "build"], capture_output=True, check=False)
        self.assertNotEqual(0, result.returncode)

    def test_package_mutation_after_preflight_cannot_change_selected_bytes(self):
        import hashlib
        package = dict(self.value["packages"][0], name="docker.io", version="1.0", path="docker.deb", sha256=hashlib.sha256(b"selected").hexdigest())
        (self.root / package["path"]).write_bytes(b"selected")
        self.value["packages"].append(package)
        self.api.verify_os_inputs(self.value, source=self.root)
        source = self.root / package["path"]
        source.write_bytes(b"replacement after verification")
        self.assertTrue(callable(getattr(self.api, "stage_package", None)), "locked package snapshot missing")
        with self.assertRaisesRegex(ValueError, "package changed"):
            self.api.stage_package(source, self.root / "staged.deb", package["sha256"])

    def test_fixed_debian_layout_rejects_other_roots_or_extra_filesystems(self):
        self.assertTrue(callable(getattr(self.api, "root_esp", None)), "explicit base layout validation missing")
        self.assertEqual("/dev/sda2", self.api.root_esp(["/dev/sda1: unknown", "/dev/sda2: vfat", "/dev/sda3: ext4"], "/dev/sda3"))
        for filesystems in (["/dev/sda2: vfat", "/dev/sda3: xfs"], ["/dev/sda2: vfat", "/dev/sda3: ext4", "/dev/sda4: ext4"]):
            with self.assertRaises(ValueError):
                self.api.root_esp(filesystems, "/dev/sda3")

    def test_base_reads_use_an_owned_verified_snapshot(self):
        import hashlib
        from unittest.mock import patch
        tool = self.root / "builder.bin"
        tool.write_bytes(b"builder")
        for name in ("qemu-img", "guestfish", "virt-resize", "virt-customize"):
            self.value["tools"].append({"name": name, "version": "1.0", "path": tool.name, "sha256": hashlib.sha256(tool.read_bytes()).hexdigest()})
        package = next(p for p in self.value["packages"] if p["name"] == "debian-base")
        original = self.root / package["path"]
        def inspect(snapshot):
            self.assertNotEqual(original, snapshot)
            self.assertEqual(package["sha256"], self.api.file_hash(snapshot))
            original.write_bytes(b"external replacement after snapshot")
            self.assertEqual(package["sha256"], self.api.file_hash(snapshot))
            raise RuntimeError("snapshot observed")
        with patch.object(self.api.platform, "machine", return_value="x86_64"), patch.object(self.api.shutil, "which", return_value=str(tool)), patch.object(self.api, "run", return_value="1.0"), patch.object(self.api, "image_info", side_effect=inspect), self.assertRaisesRegex(RuntimeError, "snapshot observed"):
            self.api.build(self.value, source=self.root, out=self.root / "build")

    def test_bad_input_leaves_no_output(self):
        (self.root / "base.qcow2").write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "checksum"):
            self.api.stage_rootfs(self.value, source=self.root, out=self.root / "overlay")
        self.assertFalse((self.root / "overlay").exists())

    def test_existing_output_is_not_overwritten(self):
        with self.assertRaisesRegex(ValueError, "already exist"):
            self.api.stage_rootfs(self.value, source=self.root, out=self.root)

    def test_factory_overlay_has_no_test_bootstrap(self):
        self.api.stage_rootfs(self.value, source=self.root, out=self.root / "overlay")
        names = [str(p.relative_to(self.root / "overlay")) for p in (self.root / "overlay").rglob("*")]
        self.assertFalse(any("reference-seed" in n or "reference-check" in n for n in names))

    def test_assembly_command_is_offline_and_never_uses_apt(self):
        self.assertTrue(callable(getattr(self.api, "customize_commands", None)))
        commands = self.api.customize_commands([self.root / "docker.deb"], "amd64")
        joined = "\n".join(commands)
        self.assertIn("dpkg --force-confold -i", joined)
        self.assertNotIn("apt-get", joined)
        self.assertIn("policy-rc.d", joined)
        self.assertIn("update-initramfs", joined)
        self.assertIn("--removable --no-nvram", joined)
        self.assertNotIn("kind create", joined)
