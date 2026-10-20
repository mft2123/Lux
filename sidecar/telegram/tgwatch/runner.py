"""Telethon 接线：首次运行登录、频道解析、监听、回填/补拉、投递、失败重试。

只有本文件 import telethon；其余模块保持纯逻辑，便于在没有 telethon 的环境跑单测。
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import sqlite3
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime

from . import config as config_mod
from . import filters
from . import links as links_mod
from . import sink as sink_mod
from . import store as store_mod

log = logging.getLogger("tgwatch.runner")

DELIVER_CONCURRENCY = 4

# 投递重试（review SC4）：Lux 的 400/403 重试也没用，直接落死信；
# 其余错误（网络、5xx、磁盘）最多尝试这么多次，第 MAX 次仍失败就落死信。
MAX_DELIVERY_ATTEMPTS = 20
# 重试间隔的指数退避上限（秒）
MAX_BACKOFF_SECONDS = 3600

# 容器里没有 tty 时，验证码文档写进 session/code.txt；最多等 15 分钟
CODE_WAIT_SECONDS = 900
CODE_POLL_SECONDS = 2
CODE_REMIND_SECONDS = 30


class RunnerError(RuntimeError):
    """启动前置条件不满足（未登录、session 损坏、频道解析不到、未装 telethon）。"""


# 判定结果 → 条目初始状态（逐链接判定，一条消息里可以有不同结果）
_ITEM_STATUS = {
    filters.STATUS_DELIVER: store_mod.STATUS_PENDING,
    filters.STATUS_HOLD: store_mod.STATUS_HELD,
    filters.STATUS_SKIP: store_mod.STATUS_SKIPPED,
}


def _message_outcome(decisions: list[filters.Decision]) -> tuple[str, str]:
    """消息级留痕（去重行，不决定投递）：有链接要投递就 pending，否则 held/skipped。

    逐链接判定后一条消息可能「一半投递一半挂起」，消息行只记最能代表这条消息的
    那个结果；每条链接的真实状态在 items 表中（review SC7）。
    """
    for decision in decisions:
        if decision.deliver:
            return store_mod.STATUS_PENDING, decision.reason
    for decision in decisions:
        if decision.status == filters.STATUS_HOLD:
            return store_mod.STATUS_HELD, decision.reason
    return store_mod.STATUS_SKIPPED, decisions[0].reason


def _short_url(url: str, limit: int = 80) -> str:
    return url if len(url) <= limit else url[: limit - 3] + "..."


def backoff_seconds(base: int, consecutive_failures: int) -> int:
    """连续失败时的重试间隔：base → 2*base → 4*base… 封顶 MAX_BACKOFF_SECONDS。

    `consecutive_failures=0`（上一轮没有失败）回到 base。
    """
    interval = max(1, int(base))
    if consecutive_failures <= 0:
        return interval
    return int(min(MAX_BACKOFF_SECONDS, interval * (2 ** min(consecutive_failures, 10))))


def _telethon():
    try:
        from telethon import TelegramClient, events, utils
    except ImportError as error:  # pragma: no cover - 容器里一定装好
        raise RunnerError("未安装 telethon：pip install -r requirements.txt") from error
    return TelegramClient, events, utils


@dataclass
class IncomingMessage:
    """一条待判定消息的纯数据视图（测试里直接造这个）。"""

    channel: str  # 配置里的频道标识，用于日志与 DB
    chat_id: int  # 只用于溯源；**不参与去重键**（review SC12）
    msg_id: int
    date: str
    text: str = ""
    entities: list[dict] = field(default_factory=list)

    @property
    def key(self) -> str:
        """去重键：`channel:msg_id`（review SC12）。

        老实现用 `chat_id:msg_id`：handler 路径有 `event.chat_id`（`-100…`），而
        backfill 路径的消息没有这个属性、退化成 `0:<id>`，同一条消息于是有两个键，
        可能被投递两次。频道标识（`@name` / `id:-100…`）在两条路径上都拿得到，
        是唯一稳定的部分。

        升级提示：键语义变了，旧状态库里的旧键不再命中。**最坏情况是重启后把回填
        窗口里的消息再投递一次**；Lux 侧 ingest 按 URL 幂等（返回 `duplicate`），
        不会产生第二条转存任务。不做数据迁移——旧键里的 chat_id 当时就没落库，
        无法可靠还原。
        """
        return f"{self.channel}:{self.msg_id}"


def entities_to_dicts(message) -> list[dict]:
    """telethon entity → 纯 dict（links 模块的输入契约）。

    已经是 dict 的原样保留，方便测试直接喂纯数据。
    """
    out: list[dict] = []
    for entity in getattr(message, "entities", None) or ():
        if isinstance(entity, dict):
            out.append(dict(entity))
            continue
        item = {
            "type": type(entity).__name__,
            "offset": int(getattr(entity, "offset", 0) or 0),
            "length": int(getattr(entity, "length", 0) or 0),
        }
        url = getattr(entity, "url", None)
        if url:
            item["url"] = str(url)
        out.append(item)
    return out


def incoming_from_message(message, channel_label: str, chat_id: int | None = None) -> IncomingMessage:
    date = getattr(message, "date", None)
    return IncomingMessage(
        channel=channel_label,
        chat_id=int(chat_id if chat_id is not None else getattr(message, "chat_id", 0) or 0),
        msg_id=int(getattr(message, "id", 0) or 0),
        date=date.isoformat() if isinstance(date, datetime) else str(date or ""),
        text=getattr(message, "text", None) or getattr(message, "message", None) or "",
        entities=entities_to_dicts(message),
    )


def peer_key(obj, peer_id: int | None = None) -> str:
    """稳定标识：有 username 用 `@name`，否则 `id:<peer_id>`（-100… 形式）。"""
    username = getattr(obj, "username", None)
    if username:
        return f"@{username}"
    if peer_id is None:
        peer_id = getattr(obj, "id", None)
    return f"id:{peer_id}"


# ---------------------------------------------------------------- 权限加固


def harden_permissions(config: config_mod.SidecarConfig) -> None:
    """session 里是账号授权密钥：目录 0700、文件 0600，进程 umask 077。

    必须显式 chmod：bind mount 会盖掉 Dockerfile 里的权限设置，而且 Telethon
    建 session 文件时用的是 0644。
    """
    os.umask(0o077)
    for directory in (config.session_dir, config.state_dir):
        try:
            directory.mkdir(parents=True, exist_ok=True)
            os.chmod(directory, 0o700)
        except OSError as error:
            log.warning("无法收紧目录权限 %s: %s", directory, error)
    if config.session_path.exists():
        try:
            os.chmod(config.session_path, 0o600)
        except OSError as error:
            log.warning("无法收紧 session 权限 %s: %s", config.session_path, error)


# ---------------------------------------------------------------- 登录


def code_provider(config: config_mod.SidecarConfig):
    """验证码回调：提示打到容器日志，人工在 session/ 目录下录入。

    两种录入方式，先到先用：
    - `session/code.txt`：容器没开 tty（`docker compose up -d`）时用，写入后本进程读走即删
    - tty 里的 stdin：`docker attach` / `docker run -it` 时直接敲
    """

    def provider() -> str:
        log.info(
            "需要登录：验证码已发到 %s。请把验证码写进 %s，或在本终端直接输入",
            config.phone or "Telegram 账号",
            config.code_file,
        )
        deadline = time.monotonic() + CODE_WAIT_SECONDS
        next_remind = time.monotonic() + CODE_REMIND_SECONDS
        while True:
            if config.code_file.exists():
                code = config.code_file.read_text(encoding="utf-8").strip()
                config.code_file.unlink(missing_ok=True)
                if code:
                    return code
            if sys.stdin is not None and sys.stdin.isatty():
                return input("请输入 Telegram 验证码: ").strip()
            if time.monotonic() >= deadline:
                raise RunnerError(
                    f"等待验证码超时（{CODE_WAIT_SECONDS}s）：重跑一次，把验证码写进 {config.code_file}"
                )
            if time.monotonic() >= next_remind:
                next_remind = time.monotonic() + CODE_REMIND_SECONDS
                log.info("仍在等待验证码：把收到的验证码写入 %s 即可继续", config.code_file)
            time.sleep(CODE_POLL_SECONDS)

    return provider


async def _authorize(client, config: config_mod.SidecarConfig) -> None:
    """首次运行登录（文档流程）：日志提示 → 人工录入 → session 落盘。

    首次登录必须有 phone；缺了就在这里抛 `RunnerError`（exit 2 + 一行中文），
    而不是让 telethon 抛一个带栈的 `ValueError`（review SC16）。
    """
    first_run = not config.session_path.exists()
    if first_run and not config.phone:
        raise RunnerError(
            "首次登录缺少 phone：在设置文件的 telegram.phone 里填，或设置 LUX_TG_PHONE"
            f"（session 已存在时不需要；当前 session 路径 {config.session_path}）"
        )
    if first_run:
        log.info("首次运行：%s 不存在，开始登录流程", config.session_path)
    await client.start(
        phone=config.phone or None,
        password=config.password or None,
        code_callback=code_provider(config),
    )
    harden_permissions(config)
    me = await client.get_me()
    log.info(
        "已登录：id=%s username=%s；session 已保存到 %s（0600）",
        getattr(me, "id", "?"),
        getattr(me, "username", "") or "-",
        config.session_path,
    )


# 只有这两类消息才说明 session 文件本身被写坏（review SC3）。
# sqlite 用 "file is not a database" / "database disk image is malformed" 报损坏。
_SESSION_CORRUPT_MARKERS = ("not a database", "malformed")

_SESSION_OPS_HINT = (
    "检查 session 文件的属主/属组与权限（目录 0700、文件 0600，容器内 uid 10001）、"
    "磁盘空间，以及是否有别的进程占用；修好后重启"
)


def _session_is_corrupt(error: BaseException) -> bool:
    message = str(error).lower()
    return any(marker in message for marker in _SESSION_CORRUPT_MARKERS)


def _sqlite_detail(error: BaseException) -> str:
    """sqlite 原文 + errno/code：无权限、磁盘满、被锁的文案各不相同，不能被抹平。"""
    detail = str(error) or error.__class__.__name__
    extras: list[str] = []
    errno_value = getattr(error, "errno", None)
    if errno_value is not None:
        extras.append(f"errno {errno_value}")
    code = getattr(error, "sqlite_errorcode", None)
    if code is not None:
        extras.append(f"{getattr(error, 'sqlite_errorname', None) or 'sqlite'} {code}")
    return f"{detail}（{'，'.join(extras)}）" if extras else detail


@contextlib.contextmanager
def session_errors(config: config_mod.SidecarConfig):
    """把 sqlite 层面的 session 故障翻译成干净的中文错误（review F6 / SC3）。

    Telethon 的 session 就是 sqlite 文件；出错时默认会连栈打到日志里，再被
    `restart: unless-stopped` 反复刷屏。分类比「一律说损坏」重要：

    - `DatabaseError` 且消息含 `not a database` / `malformed`：文件真的坏了，
      删掉重登即可；
    - `OperationalError`（无权限、磁盘满、被锁）：文件没坏，原样带 errno 报出，
      让人去修权限/空间，**不能**叫人删 session（删了要重新登录，问题还在）；
    - 其余 sqlite 错误也只报一次，不猜成损坏。
    """
    try:
        yield
    except sqlite3.OperationalError as error:
        raise RunnerError(
            f"session 无法打开或写入（{config.session_path}）：{_sqlite_detail(error)}；"
            f"{_SESSION_OPS_HINT}"
        ) from error
    except sqlite3.DatabaseError as error:
        detail = _sqlite_detail(error)
        if _session_is_corrupt(error):
            raise RunnerError(
                f"session 文件损坏（{config.session_path}）：{detail}。"
                "删掉它后重启，会重新走登录流程"
            ) from error
        raise RunnerError(
            f"session 读取失败（{config.session_path}）：{detail}；"
            "文件本身没被写坏，按提示检查权限/磁盘后再试"
        ) from error
    except sqlite3.Error as error:
        raise RunnerError(
            f"session 访问失败（{config.session_path}）：{type(error).__name__}: "
            f"{_sqlite_detail(error)}；{_SESSION_OPS_HINT}"
        ) from error


# ---------------------------------------------------------------- 登录态自检（check --deep）


# `check --deep` 连一次 Telegram 的等待上限（秒）。Dockerfile 的 HEALTHCHECK
# `--timeout` 必须比它大，否则 docker 会先杀掉进程、报不出「超时」的原因。
DEEP_CHECK_TIMEOUT = 10.0
#: 频道清单要让用户等得下去，但也不能无限等。
CHANNEL_LIST_TIMEOUT = 60.0


async def list_channels(
    config: config_mod.SidecarConfig,
    *,
    client_factory=None,
    timeout: float = CHANNEL_LIST_TIMEOUT,
) -> list[dict]:
    """列出账号里的全部频道（设置页「刷新频道列表」用）。

    只取 `is_channel` 的会话（不含普通群），返回稳定键与展示字段。
    `getDialogs` 在几千个会话的账号上会慢到分钟级，所以调用方要缓存。
    """
    if not config.session_path.exists():
        raise RunnerError(f"session 不存在（{config.session_path}）：先完成首次登录")
    factory = client_factory or _new_client
    client = factory(config)
    try:
        with session_errors(config):
            await asyncio.wait_for(client.connect(), timeout)
            dialogs = await asyncio.wait_for(client.get_dialogs(), timeout)
    except asyncio.TimeoutError as error:
        raise RunnerError(f"读取频道超时（{timeout:g}s）：检查容器网络/代理") from error
    finally:
        with contextlib.suppress(Exception):
            await client.disconnect()
    channels = []
    for dialog in dialogs:
        entity = getattr(dialog, "entity", None)
        if not getattr(dialog, "is_channel", False) or entity is None:
            continue
        username = (getattr(entity, "username", "") or "").lstrip("@")
        raw_id = getattr(entity, "id", None) or getattr(dialog, "id", None)
        channels.append(
            {
                "telegramId": str(raw_id or ""),
                "username": username,
                "title": getattr(dialog, "title", "") or username or "未命名频道",
            }
        )
    return channels


def _new_client(config: config_mod.SidecarConfig):
    """按配置造一个 TelegramClient；测试里替换掉它来注入假 client。"""
    TelegramClient, _events, _utils = _telethon()
    return TelegramClient(str(config.session_path), config.api_id, config.api_hash)


async def _probe_authorized(factory, config: config_mod.SidecarConfig) -> bool:
    client = factory(config)
    try:
        await client.connect()
        return bool(await client.is_user_authorized())
    finally:
        # 连不上/超时也要断开，别在 healthcheck 里泄漏连接
        with contextlib.suppress(Exception):
            await client.disconnect()


async def check_session(
    config: config_mod.SidecarConfig,
    *,
    client_factory=None,
    timeout: float = DEEP_CHECK_TIMEOUT,
) -> tuple[bool, str]:
    """`check --deep`：真的连一次 Telegram，确认 session 还有登录态（review SC9）。

    `check` 只证明「文件在、字段不空」；session 过期但文件还在时它照样说健康。
    这里用 `is_user_authorized()` 问一次服务端（带 10s 超时）：无 session、未授权、
    连不上都返回 `(False, 原因)`。**不抛异常**——healthcheck 只看退出码。
    """
    if not config.session_path.exists():
        return False, f"session 不存在（{config.session_path}）：先跑 `main.py run` 完成首次登录"
    factory = client_factory or _new_client
    try:
        with session_errors(config):
            authorized = await asyncio.wait_for(_probe_authorized(factory, config), timeout)
    except RunnerError as error:
        return False, str(error)
    except asyncio.TimeoutError:
        return False, f"连接 Telegram 超时（{timeout:g}s）：检查容器网络/代理后重试"
    except Exception as error:  # noqa: BLE001 - healthcheck 不能因为异常类型而崩
        return False, f"连接 Telegram 失败：{type(error).__name__}: {error}"
    if not authorized:
        return False, "session 未登录或已失效（is_user_authorized=False）：删掉 session 后重新登录"
    return True, "session 已登录（is_user_authorized=True）"


# ---------------------------------------------------------------- 洗版查询


class LibraryLookup:
    """问 Lux「库中同名版本多大」：`GET /api/v1/admin/archive/lookup?title=`。

    返回 `{"ok", "size_bytes", "matched"}`；带 `X-Lux-Api-Key`。
    查询失败（接口不存在/鉴权失败/超时）时 `ok=False`，调用方挂起而不是猜。
    """

    def __init__(self, url: str = "", api_key: str = "", timeout: float = 15.0):
        self.url = url
        self.api_key = api_key
        self.timeout = timeout
        self._cache: dict[str, dict] = {}

    async def lookup(self, title: str) -> dict:
        if not title:
            return {"ok": False, "size_bytes": None, "matched": None}
        if title in self._cache:
            return self._cache[title]
        result = await asyncio.to_thread(sink_mod.lookup_size, self.url, self.api_key, title, self.timeout)
        self._cache[title] = result
        return result


# ---------------------------------------------------------------- 监听


class Watcher:
    def __init__(
        self,
        config: config_mod.SidecarConfig,
        store: store_mod.Store,
        sink,
        lookup: LibraryLookup | None = None,
    ):
        self.config = config
        self.store = store
        self.sink = sink
        self.lookup = lookup or LibraryLookup(config.lookup_url, config.api_key, config.http_timeout)
        self.counts = {"messages": 0, "delivered": 0, "held": 0, "skipped": 0, "failed": 0, "dead": 0}
        self._semaphore = asyncio.Semaphore(DELIVER_CONCURRENCY)
        self._stop = asyncio.Event()
        # peer_key → ChannelConfig；启动解析频道时填充
        self.by_key: dict[str, config_mod.ChannelConfig] = {}

    # ---- 消息处理（核心，可单测） ----

    async def process(self, incoming: IncomingMessage, channel: config_mod.ChannelConfig) -> None:
        self.counts["messages"] += 1
        if incoming.msg_id <= 0:
            return
        if self.store.seen(incoming.key):
            log.debug("重复消息，跳过: %s", incoming.key)
            return

        found = links_mod.extract_links(incoming.text, incoming.entities)
        accepted = links_mod.split_kinds(found, self.config.allowed_kinds)
        mode = channel.mode
        size_bytes = links_mod.parse_size(incoming.text)

        if not accepted:
            self.store.record_message(
                msg_key=incoming.key,
                channel=incoming.channel,
                msg_id=incoming.msg_id,
                msg_date=incoming.date,
                mode=mode,
                link_count=len(found),
                status=store_mod.STATUS_SKIPPED,
                reason="no_allowed_link" if found else "no_link",
            )
            self.store.advance_message_id(incoming.channel, incoming.msg_id)
            self.counts["skipped"] += 1
            log.debug("%s 无可用链接（提取到 %d 条），跳过", incoming.key, len(found))
            return

        # review SC7：逐链接判定。每个链接只用自己的锚文本当标题；一条消息里有多个
        # 链接时，缺标题的那个不许拿整段正文兜底（那等于把别人的标题套给它），
        # 记 held/title_ambiguous，其余链接照常投递。
        allow_text_fallback = len(accepted) == 1
        outcomes: list[tuple[links_mod.Link, str, filters.Decision]] = []
        for link in accepted:
            title = links_mod.link_title(link, incoming.text, fallback_to_text=allow_text_fallback)
            outcomes.append((link, title, await self._decide_link(channel, mode, title, size_bytes)))

        msg_status, msg_reason = _message_outcome([decision for _, _, decision in outcomes])
        self.store.record_message(
            msg_key=incoming.key,
            channel=incoming.channel,
            msg_id=incoming.msg_id,
            msg_date=incoming.date,
            mode=mode,
            link_count=len(accepted),
            status=msg_status,
            reason=msg_reason,
        )
        self.store.advance_message_id(incoming.channel, incoming.msg_id)

        deliver_ids: list[int] = []
        held: dict[str, int] = {}
        skipped: dict[str, int] = {}
        for link, title, decision in outcomes:
            item_status = _ITEM_STATUS[decision.status]
            item_id = self.store.add_item(
                msg_key=incoming.key,
                channel=incoming.channel,
                msg_id=incoming.msg_id,
                msg_date=incoming.date,
                url=link.url,
                kind=link.kind,
                anchor=link.anchor,
                title=title,
                size_bytes=size_bytes,
                mode=mode,
                status=item_status,
                reason=decision.reason,
            )
            if decision.deliver:
                if item_id:
                    deliver_ids.append(item_id)
                continue
            if decision.reason == filters.HOLD_TITLE_AMBIGUOUS.reason:
                # 日志点名具体链接：不然运维只看到「有一条被挂起」，不知道该补哪条的标题
                log.info(
                    "%s %s 链接 %s 没有独立标题（title_ambiguous）：挂起不猜，其余链接照常处理",
                    incoming.channel,
                    incoming.msg_id,
                    _short_url(link.url),
                )
            bucket = held if decision.status == filters.STATUS_HOLD else skipped
            bucket[decision.reason] = bucket.get(decision.reason, 0) + 1

        self.counts["held"] += sum(held.values())
        self.counts["skipped"] += sum(skipped.values())
        for reason, count in held.items():
            log.info("%s %s 挂起 %d 条链接：%s", incoming.channel, incoming.msg_id, count, reason)
        for reason, count in skipped.items():
            log.info("%s %s 不投递 %d 条链接：%s", incoming.channel, incoming.msg_id, count, reason)
        if not deliver_ids:
            return

        log.info(
            "%s %s 投递 %d 条链接（%s / %s）",
            incoming.channel,
            incoming.msg_id,
            len(deliver_ids),
            mode,
            msg_reason,
        )
        await self.deliver(deliver_ids)

    async def _decide_link(
        self,
        channel: config_mod.ChannelConfig,
        mode: str,
        title: str,
        size_bytes: int | None,
    ) -> filters.Decision:
        """单个链接的判定；洗版按**这个链接自己的标题**查库（同标题走缓存）。"""
        if not title:
            return filters.HOLD_TITLE_AMBIGUOUS
        library: dict = {"ok": True, "size_bytes": None, "matched": None}
        if mode == filters.MODE_WASH and size_bytes is not None:
            library = await self.lookup.lookup(title)
        return filters.decide(
            mode,
            title=title,
            follow_list=self.config.follow_for(channel.key()),
            size_bytes=size_bytes,
            library_size=library.get("size_bytes"),
            library_known=bool(library.get("ok", True)),
            wash_tolerance=self.config.wash_tolerance,
            link_count=1,
        )

    async def deliver(self, item_ids: list[int]) -> int:
        """逐条投递；返回本轮失败（含落死信）的条数。"""
        failures = 0
        for item_id in item_ids:
            item = self.store.get_item(item_id)
            if not item or item["status"] == store_mod.STATUS_DELIVERED:
                continue
            async with self._semaphore:
                try:
                    await self.sink.send(item)
                except sink_mod.SinkError as error:
                    failures += 1
                    self._record_failure(item, error)
                except Exception as error:  # noqa: BLE001 - 序列化等意外也必须落状态（SC18）
                    failures += 1
                    log.exception("投递异常（非 SinkError）item=%s", item_id)
                    self._record_failure(
                        item, sink_mod.SinkError(f"未预期异常: {type(error).__name__}: {error}")
                    )
                else:
                    self.counts["delivered"] += 1
                    self.store.set_status(item_id, store_mod.STATUS_DELIVERED)
        return failures

    def _record_failure(self, item: dict, error: sink_mod.SinkError) -> None:
        """永久失败（400/403）或试满 MAX_DELIVERY_ATTEMPTS → 死信，其余进 failed 重试。"""
        item_id = int(item["id"])
        attempts = int(item.get("attempts") or 0) + 1  # 含本次
        reason = str(error)[:200]
        if getattr(error, "permanent", False):
            self.store.set_status(item_id, store_mod.STATUS_DEAD, reason, bump_attempts=True)
            self.counts["dead"] += 1
            log.warning(
                "投递被 Lux 永久拒绝，落死信 item=%s attempts=%s（不再重试）: %s",
                item_id,
                attempts,
                error,
            )
            return
        if attempts >= MAX_DELIVERY_ATTEMPTS:
            dead_reason = f"max_attempts({attempts}): {reason}"[:200]
            self.store.set_status(item_id, store_mod.STATUS_DEAD, dead_reason, bump_attempts=True)
            self.counts["dead"] += 1
            log.error(
                "投递失败已达上限 %s 次，落死信 item=%s: %s（`main.py replay --id %s` 可人工再试）",
                MAX_DELIVERY_ATTEMPTS,
                item_id,
                error,
                item_id,
            )
            return
        self.counts["failed"] += 1
        self.store.set_status(item_id, store_mod.STATUS_FAILED, reason, bump_attempts=True)
        log.warning(
            "投递失败 item=%s attempts=%s/%s: %s",
            item_id,
            attempts,
            MAX_DELIVERY_ATTEMPTS,
            error,
        )

    # ---- 重试循环（failed + 手动放行后的 pending） ----

    async def retry_once(self, limit: int = 50) -> tuple[int, int]:
        """推一轮可重试条目；返回 (尝试条数, 失败条数)。"""
        items = self.store.items_by_status((store_mod.STATUS_FAILED, store_mod.STATUS_PENDING), limit=limit)
        if not items:
            return 0, 0
        if self.config.dry_run:
            log.info("dry-run：跳过 %d 条待投递条目", len(items))
            return 0, 0
        log.info("重试投递 %d 条", len(items))
        failures = await self.deliver([int(item["id"]) for item in items])
        return len(items), failures

    def retry_interval(self) -> int:
        return max(10, int(self.config.retry_seconds))

    async def _wait(self, seconds: float) -> bool:
        """等 `seconds` 秒；期间收到 stop 返回 True。单独抽出来便于测试退避。"""
        try:
            await asyncio.wait_for(self._stop.wait(), timeout=seconds)
            return True
        except asyncio.TimeoutError:
            return False

    async def retry_loop(self) -> None:
        base = self.retry_interval()
        failures = 0
        delay = base
        while not self._stop.is_set():
            if await self._wait(delay):
                return
            try:
                _, failed = await self.retry_once()
            except Exception:  # pragma: no cover - 重试循环不能死
                log.exception("重试循环异常")
                failed = 1
            # 一轮里还有失败就指数退避，全部成功则回到基准间隔
            failures = failures + 1 if failed else 0
            delay = backoff_seconds(base, failures)

    def stop(self) -> None:
        self._stop.set()

    # ---- telethon 事件入口 ----

    def on_event(self, event) -> None:
        """由 telethon 调用；解析成纯数据后交给 process。"""
        channel = self._channel_for(event)
        if channel is None:
            log.debug("未登记的会话，忽略: %s", getattr(event, "chat_id", "?"))
            return
        message = getattr(event, "message", None)
        if message is None:
            return
        incoming = incoming_from_message(message, channel.key(), getattr(event, "chat_id", None))
        task = asyncio.create_task(self._safe_process(incoming, channel))
        _BACKGROUND.add(task)
        task.add_done_callback(_BACKGROUND.discard)

    async def _safe_process(self, incoming: IncomingMessage, channel: config_mod.ChannelConfig) -> None:
        try:
            await self.process(incoming, channel)
        except Exception:
            log.exception("处理消息失败: %s", incoming.key)
            self.store.record_message(
                msg_key=incoming.key,
                channel=incoming.channel,
                msg_id=incoming.msg_id,
                msg_date=incoming.date,
                mode=channel.mode,
                link_count=0,
                status=store_mod.STATUS_FAILED,
                reason="exception",
            )

    def _channel_for(self, event) -> config_mod.ChannelConfig | None:
        chat = getattr(event, "chat", None)
        key = peer_key(chat, getattr(event, "chat_id", None))
        if key in self.by_key:
            return self.by_key[key]
        label = str(getattr(chat, "username", "") or "")
        for channel in self.config.channels:
            if label and channel.key().lstrip("@") == label:
                return channel
        return None


_BACKGROUND: set[asyncio.Task] = set()


async def _drain_background(reason: str = "退出") -> None:
    """取消并等待后台 handler 任务收尾（review SC11）。

    `store.close()` 之后若还有 handler 在写库，会抛 `sqlite3.ProgrammingError`
    （或静默丢记录）。所以断开连接前先把这些任务 cancel 掉并 gather 完。
    """
    tasks = [task for task in list(_BACKGROUND) if not task.done()]
    if not tasks:
        return
    log.info("%s：等待 %d 个后台任务收尾", reason, len(tasks))
    for task in tasks:
        task.cancel()
    await asyncio.gather(*tasks, return_exceptions=True)


async def _resolve_channel(client, channel: config_mod.ChannelConfig):
    """按配置解析出 telethon entity；数字 id 与 @username 都试一遍。"""
    candidate = channel.key()
    attempts: list = []
    if candidate.lstrip("-").isdigit():
        attempts.append(int(candidate))
    attempts.append(candidate)
    last_error: Exception | None = None
    for value in attempts:
        try:
            return await client.get_entity(value)
        except Exception as error:  # noqa: BLE001 - 逐个候选试，最后统一报错
            last_error = error
    raise RunnerError(f"频道 {candidate} 解析失败: {last_error}")


async def _backfill_newest(client, watcher: "Watcher", channel, entity, limit: int) -> int:
    """全量模式：取**最新** N 条，再按时间顺序处理。

    `iter_messages(limit=N)` 默认从新到旧；加 `reverse=True` 会从频道第一条
    开始往回翻，在成熟频道上等于永远读最老的 200 条（review F4）。
    """
    if limit <= 0:
        return 0
    messages = [message async for message in client.iter_messages(entity, limit=limit)]
    log.info("回填 %s 最新 %d 条消息（全量模式）", channel.key(), len(messages))
    for message in reversed(messages):
        await watcher.process(incoming_from_message(message, channel.key()), channel)
    return len(messages)


async def _gap_backfill(client, watcher: "Watcher", channel, entity, limit: int) -> int:
    """按游标补拉宕机期间的消息（`min_id` = 已处理的最大消息 id）。"""
    last_id = watcher.store.last_message_id(channel.key())
    if last_id <= 0 or limit <= 0:
        return 0
    messages = [
        message async for message in client.iter_messages(entity, min_id=last_id, limit=limit)
    ]
    if not messages:
        return 0
    log.info("补拉 %s 宕机期间 %d 条消息（> %s）", channel.key(), len(messages), last_id)
    for message in reversed(messages):
        await watcher.process(incoming_from_message(message, channel.key()), channel)
    return len(messages)


async def run(config: config_mod.SidecarConfig) -> None:
    """常驻：加固权限 → 登录（首次）→ 解析频道 → 回填 → 订阅 → 补拉 → 重试循环。"""
    if not config.channels:
        # 空配置不再拒绝启动：设置页要先能列出账号里的频道，才有了勾选的前提。
        # 这里空转（/channels 仍可用），勾选并重启后开始投递。
        log.warning("listen_channels 为空：侧车空转，等勾选频道后重启")
        try:
            await asyncio.Event().wait()
        finally:
            return
    TelegramClient, events, utils = _telethon()

    harden_permissions(config)

    store = store_mod.Store(config.state_db)
    sink = sink_mod.build(config)
    log.info(
        "旁挂启动：sink=%s 模式=%s 频道=%d 设置=%s",
        sink.describe().get("sink"),
        config.default_mode,
        len(config.channels),
        config.settings_path,
    )
    if config.dry_run:
        log.warning("dry-run 已开启：只判定与记录，不写下游")

    watcher = Watcher(config, store, sink, LibraryLookup(config.lookup_url, config.api_key, config.http_timeout))
    client = None
    try:
        try:
            with session_errors(config):
                client = TelegramClient(str(config.session_path), config.api_id, config.api_hash)
                await _authorize(client, config)
        except RunnerError:
            raise
        except sqlite3.Error as error:  # pragma: no cover - session_errors 已兜住
            raise RunnerError(
                f"session 访问失败（{config.session_path}）：{_sqlite_detail(error)}"
            ) from error

        resolved: list[tuple[config_mod.ChannelConfig, object]] = []
        for channel in config.channels:
            try:
                entity = await _resolve_channel(client, channel)
            except RunnerError as error:
                log.error("%s", error)
                continue
            key = peer_key(entity, utils.get_peer_id(entity))
            watcher.by_key[key] = channel
            watcher.by_key[channel.key()] = channel
            resolved.append((channel, entity))
            log.info("监听 %s → %s（%s）", channel.key(), key, channel.mode)

        if not resolved:
            raise RunnerError("所有频道都解析失败：检查 listen_channels 是否正确、账号是否在频道内")

        for channel, entity in resolved:
            if channel.mode != filters.MODE_FULL:
                continue
            try:
                await _backfill_newest(client, watcher, channel, entity, config.backfill_limit)
            except Exception:  # noqa: BLE001 - 回填失败不该拖死常驻
                log.exception("回填 %s 失败", channel.key())

        client.add_event_handler(
            watcher.on_event,
            events.NewMessage(chats=[entity for _, entity in resolved], incoming=True),
        )

        # 宕机窗口：先让 telethon 收差量更新（走 handler），再按游标兜底补拉
        try:
            await client.catch_up()
        except Exception:  # noqa: BLE001
            log.warning("catch_up 失败，改由游标补拉兜底", exc_info=True)
        for channel, entity in resolved:
            try:
                await _gap_backfill(client, watcher, channel, entity, config.gap_limit)
            except Exception:  # noqa: BLE001
                log.exception("补拉 %s 失败", channel.key())

        retry_task = asyncio.create_task(watcher.retry_loop())
        log.info("就绪：%d 个频道在线", len(resolved))
        try:
            await client.run_until_disconnected()
        finally:
            watcher.stop()
            retry_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await retry_task
            # 后台 handler 必须先收尾，再让外层 finally 关库（review SC11）
            await _drain_background("停止监听")
            log.info("统计: %s", watcher.counts)
    finally:
        if client is not None:
            await client.disconnect()
        store.close()


async def deliver_pending(
    config: config_mod.SidecarConfig, item_ids: list[int] | None, statuses, limit: int
) -> int:
    """`replay` 子命令：把 held/failed/pending 推到下游（"管理员触发时才处理"）。"""
    harden_permissions(config)
    store = store_mod.Store(config.state_db)
    sink = sink_mod.build(config)
    watcher = Watcher(config, store, sink, LibraryLookup(config.lookup_url, config.api_key, config.http_timeout))
    try:
        if item_ids:
            store.promote(item_ids)
            ids = item_ids
        else:
            ids = [int(item["id"]) for item in store.items_by_status(tuple(statuses), limit=limit)]
            if ids:
                store.promote(ids)
        if not ids:
            log.info("没有可投递的条目")
            return 0
        log.info("投递 %d 条", len(ids))
        await watcher.deliver(ids)
        return len(ids)
    finally:
        store.close()
