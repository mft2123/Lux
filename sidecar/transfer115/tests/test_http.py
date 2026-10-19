"""HTTP contract tests: the exact JSON Lux sends and expects back."""

from __future__ import annotations

import json
import threading
import unittest
from http.client import HTTPConnection
from http.server import ThreadingHTTPServer

import main
from transfer115.service import TransferResult


class StubService:
    cookie = "cookie"

    def __init__(self):
        self.jobs = []

    def transfer(self, job):
        self.jobs.append(job)
        return TransferResult("transferred", "ok", ["某剧"])

    def check_login(self, client):
        return None

    def client(self):
        return object()


class HttpTests(unittest.TestCase):
    def setUp(self):
        self.stub = StubService()
        self.original = main.SERVICE
        main.SERVICE = self.stub
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), main.Handler)
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        main.SERVICE = self.original

    def request(self, method, path, body=None):
        connection = HTTPConnection("127.0.0.1", self.port, timeout=5)
        connection.request(method, path, body=body)
        response = connection.getresponse()
        payload = json.loads(response.read().decode("utf-8"))
        connection.close()
        return response.status, payload

    def test_transfer_round_trip(self):
        status, payload = self.request(
            "POST",
            "/transfer",
            json.dumps(
                {
                    "job_id": "job-1",
                    "url": "https://115.com/s/sw123?password=abcd",
                    "title": "某剧",
                    "archive_root": "/testlib",
                    "target_dir": "转存目录",
                    "cd2_prefix": "测试库",
                }
            ),
        )
        self.assertEqual(status, 200)
        self.assertEqual(payload["status"], "transferred")
        self.assertEqual(payload["saved"], ["某剧"])
        self.assertEqual(self.stub.jobs[0].cd2_prefix, "测试库")
        self.assertEqual(self.stub.jobs[0].target_dir, "转存目录")

    def test_missing_url_is_rejected(self):
        status, payload = self.request("POST", "/transfer", json.dumps({"job_id": "x"}))
        self.assertEqual(status, 400)
        self.assertFalse(payload["ok"])

    def test_broken_json_is_rejected(self):
        status, payload = self.request("POST", "/transfer", "{not json")
        self.assertEqual(status, 400)
        self.assertFalse(payload["ok"])

    def test_unknown_path(self):
        status, _ = self.request("GET", "/nope")
        self.assertEqual(status, 404)

    def test_healthz_shallow_stays_green(self):
        status, payload = self.request("GET", "/healthz")
        self.assertEqual(status, 200)
        self.assertEqual(payload["status"], "configured")


if __name__ == "__main__":
    unittest.main()
