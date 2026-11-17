"""JSON Schema parity tests; the validator is a development-only dependency."""
import copy
import hashlib
import importlib
import importlib.util
import json
import re
import tempfile
import unittest
from pathlib import Path

from images import package
from images.test_package import BoundedStream, boot_inputs, metadata


class AcceptanceTests(unittest.TestCase):
    def test_publisher_acceptance_matches_schema_and_native_consumer(self):
        root = Path(package.__file__).resolve().parents[1]
        policy = json.loads((root / 'images/publisher-policy.json').read_bytes())
        digest = hashlib.sha256((root / 'images/manifest.schema.json').read_bytes()).hexdigest()
        consumer = (root / 'app/src-tauri/src/runtime/manifest.rs').read_text()
        pin = re.search(r'const SCHEMA_SHA256: &str = "([0-9a-f]{64})";', consumer)
        assert pin is not None, 'native consumer schema pin missing'
        self.assertEqual(policy['acceptedSchemaSha256'], digest)
        self.assertEqual(pin.group(1), digest)


@unittest.skipUnless(importlib.util.find_spec("jsonschema"), "Install python3-jsonschema to run schema tests")
class SchemaTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        library = importlib.import_module("jsonschema")
        schema = json.loads(Path(package.__file__).with_name("manifest.schema.json").read_text())
        library.Draft202012Validator.check_schema(schema)
        self.validator = library.Draft202012Validator(schema, format_checker=library.FormatChecker())

    def manifest(self, platform):
        root = self.root / platform
        root.mkdir()
        meta = metadata(root, platform)
        payload = b"synthetic schema fixture, not a bootable image"
        return package.create(BoundedStream(payload), meta, root / "package",
                              **boot_inputs(root / "boot", meta, payload))

    def test_complete_role_sets_validate_for_both_targets(self):
        for platform in ("windows", "macos"):
            with self.subTest(platform=platform):
                manifest = self.manifest(platform)
                self.validator.validate(manifest)
                package.validate_manifest(manifest)

    def test_duplicate_driver_name_cannot_replace_required_driver(self):
        for platform in ("windows", "macos"):
            with self.subTest(platform=platform):
                manifest = self.manifest(platform)
                self.validator.validate(manifest)
                bad = copy.deepcopy(manifest)
                rows = bad["directBoot"]["requiredDrivers"]
                rows[-1] = dict(rows[0], builtIn=not rows[0]["builtIn"])
                self.assertEqual(len(rows), len({json.dumps(row, sort_keys=True) for row in rows}))
                self.assertEqual({"contains"}, {error.validator for error in self.validator.iter_errors(bad)})
                with self.assertRaisesRegex(ValueError, "required drivers"):
                    package.validate_manifest(bad)

    def test_duplicate_tool_name_cannot_replace_required_extractor(self):
        for platform in ("windows", "macos"):
            with self.subTest(platform=platform):
                manifest = self.manifest(platform)
                self.validator.validate(manifest)
                bad = copy.deepcopy(manifest)
                rows = bad["directBoot"]["provenance"]["extractionTools"]
                rows[-1] = dict(rows[0], version="different valid version")
                self.assertEqual(len(rows), len({json.dumps(row, sort_keys=True) for row in rows}))
                self.assertEqual({"contains"}, {error.validator for error in self.validator.iter_errors(bad)})
                with self.assertRaisesRegex(ValueError, "extraction tool"):
                    package.validate_manifest(bad)
