"""Read-only reference input. Never installed in the factory OS."""
import json
import subprocess
import tempfile
import uuid
from pathlib import Path
from typing import TYPE_CHECKING

if __package__ or TYPE_CHECKING:
    from .lock import require
else:
    from lock import require


def descriptor(installation_id, state_creation, *, case_name='base'):
    require(str(uuid.UUID(installation_id)) == installation_id, "invalid installation UUID")
    require(state_creation in ("new", "existing"), "explicit state creation mode required")
    require(case_name in ('base', 'infrastructure', 'offline-tasks', 'restart', 'interrupted'), 'unknown reference case')
    return {"schemaVersion": "bridge-guest-test/v1", "caseId": case_name, "installationId": installation_id,
            "stateCreation": state_creation, "stateSerial": "bridge-state"}


def write_seed(out: Path, installation_id: str, state_creation: str, *, case_name='base', fixture=None):
    value = descriptor(installation_id, state_creation, case_name=case_name)
    require(not out.exists() and not out.is_symlink(), "seed must not exist")
    with tempfile.TemporaryDirectory(prefix="bridge-seed-") as temp:
        if fixture is not None:
            import hashlib
            import shutil
            require(fixture.is_file() and fixture.stat().st_size < 32 * 1024**2, 'invalid fixed test fixture')
            shutil.copyfile(fixture, Path(temp) / 'fixture.tar')
            value['fixtureSha256'] = hashlib.sha256((Path(temp) / 'fixture.tar').read_bytes()).hexdigest()
        (Path(temp) / "bridge.json").write_text(json.dumps(value) + "\n")
        subprocess.run(["genisoimage", "-quiet", "-V", "BRIDGE_TEST", "-r", "-J", "-o", str(out), temp], check=True)
    return value
