"""End-to-end tests for capture.py and cadri-login against a fake Cadri server."""

from __future__ import annotations

import json
import os
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

PLUGIN = Path(__file__).resolve().parent.parent
CAPTURE = PLUGIN / "hooks" / "capture.py"
LOGIN = PLUGIN / "bin" / "cadri-login"


class FakeCadri:
    """Serves queued (status, body) responses per path and records every request."""

    def __init__(self):
        self.responses: dict[str, list[tuple[int, dict]]] = {}
        self.requests: list[dict] = []
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                length = int(self.headers.get("Content-Length") or 0)
                body = json.loads(self.rfile.read(length) or b"{}")
                fake.requests.append({"path": self.path, "headers": dict(self.headers), "body": body})
                queue = fake.responses.get(self.path) or [(200, {})]
                status, payload = queue.pop(0) if len(queue) > 1 else queue[0]
                data = json.dumps(payload).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def respond(self, path: str, *responses: tuple[int, dict]) -> None:
        self.responses[path] = list(responses)

    def requests_to(self, path: str) -> list[dict]:
        return [request for request in self.requests if request["path"] == path]

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


class PluginTestCase(unittest.TestCase):
    def setUp(self):
        self.server = FakeCadri()
        self.addCleanup(self.server.close)
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name)
        self.credentials = self.home / ".config" / "cadri" / "credentials.json"
        self.transcript = self.home / "session.jsonl"
        self.transcript.write_text('{"type":"user"}\n', encoding="utf-8")

    def env(self, **extra: str) -> dict:
        env = {k: v for k, v in os.environ.items() if not k.startswith("CADRI_")}
        env.update({"HOME": str(self.home), "CADRI_URL": self.server.url, **extra})
        return env

    def capture(self, **env: str) -> subprocess.CompletedProcess:
        event = {
            "session_id": "s1",
            "transcript_path": str(self.transcript),
            "cwd": "/work",
            "hook_event_name": "Stop",
        }
        return subprocess.run(
            [sys.executable, str(CAPTURE)],
            input=json.dumps(event),
            env=self.env(**env),
            capture_output=True,
            text=True,
            timeout=30,
        )

    def write_credentials(self, entry: dict, key: str | None = None) -> None:
        self.credentials.parent.mkdir(parents=True)
        self.credentials.write_text(json.dumps({key or self.server.url: entry}), encoding="utf-8")
        self.credentials.chmod(0o600)

    def uploads(self) -> list[dict]:
        return self.server.requests_to("/api/transcripts")

    def mode(self) -> int:
        return stat.S_IMODE(self.credentials.stat().st_mode)


class CaptureTests(PluginTestCase):
    def test_api_token_env_sets_bearer_header(self):
        result = self.capture(CADRI_API_TOKEN="secret")
        self.assertEqual(result.returncode, 0, result.stderr)
        [upload] = self.uploads()
        self.assertEqual(upload["headers"]["Authorization"], "Bearer secret")
        self.assertEqual(upload["body"]["session_id"], "s1")
        self.assertEqual(upload["body"]["content"], '{"type":"user"}\n')
        self.assertEqual(upload["body"]["hook_event"], "Stop")

    def test_no_credentials_sends_no_header(self):
        result = self.capture()
        self.assertEqual(result.returncode, 0, result.stderr)
        [upload] = self.uploads()
        self.assertNotIn("Authorization", upload["headers"])

    def test_credentials_file_sets_bearer_header(self):
        self.write_credentials({"access_token": "a1", "refresh_token": "r1", "expires_at": time.time() + 3600})
        result = self.capture(CADRI_URL=f"{self.server.url}/")
        self.assertEqual(result.returncode, 0, result.stderr)
        [upload] = self.uploads()
        self.assertEqual(upload["headers"]["Authorization"], "Bearer a1")
        self.assertEqual(self.server.requests_to("/auth/refresh"), [])

    def test_refreshes_token_near_expiry(self):
        self.write_credentials({"access_token": "old", "refresh_token": "r1", "expires_at": time.time() + 30})
        expires_at = int(time.time()) + 3600
        self.server.respond(
            "/auth/refresh",
            (200, {"access_token": "new", "refresh_token": "r2", "expires_at": expires_at}),
        )
        result = self.capture()
        self.assertEqual(result.returncode, 0, result.stderr)
        [refresh] = self.server.requests_to("/auth/refresh")
        self.assertEqual(refresh["body"], {"refresh_token": "r1"})
        [upload] = self.uploads()
        self.assertEqual(upload["headers"]["Authorization"], "Bearer new")
        saved = json.loads(self.credentials.read_text(encoding="utf-8"))
        self.assertEqual(
            saved,
            {self.server.url: {"access_token": "new", "refresh_token": "r2", "expires_at": expires_at}},
        )
        self.assertEqual(self.mode(), 0o600)
        self.assertEqual(list(self.credentials.parent.iterdir()), [self.credentials])

    def test_refresh_failure_still_uploads(self):
        entry = {"access_token": "old", "refresh_token": "r1", "expires_at": time.time() - 10}
        self.write_credentials(entry)
        self.server.respond("/auth/refresh", (401, {"error": "unauthorized"}))
        result = self.capture()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("refresh failed", result.stderr)
        [upload] = self.uploads()
        self.assertEqual(upload["headers"]["Authorization"], "Bearer old")
        saved = json.loads(self.credentials.read_text(encoding="utf-8"))
        self.assertEqual(saved[self.server.url]["access_token"], "old")

    def test_rejected_upload_exits_zero(self):
        self.server.respond("/api/transcripts", (401, {"error": "unauthorized"}))
        result = self.capture()
        self.assertEqual(result.returncode, 0)
        self.assertIn("401", result.stderr)

    def test_unreachable_server_exits_zero(self):
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        result = self.capture(CADRI_URL=f"http://127.0.0.1:{port}")
        self.assertEqual(result.returncode, 0)
        self.assertIn("could not reach", result.stderr)

    def test_invalid_hook_input_exits_zero(self):
        result = subprocess.run(
            [sys.executable, str(CAPTURE)],
            input="not json",
            env=self.env(),
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("invalid hook input", result.stderr)


class LoginTests(PluginTestCase):
    def login(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, str(LOGIN), "--no-browser", *args],
            env=self.env(),
            capture_output=True,
            text=True,
            timeout=30,
        )

    def test_device_flow_happy_path(self):
        self.server.respond(
            "/auth/device/start",
            (
                200,
                {
                    "device_code": "dev1",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://auth.example/activate",
                    "verification_uri_complete": "https://auth.example/activate?code=ABCD-EFGH",
                    "expires_in": 300,
                    "interval": 0,
                },
            ),
        )
        self.server.respond(
            "/auth/device/token",
            (428, {"error": "authorization_pending"}),
            (200, {"access_token": "a1", "refresh_token": "r1", "expires_at": 2000000000}),
        )
        result = self.login("--url", f"{self.server.url}/")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ABCD-EFGH", result.stdout)
        self.assertIn("https://auth.example/activate?code=ABCD-EFGH", result.stdout)
        polls = self.server.requests_to("/auth/device/token")
        self.assertEqual([poll["body"] for poll in polls], [{"device_code": "dev1"}] * 2)
        saved = json.loads(self.credentials.read_text(encoding="utf-8"))
        self.assertEqual(
            saved,
            {self.server.url: {"access_token": "a1", "refresh_token": "r1", "expires_at": 2000000000}},
        )
        self.assertEqual(self.mode(), 0o600)

    def test_denied_exits_one(self):
        self.server.respond(
            "/auth/device/start",
            (200, {"device_code": "dev1", "user_code": "X", "verification_uri_complete": "u", "interval": 0}),
        )
        self.server.respond("/auth/device/token", (400, {"error": "access_denied"}))
        result = self.login()
        self.assertEqual(result.returncode, 1)
        self.assertIn("denied", result.stderr)
        self.assertFalse(self.credentials.exists())

    def test_logout_removes_only_that_server(self):
        self.write_credentials({"access_token": "a1", "refresh_token": "r1", "expires_at": 1})
        data = json.loads(self.credentials.read_text(encoding="utf-8"))
        data["https://other.example"] = {"access_token": "b"}
        self.credentials.write_text(json.dumps(data), encoding="utf-8")
        result = self.login("--logout")
        self.assertEqual(result.returncode, 0, result.stderr)
        saved = json.loads(self.credentials.read_text(encoding="utf-8"))
        self.assertEqual(list(saved), ["https://other.example"])


if __name__ == "__main__":
    unittest.main()
