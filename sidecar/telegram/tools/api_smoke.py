"""升级 telethon 后跑一次：核对本旁挂用到的 API 是否还在、签名是否没变。

不需要登录、不联网：

    python tools/api_smoke.py
"""

from __future__ import annotations

import asyncio
import inspect
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from telethon import TelegramClient, events, utils  # noqa: E402

from tgwatch import links, runner  # noqa: E402

results: list[tuple[str, bool]] = []


def check(name: str, condition) -> None:
    results.append((name, bool(condition)))


def main_checks() -> None:
    iter_params = inspect.signature(TelegramClient.iter_messages).parameters
    check("iter_messages(limit)", "limit" in iter_params)
    check("iter_messages(reverse)", "reverse" in iter_params)
    # review SC13：宕机窗口靠 `iter_messages(min_id=…)` 补拉（runner._gap_backfill）
    check("iter_messages(min_id)", "min_id" in iter_params)
    check("NewMessage(chats)", "chats" in inspect.signature(events.NewMessage.__init__).parameters)
    check("NewMessage(incoming)", "incoming" in inspect.signature(events.NewMessage.__init__).parameters)
    start_params = inspect.signature(TelegramClient.start).parameters
    for param in ("phone", "password", "code_callback"):
        check(f"start({param})", param in start_params)
    for attr in ("get_entity", "run_until_disconnected", "add_event_handler", "is_user_authorized", "catch_up"):
        check(attr, hasattr(TelegramClient, attr))
    check("utils.get_peer_id", hasattr(utils, "get_peer_id"))
    check("is_user_authorized coroutine", inspect.iscoroutinefunction(TelegramClient.is_user_authorized))
    # review SC13：run() 启动时先 catch_up 收差量更新，再由游标补拉兜底
    check("catch_up coroutine", inspect.iscoroutinefunction(TelegramClient.catch_up))

    session_dir = tempfile.mkdtemp()
    client = TelegramClient(os.path.join(session_dir, "lux-telegram"), 123456, "aa" * 16)
    check("client constructed offline", client is not None)
    check("session file created", os.path.exists(os.path.join(session_dir, "lux-telegram.session")))


async def async_checks() -> None:
    from telethon.tl import types

    entity = types.MessageEntityTextUrl(offset=0, length=4, url="https://115.com/s/1")

    class FakeMessage:
        entities = [entity]
        text = "标题文字"
        date = None
        id = 1
        chat_id = -1001

    incoming = runner.incoming_from_message(FakeMessage(), "@chan")
    check("entity dict type", incoming.entities[0].get("type") == "MessageEntityTextUrl")
    check("entity dict url", incoming.entities[0].get("url", "").endswith("/s/1"))
    found = links.extract_links(incoming.text, incoming.entities)
    check("hidden link extracted", len(found) == 1)
    check("anchor sliced by utf-16 offset", bool(found) and found[0].anchor == "标题文字")
    check("kind classified", bool(found) and found[0].kind == links.KIND_115)

    channel = types.Channel(id=1, title="t", photo=None, date=None, username="ch")
    check("peer_key with username", runner.peer_key(channel, -1001) == "@ch")


if __name__ == "__main__":
    main_checks()
    asyncio.run(async_checks())
    failed = [name for name, ok in results if not ok]
    for name, ok in results:
        print(("OK   " if ok else "FAIL ") + name)
    print(f"\n{len(results) - len(failed)}/{len(results)} checks passed")
    raise SystemExit(1 if failed else 0)
