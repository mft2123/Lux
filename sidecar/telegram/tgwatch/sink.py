"""投递通道（sink）：把通过过滤的链接交给 Lux。

两种实现，`LUX_TG_SINK` 切换：

1. `http`（默认）——POST 到 Lux 的 `POST /api/v1/admin/archive/ingest`，body 见 `payload()`
   （url/title/mode + 溯源字段 source/kind/channel/message_id/size_bytes/captured_at），
   带 `X-Lux-Api-Key`。Lux 校验后写入
   `<archive_root>/链接队列/<ts>-<slug>.json`，流水线下一轮 `ingest_link_jobs()`
   把它记进 `状态.jsonl`（当前 115 转存客户端未实现，状态是 `needs_115_client`）。
2. `inbox`——写入共享目录 `/watch-inbox/<ts>-<slug>.txt`（运维/离线诊断用；
   Lux 不会消费它来转存，详见 README「已知边界」）。
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from .links import slugify

log = logging.getLogger("tgwatch.sink")

# 投递来源标记：Lux 的 LinkJob.source，用来区分「谁把这个链接送进来的」
SOURCE_TELEGRAM = "telegram"


class SinkError(RuntimeError):
    """投递失败。

    `permanent=True` 表示重试也不会成功（Lux 400 链接非法 / 403 无权限），
    runner 直接把它落死信（review SC4）；其余错误进重试队列。
    """

    def __init__(self, message: str, *, permanent: bool = False):
        super().__init__(message)
        self.permanent = permanent


def payload(item: dict) -> dict:
    """Lux `POST /api/v1/admin/archive/ingest` 的 body。

    与 Lux 的 `LinkJob`（`src/application/archive.rs`）逐字段对齐：`url` + `title` + `mode`，
    再加溯源字段 `source/kind/channel/message_id/size_bytes/captured_at`（review SC6），
    这样 `链接队列/状态.jsonl` 的每条记录都能回溯到频道与原始消息。
    """
    msg_id = item.get("msg_id")
    return {
        "url": item.get("url"),
        "title": item.get("title") or item.get("anchor") or "",
        "mode": item.get("mode"),
        "source": item.get("source") or SOURCE_TELEGRAM,
        "kind": item.get("kind") or "",
        "channel": item.get("channel") or "",
        "message_id": int(msg_id) if msg_id is not None else None,
        "size_bytes": item.get("size_bytes"),
        "captured_at": item.get("msg_date") or "",
    }


def inbox_body(item: dict) -> str:
    """`<ts>-<slug>.txt` 的内容：第一行是链接，其余为注释行。"""
    lines = [
        str(item.get("url") or ""),
        f"# mode={item.get('mode')}",
        f"# kind={item.get('kind')}",
        f"# title={item.get('title') or ''}",
        f"# channel={item.get('channel')} message_id={item.get('msg_id')}",
        f"# captured_at={item.get('msg_date') or ''}",
        "",
    ]
    return "\n".join(lines)


def inbox_filename(item: dict, now: float | None = None) -> str:
    """`<ts>-<slug>.txt`：ts 取投递时刻（本地时区，可读、可排序）。"""
    stamp = time.strftime("%Y%m%d-%H%M%S", time.localtime(now if now is not None else time.time()))
    return f"{stamp}-{slugify(item.get('title') or '')}.txt"


class Sink(Protocol):
    name: str

    async def send(self, item: dict) -> None: ...

    def describe(self) -> dict: ...


@dataclass
class HttpSink:
    url: str
    api_key: str = ""
    timeout: float = 15.0
    name: str = "http"

    def _post(self, body: bytes) -> tuple[int, str]:
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["X-Lux-Api-Key"] = self.api_key
        request = urllib.request.Request(self.url, data=body, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:  # noqa: S310 - 自填 URL
                return response.status, response.read(4096).decode("utf-8", "replace")
        except urllib.error.HTTPError as error:
            try:
                detail = error.read(512).decode("utf-8", "replace")
            finally:
                error.close()
            if error.code == 401:
                raise SinkError("Lux 拒绝：X-Lux-Api-Key 无效（检查 telegram.api_key）") from error
            if error.code in (400, 403):
                # 400 链接被判非法、403 管理员接口无权限：重试多少次结果都一样（review SC4）
                raise SinkError(
                    f"Lux 永久拒绝（{error.code}，不再重试）：{detail[:200]}", permanent=True
                ) from error
            raise SinkError(f"HTTP {error.code}: {detail[:200]}") from error
        except urllib.error.URLError as error:
            raise SinkError(f"连接 Lux 失败: {error.reason}") from error
        except TimeoutError as error:
            raise SinkError(f"Lux 超时: {self.timeout}s") from error

    async def send(self, item: dict) -> None:
        body = json.dumps(payload(item), ensure_ascii=False).encode("utf-8")
        status, text = await asyncio.to_thread(self._post, body)
        if status < 200 or status >= 300:
            raise SinkError(f"HTTP {status}: {text[:200]}")
        try:
            outcome = json.loads(text) if text.strip() else {}
        except ValueError:
            outcome = {}
        if outcome.get("duplicate"):
            log.info("Lux 已在队列里（幂等命中）: job=%s", outcome.get("jobId"))
        else:
            log.debug("Lux 已入队: job=%s file=%s", outcome.get("jobId"), outcome.get("file"))

    def describe(self) -> dict:
        return {"sink": self.name, "url": self.url, "authorized": bool(self.api_key)}


@dataclass
class InboxSink:
    directory: Path
    name: str = "inbox"

    def __post_init__(self) -> None:
        self.directory = Path(self.directory)
        self.directory.mkdir(parents=True, exist_ok=True)

    def _write(self, item: dict) -> Path:
        base = self.directory / inbox_filename(item)
        target = base
        # 同一秒同标题（批量投递很常见）时顺延编号，绝不覆盖已有文件
        for index in range(2, 100):
            if not target.exists():
                break
            target = base.with_name(f"{base.stem}-{index}{base.suffix}")
        temp = target.with_name(f".{target.name}.tmp")
        # 0644：消费方可能是别的用户；0600 会让运维/pr 进程读不到
        fd = os.open(temp, os.O_CREAT | os.O_TRUNC | os.O_WRONLY, 0o644)
        try:
            os.write(fd, inbox_body(item).encode("utf-8"))
            os.fsync(fd)
        finally:
            os.close(fd)
        os.replace(temp, target)
        return target

    async def send(self, item: dict) -> None:
        try:
            await asyncio.to_thread(self._write, item)
        except OSError as error:
            raise SinkError(f"写 {self.directory} 失败: {error}") from error

    def describe(self) -> dict:
        return {"sink": self.name, "directory": str(self.directory)}


class NullSink:
    """dry-run：只走判定与记录，不落地。"""

    name = "null"

    async def send(self, item: dict) -> None:  # pragma: no cover - dry-run 用
        return None

    def describe(self) -> dict:
        return {"sink": self.name, "dry_run": True}


def build(config) -> Sink:
    if getattr(config, "dry_run", False):
        return NullSink()
    if config.sink == "inbox":
        return InboxSink(directory=config.inbox_dir)
    return HttpSink(url=config.ingest_url, api_key=config.api_key, timeout=config.http_timeout)


def lookup_size(url: str, api_key: str, title: str, timeout: float = 15.0) -> dict:
    """GET `…/archive/lookup?title=…`：洗版模式问「库中同名版本多大」。

    返回 `{"ok": bool, "size_bytes": int|None, "matched": str|None}`：
    - ok=False：请求失败（网络/鉴权/接口不存在），调用方应挂起
    - ok=True, size_bytes=None：库里没有同名版本
    """
    if not url or not title:
        return {"ok": False, "size_bytes": None, "matched": None}
    query = urllib.parse.urlencode({"title": title})
    separator = "&" if "?" in url else "?"
    request = urllib.request.Request(f"{url}{separator}{query}", method="GET")
    if api_key:
        request.add_header("X-Lux-Api-Key", api_key)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:  # noqa: S310
            body = json.loads(response.read(4096).decode("utf-8", "replace"))
    except urllib.error.HTTPError as error:
        try:
            detail = error.read(256).decode("utf-8", "replace")
        finally:
            error.close()
        log.warning("库中体积查询失败（HTTP %s: %s），按未知处理", error.code, detail)
        return {"ok": False, "size_bytes": None, "matched": None}
    except (urllib.error.URLError, TimeoutError, ValueError, OSError) as error:
        log.warning("库中体积查询失败（%s），按未知处理", error)
        return {"ok": False, "size_bytes": None, "matched": None}
    if not isinstance(body, dict):
        return {"ok": False, "size_bytes": None, "matched": None}
    size = body.get("size_bytes")
    try:
        size_bytes = int(size) if size is not None else None
    except (TypeError, ValueError):
        size_bytes = None
    matched = body.get("matched")
    return {
        "ok": True,
        "size_bytes": size_bytes,
        "matched": str(matched) if matched else None,
    }
