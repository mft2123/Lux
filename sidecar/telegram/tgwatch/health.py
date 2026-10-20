"""Read-only HTTP probe for the Telegram sidecar.

Lux cannot look inside this container: the worker owns the Telethon session and
only ``main.py check`` can validate it. ``/healthz`` gives Lux one stable thing to
call, and doubles as the container healthcheck:

- ``GET /healthz``         process is up and the configuration is complete
- ``GET /healthz?deep=1``  additionally asks Telegram whether the session is still
                           authorized (the same check as ``check --deep``)

The answer never contains the phone number, api_id/hash or session contents; the
config module already renders those masked.
"""

from __future__ import annotations

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import parse_qs, urlparse

from . import config as config_mod
from . import runner

STATUS_CONFIGURED = "configured"
STATUS_UNCONFIGURED = "unconfigured"
STATUS_LOGGED_IN = "logged_in"
STATUS_AUTH_REQUIRED = "auth_required"
STATUS_FAILED = "failed"


def probe(config: config_mod.SidecarConfig, deep: bool) -> tuple[int, dict[str, Any]]:
    """Answer one probe. Pure enough to unit-test without a socket."""
    problems = config_mod.runtime_problems(config)
    if problems:
        return 503, {
            "ok": False,
            "status": STATUS_UNCONFIGURED,
            "detail": "；".join(problems),
        }
    if not deep:
        return 200, {"ok": True, "status": STATUS_CONFIGURED, "detail": ""}
    try:
        ok, detail = asyncio.run(runner.check_session(config))
    except Exception as error:  # noqa: BLE001 - a probe must answer, never raise
        return 503, {
            "ok": False,
            "status": STATUS_FAILED,
            "detail": f"{type(error).__name__}: {error}",
        }
    return (200 if ok else 503), {
        "ok": ok,
        "status": STATUS_LOGGED_IN if ok else STATUS_AUTH_REQUIRED,
        "detail": detail,
    }


def make_handler(config: config_mod.SidecarConfig) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        server_version = "lux-telegram-health"

        def log_message(self, fmt: str, *args: Any) -> None:  # noqa: D102 - quiet
            return

        def do_GET(self) -> None:  # noqa: N802 - http.server API
            parsed = urlparse(self.path)
            if parsed.path != "/healthz":
                self._send(404, {"ok": False, "status": "not_found", "detail": ""})
                return
            deep = parse_qs(parsed.query).get("deep", ["0"])[0].lower() in (
                "1",
                "true",
                "yes",
            )
            status, body = probe(config, deep)
            self._send(status, body)

        def _send(self, status: int, body: dict[str, Any]) -> None:
            raw = json.dumps(body, ensure_ascii=False).encode("utf-8")
            self.send_response(status)
            self.send_header("content-type", "application/json; charset=utf-8")
            self.send_header("content-length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

    return Handler


def serve(
    config: config_mod.SidecarConfig,
    port: int,
    host: str = "0.0.0.0",
) -> ThreadingHTTPServer | None:
    """Start the probe on a daemon thread. ``port <= 0`` disables it."""
    if port <= 0:
        return None
    server = ThreadingHTTPServer((host, port), make_handler(config))
    thread = threading.Thread(target=server.serve_forever, name="tg-health", daemon=True)
    thread.start()
    return server
