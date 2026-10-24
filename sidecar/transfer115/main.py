#!/usr/bin/env python3
"""HTTP front end of the 115 transfer sidecar.

Endpoints (all JSON):

- ``POST /transfer``  — body ``{job_id, url, title, archive_root, target_dir,
  cd2_prefix}``; answers ``{status, detail, saved}`` where status is one of
  ``transferred`` / ``auth_required`` / ``failed`` / ``unsupported``.
- ``POST /offline``   — body ``{job_id, url, title, target_dir, cd2_prefix}``
  for a magnet / ed2k link; answers ``{status, detail}`` where status is one of
  ``submitted`` / ``auth_required`` / ``failed`` / ``unsupported``. ``submitted``
  only means 115 accepted the cloud-download task.
- ``GET /healthz``    — shallow probe (process up, cookie configured). Add
  ``?deep=1`` to validate the cookie against 115.
- ``GET /``           — one-line description.

The share link is never logged in full: it can carry the share password.
"""

from __future__ import annotations

import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import parse_qs, urlparse

from p115client.exception import P115AuthenticationError

from transfer115 import (
    STATUS_AUTH_REQUIRED,
    OfflineJob,
    TransferJob,
    TransferService,
    load_cookie,
)

DEFAULT_CONFIG_DIR = "/config"
DEFAULT_HOST = "0.0.0.0"
DEFAULT_PORT = 8098


def _float_env(name: str, default: float) -> float:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        return float(raw)
    except ValueError:
        return default


def _int_env(name: str, default: int) -> int:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def build_service() -> TransferService:
    config_dir = os.environ.get("TRANSFER115_CONFIG_DIR", DEFAULT_CONFIG_DIR)
    cookie = os.environ.get("LUX_115_COOKIE", "").strip()
    if not cookie:
        try:
            cookie = load_cookie(config_dir)
        except (OSError, ValueError) as error:
            print(f"[transfer115] cannot read {config_dir}: {error}", file=sys.stderr, flush=True)
            cookie = ""
    return TransferService(
        cookie=cookie,
        confirm_seconds=_float_env("TRANSFER115_CONFIRM_SECONDS", 30.0),
        poll_interval=_float_env("TRANSFER115_POLL_INTERVAL", 2.0),
        is_check=_int_env("TRANSFER115_IS_CHECK", 0),
    )


SERVICE = build_service()
#: One transfer at a time: the 115 account is a single session, and a burst of
#: share receives would just compete with each other.
TRANSFER_LOCK = threading.Lock()


def _host_of(url: str) -> str:
    try:
        return urlparse(url).netloc or "(no host)"
    except ValueError:
        return "(bad url)"


class Handler(BaseHTTPRequestHandler):
    server_version = "transfer115"

    def log_message(self, fmt: str, *args: Any) -> None:  # noqa: D102 - quieter logs
        print(f"[transfer115] {self.address_string()} {fmt % args}", flush=True)

    def _send(self, status: int, payload: dict) -> None:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json; charset=utf-8")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        query = parse_qs(parsed.query)
        if parsed.path == "/healthz":
            deep = query.get("deep", ["0"])[0] in ("1", "true", "yes")
            if not deep:
                # The process is up. A missing cookie is a configuration problem
                # the operator fixes in the settings file, not a health failure:
                # report it in the body so the probe stays green.
                self._send(200, {"ok": True, "status": "configured" if SERVICE.cookie else "unconfigured"})
                return
            if not SERVICE.cookie:
                self._send(503, {"ok": False, "status": "unconfigured", "detail": "未配置 115 cookie"})
                return
            try:
                SERVICE.check_login(SERVICE.client())
            except P115AuthenticationError as error:
                self._send(503, {"ok": False, "status": STATUS_AUTH_REQUIRED, "detail": str(error)})
                return
            except Exception as error:  # pragma: no cover - network/upstream
                self._send(503, {"ok": False, "status": "failed", "detail": str(error)})
                return
            self._send(200, {"ok": True, "status": "logged_in"})
            return
        if parsed.path == "/":
            self._send(
                200,
                {
                    "service": "transfer115",
                    "endpoints": ["POST /transfer", "GET /healthz", "GET /"],
                    "cookieConfigured": bool(SERVICE.cookie),
                },
            )
            return
        self._send(404, {"ok": False, "detail": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path not in ("/transfer", "/offline"):
            self._send(404, {"ok": False, "detail": "not found"})
            return
        try:
            length = int(self.headers.get("content-length") or 0)
        except ValueError:
            length = 0
        if length <= 0 or length > 1_000_000:
            self._send(400, {"ok": False, "detail": "请求体缺失或过大"})
            return
        raw = self.rfile.read(length)
        try:
            body = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, ValueError):
            self._send(400, {"ok": False, "detail": "请求体不是合法 JSON"})
            return
        if not isinstance(body, dict) or not isinstance(body.get("url"), str) or not body["url"].strip():
            self._send(400, {"ok": False, "detail": "缺少 url"})
            return
        if parsed.path == "/offline":
            offline_job = OfflineJob(
                job_id=str(body.get("job_id") or ""),
                url=body["url"].strip(),
                title=str(body.get("title") or ""),
                target_dir=str(body.get("target_dir") or ""),
                cd2_prefix=str(body.get("cd2_prefix") or ""),
            )
            # Only the scheme is logged: a magnet link carries the info hash and
            # an ed2k link carries the file name.
            scheme = offline_job.url.split(":", 1)[0] if ":" in offline_job.url else "(no scheme)"
            print(
                f"[transfer115] offline job={offline_job.job_id or '(none)'} scheme={scheme}",
                flush=True,
            )
            with TRANSFER_LOCK:
                result = SERVICE.offline(offline_job)
            print(
                f"[transfer115] offline job={offline_job.job_id or '(none)'} status={result.status}",
                flush=True,
            )
            self._send(200, {"status": result.status, "detail": result.detail})
            return
        job = TransferJob(
            job_id=str(body.get("job_id") or ""),
            url=body["url"].strip(),
            title=str(body.get("title") or ""),
            archive_root=str(body.get("archive_root") or ""),
            target_dir=str(body.get("target_dir") or ""),
            cd2_prefix=str(body.get("cd2_prefix") or ""),
        )
        print(
            f"[transfer115] transfer job={job.job_id or '(none)'} host={_host_of(job.url)}",
            flush=True,
        )
        with TRANSFER_LOCK:
            result = SERVICE.transfer(job)
        print(f"[transfer115] transfer job={job.job_id or '(none)'} status={result.status}", flush=True)
        self._send(200, {"status": result.status, "detail": result.detail, "saved": result.saved})


def main() -> None:
    host = os.environ.get("TRANSFER115_HOST", DEFAULT_HOST)
    port = _int_env("TRANSFER115_PORT", DEFAULT_PORT)
    server = ThreadingHTTPServer((host, port), Handler)
    print(f"[transfer115] listening on {host}:{port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:  # pragma: no cover
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
