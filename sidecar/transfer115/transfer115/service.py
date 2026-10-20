"""Core logic of the 115 transfer sidecar.

The HTTP layer (``main.py``) is a thin shell around :class:`TransferService`.
Every network call goes through a ``p115client`` client that is injected through
``client_factory`` so the whole flow can be tested offline.
"""

from __future__ import annotations

import json
import os
import re
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable, Sequence
from urllib.parse import parse_qs, urlparse

from p115client import P115Client
from p115client.exception import P115AuthenticationError, P115Error

SETTINGS_FILENAME = "archive-settings.json"

STATUS_TRANSFERRED = "transferred"
STATUS_FAILED = "failed"
STATUS_AUTH_REQUIRED = "auth_required"
STATUS_UNSUPPORTED = "unsupported"

#: Hosts whose ``/s/<code>`` paths are 115 share links. The Rust side only sends
#: 115.com links here, but the sidecar accepts the whole family so a link posted
#: by a different 115 front end still works.
SHARE_HOSTS = frozenset({"115.com", "115cdn.com", "anxia.com", "115.link", "115.com.cn"})

#: Query parameters 115 uses for the receive code (share password).
RECEIVE_CODE_KEYS = ("password", "pwd", "receive_code")

#: The receive code is a short ASCII token. Share links are routinely copied
#: together with the poster's note (``?password=x3c1，请及时检查…``), so the query
#: value can carry trailing prose that must not be sent to 115.
RECEIVE_CODE_TOKEN = re.compile(r"[A-Za-z0-9]+")

#: errno values 115 answers with when the session is gone (``登录超时，请重新登录。``).
#: They arrive as ``{"state": false, ...}`` on an HTTP 200, not as an exception.
AUTH_ERRNOS = frozenset({990001})


class TransferRejected(Exception):
    """The transfer cannot be performed; reported to Lux as ``failed``."""

    def __init__(self, detail: str) -> None:
        super().__init__(detail)
        self.detail = detail


@dataclass(frozen=True)
class ShareLink:
    share_code: str
    receive_code: str


@dataclass
class TransferJob:
    """Exactly what Lux sends in the ``POST /transfer`` body."""

    job_id: str
    url: str
    title: str
    archive_root: str
    target_dir: str
    cd2_prefix: str


@dataclass
class TransferResult:
    status: str
    detail: str = ""
    saved: list[str] = field(default_factory=list)


def parse_share_link(url: str) -> ShareLink | None:
    """Return the share code and receive code of a 115 share link, else ``None``."""
    parsed = urlparse((url or "").strip())
    if parsed.scheme not in ("http", "https"):
        return None
    host = (parsed.hostname or "").lower()
    if host not in SHARE_HOSTS and not host.endswith(".115.com"):
        return None
    path = parsed.path.rstrip("/")
    if not path.startswith("/s/") or len(path) <= 3:
        return None
    share_code = path[3:].split("/", 1)[0]
    if not share_code:
        return None
    query = parse_qs(parsed.query)
    receive_code = ""
    for key in RECEIVE_CODE_KEYS:
        values = query.get(key)
        if values and values[0]:
            match = RECEIVE_CODE_TOKEN.match(values[0].strip())
            receive_code = match.group(0) if match else ""
            break
    return ShareLink(share_code=share_code, receive_code=receive_code)


def load_cookie(config_dir: str) -> str:
    """Read ``transfer115.cookie`` from the settings file Lux shares with us.

    The settings file is owned by Lux and is mode 0600, so it is mounted read
    only into this container and the sidecar runs with a group that can read it
    (see README). A value that is just the API's echoed mask is not a cookie.
    """
    path = os.path.join(config_dir, SETTINGS_FILENAME)
    with open(path, "r", encoding="utf-8") as handle:
        document = json.load(handle)
    section = document.get("transfer115")
    if not isinstance(section, dict):
        return ""
    cookie = section.get("cookie")
    if not isinstance(cookie, str):
        return ""
    cookie = cookie.strip()
    if not cookie or cookie.startswith("***") or cookie.lower() == "masked":
        return ""
    return cookie


def _items_of(payload: Any) -> list[dict]:
    """Pull the item list out of a p115client response, whatever wrapper it uses."""
    if not isinstance(payload, dict):
        return []
    for key in ("list", "data"):
        value = payload.get(key)
        if isinstance(value, list):
            return [item for item in value if isinstance(item, dict)]
        if isinstance(value, dict):
            for nested in ("list", "files"):
                nested_value = value.get(nested)
                if isinstance(nested_value, list):
                    return [item for item in nested_value if isinstance(item, dict)]
    return []


def _name_of(item: dict) -> str:
    for key in ("file_name", "name", "n"):
        value = item.get(key)
        if isinstance(value, str) and value:
            return value
    return ""


def _id_of(item: dict) -> str:
    for key in ("file_id", "fid", "cid"):
        value = item.get(key)
        if value not in (None, ""):
            return str(value)
    return ""


def _is_dir_of(item: dict) -> bool:
    value = item.get("is_dir")
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    if isinstance(value, str) and value:
        return value.lower() in ("1", "true", "yes")
    # 115's own listing does not send ``is_dir``: a folder carries ``cid`` and no
    # ``fid``, while a file carries ``fid`` (its ``cid`` is then the parent and is
    # often "0"). Checking ``fid`` is therefore the only reliable discriminator.
    return not item.get("fid")


def _raise_for_state(payload: Any) -> None:
    """Turn 115's in-body failures into the right exception.

    A dead session is reported as ``state: false`` with errno 990001 on an HTTP
    200, and cookies that are merely invalid make ``login_status()`` return
    ``False`` rather than raise — so every answer has to be inspected here, or a
    lost job is reported as ``failed`` instead of being kept for a retry.
    """
    if not isinstance(payload, dict) or payload.get("state") is not False:
        return
    errno = payload.get("errno") or payload.get("errNo") or 0
    message = str(payload.get("error") or "115 接口返回失败")
    if errno in AUTH_ERRNOS or "登录" in message:
        raise P115AuthenticationError(message)
    raise TransferRejected(message)


def _new_id_of(payload: Any) -> str:
    if not isinstance(payload, dict):
        return ""
    data = payload.get("data") if isinstance(payload.get("data"), dict) else payload
    return _id_of(data)


@dataclass
class TransferService:
    """Receive 115 share links into the archive target directory."""

    cookie: str
    client_factory: Callable[[], Any] | None = None
    confirm_seconds: float = 30.0
    poll_interval: float = 2.0
    is_check: int = 0
    page_size: int = 100

    _client: Any = field(default=None, repr=False)

    # -- client ---------------------------------------------------------------

    def client(self) -> Any:
        if self._client is None:
            factory = self.client_factory or (lambda: P115Client(self.cookie))
            self._client = factory()
        return self._client

    def check_login(self, client: Any) -> None:
        """Raise ``P115AuthenticationError`` when the cookie no longer works.

        ``p115client`` reports a dead session as the bare ``False`` (not a dict,
        not an exception), which is why the truthiness check comes first.
        """
        status = client.login_status()
        if status is False or status is None:
            raise P115AuthenticationError("115 登录状态已失效")
        if isinstance(status, dict):
            data = status.get("data")
            if isinstance(data, dict) and data.get("login") is False:
                raise P115AuthenticationError("115 登录状态已失效")

    # -- target directory -----------------------------------------------------

    def resolve_target_cid(self, client: Any, relative_path: str) -> str:
        """Walk ``cd2_prefix/target_dir`` from the account root, creating gaps."""
        cid = "0"
        segments = [
            segment
            for segment in (relative_path or "").replace("\\", "/").split("/")
            if segment and segment != "."
        ]
        for segment in segments:
            cid = self._child_dir(client, cid, segment)
        return cid

    def _child_dir(self, client: Any, cid: str, name: str) -> str:
        for item in self._list_dir(client, cid):
            if _is_dir_of(item) and _name_of(item) == name:
                return _id_of(item)
        created = client.fs_mkdir(name, pid=cid)
        _raise_for_state(created)
        new_id = _new_id_of(created)
        if not new_id:
            raise TransferRejected(f"创建 115 目录 {name!r} 失败：接口没有返回目录 id")
        return new_id

    def _list_dir(self, client: Any, cid: str) -> list[dict]:
        payload = client.fs_files({"cid": cid, "limit": self.page_size})
        _raise_for_state(payload)
        return _items_of(payload)

    # -- transfer -------------------------------------------------------------

    def transfer(self, job: TransferJob) -> TransferResult:
        link = parse_share_link(job.url)
        if link is None:
            return TransferResult(
                STATUS_UNSUPPORTED,
                "不是 115 分享链接（只处理 115.com/s/<code> 形式的链接）",
            )
        try:
            client = self.client()
            self.check_login(client)
            items = self._share_root_items(client, link)
            if not items:
                raise TransferRejected("分享为空或无法读取（可能已被取消或需要提取码）")
            file_ids = ",".join(_id_of(item) for item in items if _id_of(item))
            if not file_ids:
                raise TransferRejected("分享条目没有 file_id")
            target = "/".join(
                part.strip("/") for part in (job.cd2_prefix, job.target_dir) if part and part.strip("/")
            )
            if not target:
                raise TransferRejected("转存目标目录为空")
            cid = self.resolve_target_cid(client, target)
            self._receive(client, link, file_ids, cid)
            saved = [name for name in (_name_of(item) for item in items) if name]
            detail = self._confirm_saved(client, cid, saved)
            return TransferResult(STATUS_TRANSFERRED, detail, saved)
        except P115AuthenticationError as error:
            # The cookie is dead: Lux keeps the job queued and retries after the
            # operator refreshes it.
            return TransferResult(STATUS_AUTH_REQUIRED, f"115 cookie 无效或已过期: {error}")
        except TransferRejected as error:
            return TransferResult(STATUS_FAILED, error.detail)
        except P115Error as error:
            return TransferResult(STATUS_FAILED, f"115 接口错误: {error}")
        except Exception as error:  # pragma: no cover - defensive boundary
            return TransferResult(STATUS_FAILED, f"转存失败: {type(error).__name__}: {error}")

    def _share_root_items(self, client: Any, link: ShareLink) -> list[dict]:
        payload = client.share_snap(
            {
                "share_code": link.share_code,
                "receive_code": link.receive_code,
                "cid": 0,
                "limit": self.page_size,
            }
        )
        items = _items_of(payload)
        _raise_for_state(payload)
        if not items:
            raise TransferRejected("分享内容为空或无法读取")
        return items

    def _receive(self, client: Any, link: ShareLink, file_ids: str, cid: str) -> None:
        payload = client.share_receive(
            {
                "share_code": link.share_code,
                "receive_code": link.receive_code,
                "file_id": file_ids,
                "cid": cid,
                "is_check": self.is_check,
            }
        )
        _raise_for_state(payload)

    def _confirm_saved(self, client: Any, cid: str, expected: Sequence[str]) -> str:
        """Wait briefly for the share to land, so the log says what happened.

        A share that is still being processed by 115 is not an error: the receive
        call succeeded, and CD2 will surface the files when 115 finishes.
        """
        if self.confirm_seconds <= 0 or not expected:
            return "已提交转存任务"
        deadline = time.monotonic() + self.confirm_seconds
        expected_set = set(expected)
        while True:
            try:
                names = {_name_of(item) for item in self._list_dir(client, cid)}
            except P115AuthenticationError:
                raise
            except Exception:
                names = set()
            if expected_set & names:
                return f"已转存 {len(expected_set & names)} 个条目"
            if time.monotonic() >= deadline:
                return "已提交转存任务，等待 115 完成"
            time.sleep(self.poll_interval)
