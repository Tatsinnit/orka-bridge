"""Fixed reference-host exchange for the delivered endpoint, not an app backend."""
import json
import socket
import time
import uuid
from typing import TYPE_CHECKING

if __package__ or TYPE_CHECKING:
    from .lock import require
else:
    from lock import require


def runtime_exchange(channel, boot, before_shutdown=None):
    frames = []
    with channel.makefile("rb") as reader:
        def receive():
            raw = reader.readline(128 * 1024 + 1)
            require(raw.endswith(b"\n") and len(raw) <= 128 * 1024, "invalid runtime frame length")
            value = json.loads(raw)
            require(isinstance(value, dict) and set(value) == {"type", "body"}, "invalid runtime frame")
            require(value["body"].get("version") == 1, "runtime version mismatch")
            frames.append(value)
            return value

        def request(id_, body):
            channel.sendall(json.dumps({"type": "request", "body": {"version": 1, "bootNonce": boot["bootNonce"], "requestId": id_, "body": body}}).encode() + b"\n")
            result = receive()
            require(result["type"] == "result" and result["body"].get("bootNonce") == boot["bootNonce"] and result["body"].get("requestId") == id_, "runtime result identity mismatch")
            require(result["body"].get("body", {}).get("op") in ("ready", "error"), "unexpected runtime result")
            return result["body"]["body"]["op"] == "ready"

        previous = None
        for stage, id_ in (("early", 1), ("serve", 2)):
            hello = receive()
            expected = None if stage == "early" else boot["bootNonce"]
            require(hello["type"] == "hello" and hello["body"].get("stage") == stage and hello["body"].get("acceptedBootNonce") == expected, "runtime hello stage mismatch")
            nonce = uuid.UUID(hello["body"]["guestNonce"])
            require(nonce.int != 0 and nonce != previous, "runtime hello nonce mismatch")
            previous = nonce
            descriptor = {**boot, "stateCreation": boot["stateCreation"] if stage == "early" else "existing"}
            if not request(id_, {"op": "boot", "body": descriptor}):
                return frames
        require(request(3, {"op": "readiness", "body": {}}), "runtime readiness refused")
        if before_shutdown is not None:
            before_shutdown()
            require(request(4, {"op": "shutdown", "body": {}}), "runtime shutdown refused")
    return frames


def wait_for_verifier(root, process, stop, deadline):
    while True:
        serial = root / "serial.log"
        if serial.is_file():
            with serial.open("rb") as source:
                source.seek(0, 2)
                source.seek(max(0, source.tell() - 128 * 1024))
                if any(line.startswith(b"BRIDGE_BASE ") and line.endswith(b"\n") for line in source.read(128 * 1024).splitlines(keepends=True)):
                    return
        require(process.poll() is None and not stop.is_set() and time.monotonic() < deadline, "guest verifier incomplete")
        stop.wait(0.05)


def control_worker(root, boot, process, stop, frames, errors):
    deadline = time.monotonic() + 180
    try:
        with socket.socket(socket.AF_UNIX) as channel:
            while True:
                try:
                    channel.connect(str(root / "control.sock"))
                    break
                except (FileNotFoundError, ConnectionRefusedError):
                    require(process.poll() is None and time.monotonic() < deadline, "runtime control unavailable")
                    time.sleep(0.05)
            channel.settimeout(max(0.1, deadline - time.monotonic()))
            frames.extend(runtime_exchange(channel, boot, before_shutdown=lambda: wait_for_verifier(root, process, stop, deadline)))
            # Keep the serial lease open until this owned VM stops.
            stop.wait(max(0, deadline - time.monotonic()))
    except (OSError, ValueError, KeyError, TypeError) as error:
        errors.append(str(error))
