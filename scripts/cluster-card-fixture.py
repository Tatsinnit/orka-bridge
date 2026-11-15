"""Loopback-only native Send walk fixture; no real cluster, model or credentials."""
import base64
import hashlib
import json
import struct
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import sys
from urllib.parse import parse_qs, urlsplit


def send_schema(plural):
    def obj(**properties):
        return {"type": "object", "properties": properties}

    string = {"type": "string"}
    schemas = {
        "agents": obj(spec=obj(model=obj(name=string), providerRef=obj(name=string),
                               systemPrompt=obj(inline=string), tools={"type": "array", "items": obj(name=string)})),
        "tasks": obj(spec=obj(type={"type": "string", "enum": ["container", "ai", "agent"]},
                              agentRef=obj(name=string), prompt=string, timeout=string,
                              retryPolicy=obj(maxRetries={"type": "integer"}))),
        "providers": obj(status=obj(ready={"type": "boolean"})),
        "tools": obj(metadata={"type": "object"}),
    }
    return schemas[plural]


class Discovery(BaseHTTPRequestHandler):
    objects = {}
    agents = {}
    offline = set()
    lose_receipt = set()
    cancel_modes = {}
    cancel_ready = threading.Event()
    lock = threading.Lock()

    @staticmethod
    def place(authorization):
        return "fixture-secondary" if authorization == "Bearer secondary-fixture" else "fixture-cluster"

    def log_message(self, *_):
        pass

    def reply(self, status, value):
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        # Fixture controls live only on this loopback server, never in the app.
        if self.path != "/fixture-control" or self.client_address[0] != "127.0.0.1":
            self.send_error(404)
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 4096:
                raise ValueError()
            control = json.loads(self.rfile.read(length))
            action = control["action"]
            with self.lock:
                if action == "reset":
                    self.objects.clear()
                    self.agents.clear()
                    self.offline.clear()
                    self.lose_receipt.clear()
                    self.cancel_modes.clear()
                    self.cancel_ready.set()
                elif action == "cancel-mode" and control["place"] in ["fixture-cluster", "fixture-secondary"] and control["mode"] in ["cancelling", "requested", "confirmed", "denied", "unsupported", "unconfirmed", "completed"]:
                    self.cancel_modes[control["place"]] = control["mode"]
                    if control["mode"] == "cancelling":
                        self.cancel_ready.clear()
                    else:
                        self.cancel_ready.set()
                elif action == "lose-agent-receipt" and control["place"] in ["fixture-cluster", "fixture-secondary"]:
                    self.lose_receipt.add(control["place"])
                elif action == "offline" and control["place"] in ["fixture-cluster", "fixture-secondary"] and isinstance(control["offline"], bool):
                    if control["offline"]:
                        self.offline.add(control["place"])
                    else:
                        self.offline.discard(control["place"])
                else:
                    raise ValueError()
        except (ValueError, KeyError, TypeError):
            self.reply(400, {"error": "Invalid fixture control"})
            return
        self.reply(200, {"ok": True})

    def do_GET(self):
        path = self.path.split("?")[0]
        place = self.place(self.headers.get("Authorization", ""))
        with self.lock:
            offline = place in self.offline
        if offline:
            self.reply(503, {"error": "Synthetic place offline"})
            return
        suffix = "-secondary" if place == "fixture-secondary" else ""
        if path.endswith("/portforward"):
            self.forward()
            return
        if path.endswith("/pods"):
            value = {"apiVersion": "v1", "kind": "PodList", "items": [{
                "metadata": {"name": "ready", "namespace": "control", "uid": "pod-fixture" + suffix, "labels": {"app": "orka"}},
                "spec": {"containers": [{"name": "controller", "ports": [{"containerPort": 8080}]}]},
                "status": {"phase": "Running", "conditions": [{"type": "Ready", "status": "True"}]},
            }]}
        elif path.endswith("/services"):
            value = {"apiVersion": "v1", "kind": "ServiceList", "items": [{
                "metadata": {"name": "orka", "namespace": "control", "uid": "service-fixture" + suffix},
                "spec": {"selector": {"app": "orka"}, "ports": [{"name": "api", "port": 80, "targetPort": 8080}]},
            }]}
        elif path.endswith("/deployments"):
            value = {"apiVersion": "apps/v1", "kind": "DeploymentList", "items": [{
                "metadata": {"name": "controller", "namespace": "control", "uid": "deployment-fixture" + suffix},
                "spec": {"selector": {"matchLabels": {"app": "orka"}}, "template": {
                    "metadata": {"labels": {"app": "orka"}}, "spec": {"containers": [{
                        "name": "controller", "image": "ghcr.io/orka-agents/orka:fixture",
                        "args": ["--api-port=8080", "--watch-namespace=agents"],
                        "ports": [{"containerPort": 8080}],
                    }]},
                }},
            }]}
        elif "/customresourcedefinitions/" in path:
            plural = path.rsplit("/", 1)[1].split(".")[0]
            value = {"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
                     "metadata": {"name": f"{plural}.core.orka.ai"},
                     "spec": {"group": "core.orka.ai", "scope": "Namespaced",
                              "names": {"kind": "Fixture", "plural": plural},
                              "versions": [{"name": "v1alpha1", "served": True, "storage": True,
                                            "schema": {"openAPIV3Schema": send_schema(plural)}}]}}
        else:
            self.send_error(404)
            return
        self.reply(200, value)

    def frame(self, payload):
        length = len(payload)
        size = bytes([length]) if length < 126 else b"\x7e" + struct.pack("!H", length) if length < 65536 else b"\x7f" + struct.pack("!Q", length)
        self.wfile.write(b"\x82" + size + payload)
        self.wfile.flush()

    def forward(self):
        key = self.headers["Sec-WebSocket-Key"]
        accept = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        self.protocol_version = "HTTP/1.1"
        self.send_response(101, "Switching Protocols")
        for name, value in [("Connection", "Upgrade"), ("Upgrade", "websocket"), ("Sec-WebSocket-Accept", accept), ("Sec-WebSocket-Protocol", "v4.channel.k8s.io")]:
            self.send_header(name, value)
        self.end_headers()
        self.close_connection = True
        try:
            self.frame(b"\x00\x90\x1f")
            self.frame(b"\x01\x90\x1f")
            pending = b""
            while header := self.rfile.read(2):
                opcode, size = header
                if opcode & 15 == 8:
                    return
                length = size & 127
                if length == 126:
                    length = struct.unpack("!H", self.rfile.read(2))[0]
                elif length == 127:
                    length = struct.unpack("!Q", self.rfile.read(8))[0]
                if length > 1024 * 1024 or not size & 128:
                    return
                mask = self.rfile.read(4)
                data = self.rfile.read(length)
                data = bytes(v ^ mask[i % 4] for i, v in enumerate(data))
                if opcode & 15 != 2 or not data or data[0] != 0:
                    return
                pending += data[1:]
                while b"\r\n\r\n" in pending:
                    head, body = pending.split(b"\r\n\r\n", 1)
                    lines = head.decode().split("\r\n")
                    headers = {name.lower(): value for line in lines[1:] if ":" in line for name, value in [line.split(":", 1)]}
                    length = int(headers.get("content-length", "0"))
                    if len(body) < length:
                        break
                    method, route, _ = lines[0].split(" ")
                    status, value = self.orka(method, route, json.loads(body[:length]) if length else None, headers.get("authorization", "").strip())
                    if status is None:  # Synthetic response loss: close the forward without an acknowledgement.
                        return
                    encoded = json.dumps(value).encode()
                    reply = f"HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {len(encoded)}\r\n\r\n".encode() + encoded
                    self.frame(b"\x00" + reply)
                    pending = body[length:]
        except (BrokenPipeError, ConnectionResetError, OSError):
            pass

    def orka(self, method, route, body, authorization=""):
        query = parse_qs(urlsplit(route).query)
        route = urlsplit(route).path
        place = self.place(authorization)
        if route.endswith("/tools") and authorization == "Bearer unreadable-tools-fixture":
            return 403, {}
        if route.endswith("/tools") and authorization == "Bearer missing-tools-fixture":
            return 200, {"items": [], "metadata": {}}
        if authorization == "Bearer denied-fixture":
            return 403, {}
        if route.endswith("/auth/whoami") and authorization == "Bearer conditional-fixture":
            return 200, {"authenticated": True, "authType": "tokenReview", "namespace": ""}
        cancel = method == "POST" and route.startswith("/api/v1/tasks/") and route.endswith("/cancel")
        # Hold only the synthetic cancel response, never the state lock or ordinary reads.
        if cancel and self.cancel_modes.get(place) == "cancelling" and not self.cancel_ready.wait(5):
            return 503, {"error": "Synthetic cancellation capture was not released"}
        with self.lock:
            if place in self.offline:
                return 503, {"error": "Synthetic place offline"}
            if cancel:
                task = self.objects.get((place, route.removesuffix("/cancel")))
                if task is None:
                    return 404, {}
                if not isinstance(body, dict) or not body.get("uid"):
                    return 400, {}
                if body["uid"] != task["metadata"]["uid"] or task["status"]["phase"] in ["Succeeded", "Failed", "Cancelled"]:
                    return 409, {}
                mode = self.cancel_modes.get(place, "confirmed")
                if mode in ["denied", "unsupported", "unconfirmed"]:
                    return {"denied": 403, "unsupported": 405, "unconfirmed": None}[mode], {}
                if mode == "completed":
                    task["status"]["phase"] = "Succeeded"
                    return 409, {}
                if mode == "confirmed":
                    task["status"]["phase"] = "Cancelled"
                return 200, {"name": task["metadata"]["name"], "namespace": "agents", "requested": True,
                             "phase": task["status"]["phase"]}
            if method == "POST":
                kind = "Agent" if route.endswith("/agents") else "Task"
                if kind == "Agent":
                    self.agents[place] = self.agents.get(place, 0) + 1
                value = {"apiVersion": "core.orka.ai/v1alpha1", "kind": kind,
                         "metadata": {"name": body["name"], "namespace": "agents", "uid": "uid-" + body["name"], "generation": 1},
                         "spec": body["spec"], "status": {"phase": "Succeeded" if self.agents.get(place, 0) % 3 else "Running"}}
                if kind == "Task" and place in self.cancel_modes:
                    agent = body["spec"].get("agentRef")
                    earlier = any(scope == place and item["kind"] == "Task" and item["spec"].get("agentRef") == agent
                                  for (scope, _), item in self.objects.items())
                    value["status"]["phase"] = "Running" if earlier else "Succeeded"
                if kind == "Task" and "timeout" in value["spec"]:
                    value["spec"]["timeout"] = "2m0s"
                    value["spec"]["retryPolicy"].pop("maxRetries", None)
                self.objects[place, route + "/" + body["name"]] = value
                if kind == "Agent" and place in self.lose_receipt:
                    self.lose_receipt.remove(place)
                    return 503, {"error": "Synthetic lost create receipt"}
                return 201, value
            if route.endswith("/auth/whoami"):
                value = {"authenticated": True, "authType": "oidc", "namespace": "agents"}
            elif route.endswith("/providers"):
                value = {"items": [{"metadata": {"name": "fixture-model", "namespace": "agents", "uid": "provider-fixture"}, "status": {"ready": True}}], "metadata": {}}
            elif route.endswith("/tools"):
                value = {"items": [{"name": "web_fetch", "builtin": True}], "metadata": {}}
            elif route.endswith("/agents"):
                external = {"apiVersion": "core.orka.ai/v1alpha1", "kind": "Agent",
                            "metadata": {"name": "runtime-reader", "namespace": "agents", "uid": "runtime-" + place},
                            "spec": {"type": "runtime", "model": {"name": "external-model"}, "tools": [{"name": "web_fetch"}]}}
                agents = [external] + [item for (scope, _), item in self.objects.items() if scope == place and item["kind"] == "Agent"]
                value = {"items": agents if query.get("namespace", ["agents"]) == ["agents"] else [], "metadata": {}}
            elif route.endswith("/result"):
                value = {"result": "Fixture answer: cf699c56340dc775230fde4ef84237d27563ea6ef52164c7d078072b586c3918"}
            elif route.endswith("/events"):
                task = route.rsplit("/", 2)[1]
                agent = self.objects[place, route.rsplit("/", 1)[0]]["spec"]["agentRef"]["name"]
                event = {"namespace": "agents", "streamType": "task", "streamID": task, "seq": 1, "type": "ToolCallCompleted", "taskName": task, "agentName": agent, "toolName": "web_fetch", "toolCallID": "fixture-call"}
                value = {"namespace": "agents", "streamType": "task", "streamID": task, "afterSeq": 0, "latestSeq": 1, "events": [event]}
            elif (place, route) in self.objects:
                value = self.objects[place, route]
            else:
                return 404, {}
            return 200, value


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", 0), Discovery)
    # The caller supplies its CI scratch path; the bearer below is a synthetic fixture.
    Path(sys.argv[1]).write_text(json.dumps({
        "apiVersion": "v1", "kind": "Config", "current-context": "fixture-cluster",
        "clusters": [{"name": "fixture", "cluster": {"server": f"http://127.0.0.1:{server.server_port}"}}],
        "contexts": [{"name": name, "context": {"cluster": "fixture", "user": user}} for name, user in [("fixture-cluster", "fixture"), ("conditional-fixture", "conditional"), ("denied-fixture", "denied"), ("missing-tools-fixture", "missing-tools"), ("unreadable-tools-fixture", "unreadable-tools"), ("fixture-secondary", "secondary")]],
        "users": [{"name": name, "user": {"token": token}} for name, token in [("fixture", "ci-fixture-not-a-credential"), ("conditional", "conditional-fixture"), ("denied", "denied-fixture"), ("missing-tools", "missing-tools-fixture"), ("unreadable-tools", "unreadable-tools-fixture"), ("secondary", "secondary-fixture")]],
    }))
    server.serve_forever()
