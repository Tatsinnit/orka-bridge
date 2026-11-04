import hashlib
import json
import unittest
from pathlib import Path


def runtime_inputs(root, value):
    arch = value["architecture"]
    binary = bytearray(64)
    binary[:6] = b"\x7fELF\x02\x01"
    binary[18:20] = (62 if arch == "amd64" else 183).to_bytes(2, "little")
    (root / "bridge-guest").write_bytes(binary)
    record = {"schemaVersion": "bridge-guest-binary/v1", "sourceCommit": "a" * 40,
              "target": "x86_64-unknown-linux-musl" if arch == "amd64" else "aarch64-unknown-linux-musl",
              "binaryBytes": len(binary), "binarySha256": hashlib.sha256(binary).hexdigest(),
              "contractVersion": "H1/v1", "protocolMajor": 1}
    (root / "provenance.json").write_text(json.dumps(record))
    for name, path in (("bridge-guest", "bridge-guest"), ("bridge-guest-provenance", "provenance.json")):
        value["tools"].append({"name": name, "version": record["sourceCommit"], "path": path,
                               "sha256": hashlib.sha256((root / path).read_bytes()).hexdigest()})


class RuntimeInputsTests(unittest.TestCase):
    def test_runtime_requires_hash_bound_provenance(self):
        import tempfile

        from guest.build import verify_os_inputs

        from .fixtures import lock
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            value = lock(root)
            with self.assertRaisesRegex(ValueError, "runtime"):
                verify_os_inputs(value, source=root)
            runtime_inputs(root, value)
            result = verify_os_inputs(value, source=root)
            self.assertEqual("a" * 40, result["runtime"]["sourceCommit"])
            record = json.loads((root / "provenance.json").read_text())
            record["binarySha256"] = "0" * 64
            (root / "provenance.json").write_text(json.dumps(record))
            value["tools"][-1]["sha256"] = hashlib.sha256((root / "provenance.json").read_bytes()).hexdigest()
            with self.assertRaisesRegex(ValueError, "runtime"):
                verify_os_inputs(value, source=root)
