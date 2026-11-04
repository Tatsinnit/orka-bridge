import importlib
import unittest
import uuid
from pathlib import Path


class ReferenceTests(unittest.TestCase):
    def module(self, name):
        try:
            return importlib.import_module("guest." + name)
        except ModuleNotFoundError:
            self.fail(name + " not implemented")

    def test_seed_explicitly_authorizes_new_or_existing(self):
        api = self.module("test_seed")
        id_ = str(uuid.uuid4())
        self.assertEqual("new", api.descriptor(id_, "new")["stateCreation"])
        self.assertEqual("existing", api.descriptor(id_, "existing")["stateCreation"])
        for mode in ("", "auto", "missing"):
            with self.assertRaises(ValueError):
                api.descriptor(id_, mode)
        with self.assertRaises(ValueError):
            api.descriptor("not-a-uuid", "new")

    def test_qemu_devices_are_identified_and_egress_restricted(self):
        api = self.module("boot_test")
        args = api.qemu_argv(Path("/owned"), str(uuid.uuid4()), True)
        text = " ".join(map(str, args))
        for required in ("-accel kvm", "serial=bridge-os", "serial=bridge-state", "serial=bridge-test", "ai.orka.bridge.control", "restrict=on", "-no-reboot"):
            self.assertIn(required, text)
        for forbidden in ("hostfwd", "-nographic", "-snapshot", "cache=unsafe"):
            self.assertNotIn(forbidden, text)
        self.assertIn("readonly=on", text)

    def test_output_path_cannot_inject_qemu_options(self):
        api = self.module("boot_test")
        text = " ".join(map(str, api.qemu_argv(Path("/owned,a=b"), str(uuid.uuid4()), True)))
        self.assertIn("file=/owned,,a=b/os.qcow2", text)
        self.assertIn("path=/owned,,a=b/control.sock", text)

    def test_missing_state_case_has_no_state_device(self):
        api = self.module("boot_test")
        text = " ".join(map(str, api.qemu_argv(Path("/owned"), str(uuid.uuid4()), False)))
        self.assertNotIn("serial=bridge-state", text)

    def test_nonhex_machine_id_is_not_a_valid_result(self):
        api = self.module("boot_test")
        import json
        result = {"schemaVersion": "bridge-guest-base/v1", "installationId": "id", "stateUuid": "id", "machineId": "z" * 32,
                  "sentinel": True, "stateBeforeDaemons": True, "machineIdBeforeSystemd": True, "controlPort": True,
                  "credentialsType": "tmpfs", "dockerRoot": "/var/lib/bridge/docker", "containerdRoot": "/var/lib/bridge/containerd"}
        with self.assertRaises(ValueError):
            api.parse_result("BRIDGE_BASE " + json.dumps(result), "id")

    def test_failed_or_wrong_install_result_is_not_success(self):
        api = self.module("boot_test")
        with self.assertRaises(ValueError):
            api.parse_result("booted Linux", "id")
        with self.assertRaises(ValueError):
            api.parse_result('BRIDGE_BASE {"schemaVersion":"bridge-guest-base/v1","installationId":"other"}', "id")
