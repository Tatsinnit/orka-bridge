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


def descriptor(installation_id, state_creation):
    require(str(uuid.UUID(installation_id)) == installation_id, "invalid installation UUID")
    require(state_creation in ("new", "existing"), "explicit state creation mode required")
    return {"schemaVersion": "bridge-guest-test/v1", "caseId": "base", "installationId": installation_id,
            "stateCreation": state_creation, "stateSerial": "bridge-state"}


def write_seed(out: Path, installation_id: str, state_creation: str):
    value = descriptor(installation_id, state_creation)
    require(not out.exists() and not out.is_symlink(), "seed must not exist")
    with tempfile.TemporaryDirectory(prefix="bridge-seed-") as temp:
        (Path(temp) / "bridge.json").write_text(json.dumps(value) + "\n")
        subprocess.run(["genisoimage", "-quiet", "-V", "BRIDGE_TEST", "-r", "-J", "-o", str(out), temp], check=True)
    return value
