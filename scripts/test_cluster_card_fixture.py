"""Exercise the CI-only native Send fixture over the real port-forward wire."""
import http.client
import importlib.util
import json
from pathlib import Path
import socket
import struct
import threading
import unittest
from http.server import ThreadingHTTPServer

spec = importlib.util.spec_from_file_location("fixture", Path(__file__).with_name("cluster-card-fixture.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class SendFixtureTest(unittest.TestCase):
    def setUp(self):
        fixture.Discovery.objects = {}
        fixture.Discovery.agents = {}
        fixture.Discovery.offline = set()
        fixture.Discovery.lose_receipt = set()
        fixture.Discovery.cancel_modes = {}
        fixture.Discovery.cancel_ready = threading.Event()

    def cancel_fixture(self, mode):
        server = ThreadingHTTPServer(("127.0.0.1", 0), fixture.Discovery)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        connection = http.client.HTTPConnection(*server.server_address, timeout=2)
        self.addCleanup(connection.close)

        def control(mode):
            connection.request("POST", "/fixture-control", json.dumps({
                "action": "cancel-mode", "place": "fixture-cluster", "mode": mode,
            }))
            response = connection.getresponse()
            self.assertEqual(response.status, 200)
            response.read()

        connection.request("POST", "/fixture-control", json.dumps({"action": "reset"}))
        response = connection.getresponse()
        self.assertEqual(response.status, 200)
        response.read()
        control(mode)
        handler = object.__new__(fixture.Discovery)
        handler.orka("POST", "/api/v1/agents", {"name": "probe", "spec": {}})
        for name in ["finished", "live"]:
            self.assertEqual(handler.orka("POST", "/api/v1/tasks", {
                "name": name, "spec": {"agentRef": {"name": "probe"}},
            })[0], 201)
        return handler, control

    def test_cancel_walk_preserves_completed_answer_and_requires_exact_task_uid(self):
        handler, _ = self.cancel_fixture("requested")
        self.assertEqual(handler.orka("GET", "/api/v1/tasks/finished", None)[1]["status"]["phase"], "Succeeded")
        self.assertEqual(handler.orka("GET", "/api/v1/tasks/live", None)[1]["status"]["phase"], "Running")
        answer = handler.orka("GET", "/api/v1/tasks/finished/result", None)[1]
        self.assertEqual(handler.orka("POST", "/api/v1/tasks/finished/cancel", {"uid": "uid-finished"})[0], 409)
        self.assertEqual(handler.orka("GET", "/api/v1/tasks/finished", None)[1]["status"]["phase"], "Succeeded")
        for body, expected in [({}, 400), ({"uid": "replacement"}, 409), ({"uid": "uid-live"}, 200)]:
            status, reply = handler.orka("POST", "/api/v1/tasks/live/cancel?namespace=agents", body)
            self.assertEqual(status, expected)
        self.assertEqual(reply, {"name": "live", "namespace": "agents", "requested": True, "phase": "Running"})
        self.assertEqual(handler.orka("GET", "/api/v1/tasks/live", None)[1]["status"]["phase"], "Running")
        self.assertEqual(handler.orka("GET", "/api/v1/tasks/finished/result", None)[1], answer)
        self.assertEqual(len(fixture.Discovery.objects), 3)  # agent and both Tasks retained

    def test_cancel_modes_distinguish_terminal_denied_unsupported_unknown_and_completion_race(self):
        for mode, expected_status, expected_phase in [
            ("denied", 403, "Running"), ("unsupported", 405, "Running"),
            ("unconfirmed", None, "Running"), ("confirmed", 200, "Cancelled"),
            ("completed", 409, "Succeeded"),
        ]:
            with self.subTest(mode=mode):
                handler, _ = self.cancel_fixture(mode)
                status, _ = handler.orka("POST", "/api/v1/tasks/live/cancel", {"uid": "uid-live"})
                self.assertEqual(status, expected_status)
                self.assertEqual(handler.orka("GET", "/api/v1/tasks/live", None)[1]["status"]["phase"], expected_phase)
                self.assertEqual(handler.orka("GET", "/api/v1/tasks/finished", None)[1]["status"]["phase"], "Succeeded")
                self.assertEqual(len(fixture.Discovery.objects), 3)

    def test_cancelling_response_waits_for_explicit_fixture_release(self):
        handler, control = self.cancel_fixture("cancelling")
        reply = []
        worker = threading.Thread(target=lambda: reply.append(handler.orka(
            "POST", "/api/v1/tasks/live/cancel", {"uid": "uid-live"})))
        worker.start()
        self.addCleanup(worker.join, 2)
        worker.join(0.05)
        self.assertTrue(worker.is_alive(), "Cancelling must remain visible until the native capture releases it")
        control("requested")
        worker.join(1)
        self.assertFalse(worker.is_alive())
        self.assertEqual(reply[0][0], 200)
        self.assertEqual(reply[0][1]["phase"], "Running")

    def test_inventory_retains_unique_versions_and_external_runtime_metadata(self):
        handler = object.__new__(fixture.Discovery)
        for name in ["bridge-run-first", "bridge-run-second"]:
            status, _ = handler.orka("POST", "/api/v1/agents", {
                "name": name, "spec": {"model": {"name": "exact"}, "tools": [{"name": "web_fetch"}]},
            })
            self.assertEqual(status, 201)
        status, page = handler.orka("GET", "/api/v1/agents?namespace=agents&limit=100", None)
        self.assertEqual(status, 200)
        by_name = {agent["metadata"]["name"]: agent for agent in page["items"]}
        self.assertEqual(set(by_name), {"bridge-run-first", "bridge-run-second", "runtime-reader"})
        self.assertEqual(len({agent["metadata"]["uid"] for agent in page["items"]}), 3)
        self.assertTrue(all(agent["metadata"]["namespace"] == "agents" for agent in page["items"]))
        self.assertEqual(by_name["runtime-reader"]["spec"], {
            "type": "runtime", "model": {"name": "external-model"}, "tools": [{"name": "web_fetch"}],
        })
        self.assertEqual(by_name["bridge-run-first"]["spec"]["model"]["name"], "exact")
        self.assertEqual(handler.orka("GET", "/api/v1/agents?namespace=elsewhere", None)[1]["items"], [])

    def test_places_have_distinct_installations_and_isolated_retained_agents(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), fixture.Discovery)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        connection = http.client.HTTPConnection(*server.server_address, timeout=2)
        self.addCleanup(connection.close)
        identities = []
        handler = object.__new__(fixture.Discovery)
        for token, name in [("ci-fixture-not-a-credential", "first-only"), ("secondary-fixture", "second-only")]:
            authorization = "Bearer " + token
            uids = []
            for path in ["/api/v1/services", "/apis/apps/v1/deployments"]:
                connection.request("GET", path, headers={"Authorization": authorization})
                response = connection.getresponse()
                self.assertEqual(response.status, 200)
                uids.append(json.loads(response.read())["items"][0]["metadata"]["uid"])
            identities.append(uids)
            handler.orka("POST", "/api/v1/agents", {"name": name, "spec": {}}, authorization)
            _, task = handler.orka("POST", "/api/v1/tasks", {"name": name, "spec": {}}, authorization)
            self.assertEqual(task["status"]["phase"], "Succeeded")
        self.assertNotEqual(identities[0][0], identities[1][0])
        self.assertNotEqual(identities[0][1], identities[1][1])
        for token, expected in [("ci-fixture-not-a-credential", "first-only"), ("secondary-fixture", "second-only")]:
            status, page = handler.orka("GET", "/api/v1/agents?namespace=agents&limit=100", None, "Bearer " + token)
            self.assertEqual(status, 200)
            self.assertEqual({agent["metadata"]["name"] for agent in page["items"]}, {expected, "runtime-reader"})

    def test_fixture_control_takes_only_one_place_offline_and_reset_restores_it(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), fixture.Discovery)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        connection = http.client.HTTPConnection(*server.server_address, timeout=2)
        self.addCleanup(connection.close)
        handler = object.__new__(fixture.Discovery)
        auth = "Bearer secondary-fixture"
        handler.orka("POST", "/api/v1/agents", {"name": "retained", "spec": {}}, auth)
        for control in [{"action": "offline", "place": "fixture-secondary", "offline": True},
                        {"action": "offline", "place": "fixture-secondary", "offline": False},
                        {"action": "reset"}]:
            connection.request("POST", "/fixture-control", json.dumps(control), {"Content-Type": "application/json"})
            response = connection.getresponse()
            self.assertEqual(response.status, 200)
            response.read()
            offline = control.get("offline", False)
            connection.request("GET", "/api/v1/services", headers={"Authorization": auth})
            response = connection.getresponse()
            self.assertEqual(response.status, 503 if offline else 200)
            response.read()
            self.assertEqual(handler.orka("GET", "/api/v1/agents", None)[0], 200)
            status, page = handler.orka("GET", "/api/v1/agents", None, auth)
            self.assertEqual(status, 503 if offline else 200)
            if not offline:
                self.assertEqual("retained" in [agent["metadata"]["name"] for agent in page["items"]],
                                 control["action"] != "reset")

    def test_conditional_and_denied_access_are_distinct(self):
        handler = object.__new__(fixture.Discovery)
        status, identity = handler.orka("GET", "/api/v1/auth/whoami", None, "Bearer conditional-fixture")
        self.assertEqual(status, 200)
        self.assertEqual(identity["authType"], "tokenReview")
        self.assertEqual(identity["namespace"], "")
        status, _ = handler.orka("GET", "/api/v1/auth/whoami", None, "Bearer denied-fixture")
        self.assertEqual(status, 403)

    def test_missing_and_unreadable_catalogues_are_distinct(self):
        handler = object.__new__(fixture.Discovery)
        status, page = handler.orka("GET", "/api/v1/tools", None, "Bearer missing-tools-fixture")
        self.assertEqual(status, 200)
        self.assertEqual(page["items"], [])
        status, _ = handler.orka("GET", "/api/v1/tools", None, "Bearer unreadable-tools-fixture")
        self.assertEqual(status, 403)

    def test_discovery_serves_required_send_schema_fields(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), fixture.Discovery)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        connection = http.client.HTTPConnection(*server.server_address, timeout=2)
        self.addCleanup(connection.close)
        for plural, path, expected in [
            ("agents", ["spec", "model", "name"], "string"),
            ("tasks", ["spec", "retryPolicy", "maxRetries"], "integer"),
            ("providers", ["status", "ready"], "boolean"),
        ]:
            connection.request("GET", f"/apis/apiextensions.k8s.io/v1/customresourcedefinitions/{plural}.core.orka.ai")
            response = connection.getresponse()
            self.assertEqual(response.status, 200)
            schema = json.loads(response.read())["spec"]["versions"][0]["schema"]["openAPIV3Schema"]
            for field in path:
                schema = schema["properties"][field]
            self.assertEqual(schema["type"], expected)

    def test_native_forward_carries_create_receipts_and_check_evidence(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), fixture.Discovery)
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        with socket.create_connection(server.server_address, timeout=2) as sock:
            sock.sendall(b"GET /api/v1/namespaces/control/pods/ready/portforward?ports=8080 HTTP/1.1\r\nHost: fixture\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: v4.channel.k8s.io\r\n\r\n")
            wire = sock.makefile("rb")
            self.assertIn(b"101 Switching Protocols", wire.readline())
            while wire.readline() != b"\r\n":
                pass

            def frame():
                opcode, length = wire.read(2)
                self.assertEqual(opcode, 130)
                if length == 126:
                    length = struct.unpack("!H", wire.read(2))[0]
                elif length == 127:
                    length = struct.unpack("!Q", wire.read(8))[0]
                return wire.read(length)

            self.assertEqual(frame(), b"\x00\x90\x1f")
            self.assertEqual(frame(), b"\x01\x90\x1f")

            def request(path, body=None, authorization="Bearer ci-fixture-not-a-credential", expected_status=200):
                data = json.dumps(body).encode() if body is not None else b""
                method = "POST" if body is not None else "GET"
                http = f"{method} /api/v1/{path}?namespace=agents HTTP/1.1\r\nHost: fixture\r\nAuthorization: {authorization}\r\nContent-Length: {len(data)}\r\n\r\n".encode() + data
                payload = b"\x00" + http
                mask = b"1234"
                size = bytes([len(payload)]) if len(payload) < 126 else b"\x7e" + struct.pack("!H", len(payload))
                sock.sendall(b"\x82" + bytes([size[0] | 128]) + size[1:] + mask + bytes(v ^ mask[i % 4] for i, v in enumerate(payload)))
                reply = frame()
                self.assertIn(f"HTTP/1.1 {expected_status} ".encode(), reply.split(b"\r\n", 1)[0])
                return json.loads(reply.split(b"\r\n\r\n", 1)[1])

            self.assertEqual(request("auth/whoami")["namespace"], "agents")
            self.assertEqual(request("auth/whoami", authorization="Bearer conditional-fixture")["namespace"], "")
            self.assertEqual(request("auth/whoami", authorization="Bearer denied-fixture", expected_status=403), {})
            created = request("agents", {"name": "probe", "namespace": "agents", "spec": {"model": {"name": "exact"}}}, expected_status=201)
            self.assertEqual(created["metadata"]["name"], "probe")
            self.assertEqual(created["spec"]["model"]["name"], "exact")
            task = request("tasks", {"name": "check", "namespace": "agents", "spec": {"agentRef": {"name": "probe"}, "timeout": "120s", "retryPolicy": {"maxRetries": 0}}}, expected_status=201)
            self.assertEqual(task["status"]["phase"], "Succeeded")
            self.assertEqual(request("tasks/check")["metadata"]["uid"], task["metadata"]["uid"])
            self.assertIn("cf699c", request("tasks/check/result")["result"])
            self.assertEqual(request("tasks/check/events")["events"][0]["toolName"], "web_fetch")
            request("agents", {"name": "probe2", "namespace": "agents", "spec": {}}, expected_status=201)
            second = request("tasks", {"name": "check2", "namespace": "agents", "spec": {"agentRef": {"name": "probe2"}}}, expected_status=201)
            self.assertEqual(second["status"]["phase"], "Succeeded")
            request("agents", {"name": "probe3", "namespace": "agents", "spec": {}}, expected_status=201)
            third = request("tasks", {"name": "check3", "namespace": "agents", "spec": {"agentRef": {"name": "probe3"}}}, expected_status=201)
            self.assertEqual(third["status"]["phase"], "Running")
            control = http.client.HTTPConnection(*server.server_address, timeout=2)
            self.addCleanup(control.close)
            control.request("POST", "/fixture-control", json.dumps({"action": "lose-agent-receipt", "place": "fixture-cluster"}))
            response = control.getresponse()
            self.assertEqual(response.status, 200)
            response.read()
            lost = request("agents", {"name": "unacknowledged", "spec": {}}, expected_status=503)
            self.assertNotIn("metadata", lost)
            inventory = request("agents")
            self.assertEqual({agent["metadata"]["name"] for agent in inventory["items"]},
                             {"probe", "probe2", "probe3", "unacknowledged", "runtime-reader"})
            external = next(agent for agent in inventory["items"] if agent["metadata"]["name"] == "runtime-reader")
            self.assertEqual(external["spec"]["type"], "runtime")
            self.assertEqual(external["spec"]["tools"], [{"name": "web_fetch"}])
            # The fault is consumed once. Another explicit create receives its UID.
            self.assertIn("uid", request("agents", {"name": "after-loss", "spec": {}}, expected_status=201)["metadata"])
            # The cancellation fixture first leaves a completed answer, then a
            # live Task. A lost response closes this real forward, not a fake 200.
            control.request("POST", "/fixture-control", json.dumps({
                "action": "cancel-mode", "place": "fixture-cluster", "mode": "unconfirmed",
            }))
            response = control.getresponse()
            self.assertEqual(response.status, 200)
            response.read()
            request("agents", {"name": "cancel-probe", "spec": {}}, expected_status=201)
            for name in ["cancel-finished", "cancel-live"]:
                request("tasks", {"name": name, "spec": {"agentRef": {"name": "cancel-probe"}}}, expected_status=201)
            self.assertEqual(request("tasks/cancel-live")["status"]["phase"], "Running")
            data = json.dumps({"uid": "uid-cancel-live"}).encode()
            payload = b"\x00" + f"POST /api/v1/tasks/cancel-live/cancel?namespace=agents HTTP/1.1\r\nHost: fixture\r\nContent-Length: {len(data)}\r\n\r\n".encode() + data
            mask = b"1234"
            sock.sendall(b"\x82\xfe" + struct.pack("!H", len(payload)) + mask + bytes(v ^ mask[i % 4] for i, v in enumerate(payload)))
            self.assertEqual(wire.read(2), b"", "Lost cancellation response must end the forward without an acknowledgement")
            self.assertEqual(fixture.Discovery.objects["fixture-cluster", "/api/v1/tasks/cancel-live"]["status"]["phase"], "Running")
            wire.close()


if __name__ == "__main__":
    unittest.main()
