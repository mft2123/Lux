import asyncio
import io
import json
import os
import sqlite3
import stat
import unittest
from pathlib import Path
from unittest.mock import patch

from tests import support
from tgwatch import config as config_mod
from tgwatch import filters, runner, sink as sink_mod, store as store_mod


class FakeLookup:
    """假 Lux 库查询：可以返回体积、没有同名、或查询失败。"""

    def __init__(self, size_bytes=None, ok=True):
        self.size = size_bytes
        self.ok = ok
        self.calls = []

    async def lookup(self, title):
        self.calls.append(title)
        return {"ok": self.ok, "size_bytes": self.size, "matched": None}


class FakeSink:
    name = "fake"

    def __init__(self):
        self.sent = []
        self.fail = False

    async def send(self, item):
        if self.fail:
            raise sink_mod.SinkError("boom")
        self.sent.append(item)

    def describe(self):
        return {"sink": self.name}


def make_config(tmp: Path, **overrides) -> config_mod.SidecarConfig:
    base = dict(
        api_id=1,
        api_hash="hash",
        api_key="test-key",
        session_dir=tmp / "session",
        state_dir=tmp / "state",
        inbox_dir=tmp / "inbox",
        default_mode=filters.MODE_NEW,
        channels=(config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_NEW),),
    )
    base.update(overrides)
    return config_mod.SidecarConfig(**base)


def message(text="某剧 第二季 4K", entities=None, msg_id=42, chat_id=-100123, channel="@chan"):
    return runner.IncomingMessage(
        channel=channel,
        chat_id=chat_id,
        msg_id=msg_id,
        date="2026-10-18T00:00:00+00:00",
        text=text,
        entities=entities or [],
    )


HIDDEN = [{"type": "MessageEntityTextUrl", "offset": 0, "length": 9, "url": "https://115.com/s/abc"}]


class FakeMessage:
    def __init__(self, msg_id, text="", entities=None, chat_id=-100123):
        self.id = msg_id
        self.text = text
        self.entities = entities or []
        self.chat_id = chat_id
        self.date = None


class FakeClient:
    """按 Telethon 的语义返回消息：默认从新到旧，min_id 只取更新的。"""

    def __init__(self, messages):
        self.messages = messages
        self.calls = []

    async def iter_messages(self, entity, **kwargs):
        self.calls.append(dict(kwargs))
        selected = list(self.messages)
        if kwargs.get("min_id"):
            selected = [item for item in selected if item.id > int(kwargs["min_id"])]
        limit = kwargs.get("limit")
        if limit is not None:
            selected = selected[: int(limit)]
        for item in selected:
            yield item


class ProcessTest(unittest.TestCase):
    def setUp(self):
        self.tmp = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.tmp)

    def build(self, config, lookup=None, sink=None):
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        watcher = runner.Watcher(config, store, sink or FakeSink(), lookup or FakeLookup())
        return watcher, store

    def test_new_mode_delivers(self):
        config = make_config(self.tmp)
        sink = FakeSink()
        watcher, store = self.build(config, sink=sink)
        asyncio.run(watcher.process(message(entities=HIDDEN), config.channels[0]))
        self.assertEqual(len(sink.sent), 1)
        self.assertEqual(sink.sent[0]["url"], "https://115.com/s/abc")
        self.assertEqual(store.stats()["by_status"].get(store_mod.STATUS_DELIVERED), 1)

    def test_manual_mode_holds(self):
        config = make_config(
            self.tmp,
            default_mode=filters.MODE_MANUAL,
            channels=(config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_MANUAL),),
        )
        sink = FakeSink()
        watcher, store = self.build(config, sink=sink)
        asyncio.run(watcher.process(message(entities=HIDDEN), config.channels[0]))
        self.assertEqual(sink.sent, [])
        self.assertEqual(store.items_by_status((store_mod.STATUS_HELD,))[0]["reason"], "manual_mode")

    def test_duplicate_message_is_ignored(self):
        config = make_config(self.tmp)
        sink = FakeSink()
        watcher, store = self.build(config, sink=sink)

        async def scenario():
            await watcher.process(message(entities=HIDDEN), config.channels[0])
            await watcher.process(message(entities=HIDDEN), config.channels[0])

        asyncio.run(scenario())
        self.assertEqual(len(sink.sent), 1)
        self.assertEqual(store.stats()["messages"], 1)

    def test_follow_miss_is_skipped(self):
        config = make_config(
            self.tmp,
            channels=(
                config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_FOLLOW, follow_list=("别的剧",)),
            ),
        )
        watcher, store = self.build(config)
        asyncio.run(watcher.process(message(entities=HIDDEN), config.channels[0]))
        self.assertEqual(store.items_by_status((store_mod.STATUS_SKIPPED,))[0]["reason"], "follow_miss")

    def test_no_link_message_does_not_create_items(self):
        config = make_config(self.tmp)
        watcher, store = self.build(config)
        asyncio.run(watcher.process(message(text="今天没有资源"), config.channels[0]))
        self.assertEqual(store.stats()["items"], 0)

    def test_telegram_links_filtered_out(self):
        config = make_config(self.tmp)
        sink = FakeSink()
        watcher, _ = self.build(config, sink=sink)
        asyncio.run(watcher.process(message(text="见 https://t.me/other/1"), config.channels[0]))
        self.assertEqual(sink.sent, [])

    def test_cursor_advances_with_every_recorded_message(self):
        config = make_config(self.tmp)
        watcher, store = self.build(config)
        asyncio.run(watcher.process(message(entities=HIDDEN, msg_id=77), config.channels[0]))
        self.assertEqual(store.last_message_id("@chan"), 77)
        # 不后退
        store.advance_message_id("@chan", 50)
        self.assertEqual(store.last_message_id("@chan"), 77)

    def wash_config(self):
        return make_config(
            self.tmp,
            channels=(config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_WASH),),
        )

    def test_wash_bigger_is_delivered(self):
        config = self.wash_config()
        lookup = FakeLookup(size_bytes=5 * 1024**3)
        sink = FakeSink()
        watcher, _ = self.build(config, lookup=lookup, sink=sink)
        asyncio.run(watcher.process(message("某剧 大小 12G", HIDDEN), config.channels[0]))
        self.assertEqual(len(sink.sent), 1)
        self.assertEqual(lookup.calls, ["某剧 大小 12G"])

    def test_wash_unknown_size_holds_without_querying(self):
        config = self.wash_config()
        lookup = FakeLookup(size_bytes=5)
        sink = FakeSink()
        watcher, store = self.build(config, lookup=lookup, sink=sink)
        asyncio.run(watcher.process(message("某剧 4K HDR", HIDDEN), config.channels[0]))
        self.assertEqual(sink.sent, [])
        self.assertEqual(store.items_by_status((store_mod.STATUS_HELD,))[0]["reason"], "size_unknown")
        self.assertEqual(lookup.calls, [])  # 体积不知道就别去问库

    def test_wash_lookup_failure_holds(self):
        config = self.wash_config()
        lookup = FakeLookup(ok=False)
        watcher, store = self.build(config, lookup=lookup)
        asyncio.run(watcher.process(message("某剧 大小 12G", HIDDEN), config.channels[0]))
        self.assertEqual(
            store.items_by_status((store_mod.STATUS_HELD,))[0]["reason"], "library_size_unknown"
        )

    def test_wash_no_library_copy_is_delivered(self):
        config = self.wash_config()
        lookup = FakeLookup(size_bytes=None, ok=True)
        sink = FakeSink()
        watcher, _ = self.build(config, lookup=lookup, sink=sink)
        asyncio.run(watcher.process(message("某剧 大小 12G", HIDDEN), config.channels[0]))
        self.assertEqual(len(sink.sent), 1)

    def test_sink_failure_marks_failed_and_retry_succeeds(self):
        config = make_config(self.tmp)
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        flaky = FakeSink()
        flaky.fail = True
        watcher = runner.Watcher(config, store, flaky, FakeLookup())

        asyncio.run(watcher.process(message(entities=HIDDEN), config.channels[0]))
        self.assertEqual(store.items_by_status((store_mod.STATUS_FAILED,))[0]["attempts"], 1)

        flaky.fail = False
        asyncio.run(watcher.retry_once())
        self.assertEqual(store.stats()["by_status"].get(store_mod.STATUS_DELIVERED), 1)


class BackfillTest(unittest.TestCase):
    """review F4：全量回填必须是「最新 N 条」，按时间顺序处理。"""

    def setUp(self):
        self.tmp = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.tmp)

    def test_backfill_takes_newest_and_processes_chronologically(self):
        config = make_config(
            self.tmp,
            channels=(config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_FULL),),
        )
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        sink = FakeSink()
        watcher = runner.Watcher(config, store, sink, FakeLookup())
        channel = config.channels[0]
        # Telethon 默认从新到旧返回
        client = FakeClient([FakeMessage(3, "c", HIDDEN), FakeMessage(2, "b", HIDDEN), FakeMessage(1, "a", HIDDEN)])

        count = asyncio.run(runner._backfill_newest(client, watcher, channel, object(), 3))
        self.assertEqual(count, 3)
        self.assertEqual(client.calls[0], {"limit": 3})
        self.assertNotIn("reverse", client.calls[0])
        # 处理顺序按时间（1,2,3）——入库顺序即 msg_id 升序
        ids = [item["msg_id"] for item in store.items_by_status((store_mod.STATUS_DELIVERED,), limit=10)]
        self.assertEqual(sorted(ids), [1, 2, 3])
        self.assertEqual(store.last_message_id(channel.key()), 3)

    def test_backfill_limit_zero_does_nothing(self):
        config = make_config(self.tmp)
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        watcher = runner.Watcher(config, store, FakeSink(), FakeLookup())
        client = FakeClient([])
        self.assertEqual(asyncio.run(runner._backfill_newest(client, watcher, config.channels[0], object(), 0)), 0)
        self.assertEqual(client.calls, [])


class GapBackfillTest(unittest.TestCase):
    """review F5：宕机期间的消息按游标补拉。"""

    def setUp(self):
        self.tmp = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.tmp)

    def test_only_messages_after_the_cursor(self):
        config = make_config(self.tmp)
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        sink = FakeSink()
        watcher = runner.Watcher(config, store, sink, FakeLookup())
        store.advance_message_id(config.channels[0].key(), 5)
        client = FakeClient(
            [FakeMessage(7, "new2", HIDDEN), FakeMessage(6, "new1", HIDDEN), FakeMessage(5, "old", HIDDEN)]
        )

        count = asyncio.run(runner._gap_backfill(client, watcher, config.channels[0], object(), 100))
        self.assertEqual(count, 2)
        self.assertEqual(client.calls[0]["min_id"], 5)
        urls = [item["url"] for item in sink.sent]
        self.assertEqual(len(urls), 2)
        self.assertEqual(store.last_message_id(config.channels[0].key()), 7)

    def test_no_cursor_means_no_gap_pull(self):
        config = make_config(self.tmp)
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        watcher = runner.Watcher(config, store, FakeSink(), FakeLookup())
        client = FakeClient([FakeMessage(3, "x", HIDDEN)])
        self.assertEqual(
            asyncio.run(runner._gap_backfill(client, watcher, config.channels[0], object(), 100)), 0
        )
        self.assertEqual(client.calls, [])


class SessionTest(unittest.TestCase):
    def setUp(self):
        self.tmp = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.tmp)

    def test_corrupt_session_becomes_runner_error(self):
        config = make_config(self.tmp)
        config.session_dir.mkdir(parents=True, exist_ok=True)
        config.session_path.write_text("not a database", encoding="utf-8")
        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(config):
                raise sqlite3.DatabaseError("file is not a database")
        self.assertIn("session 文件损坏", str(ctx.exception))
        self.assertIn(str(config.session_path), str(ctx.exception))

    @unittest.skipUnless(os.name == "posix", "POSIX 权限位")
    def test_harden_permissions(self):
        config = make_config(self.tmp)
        config.session_dir.mkdir(parents=True, exist_ok=True)
        config.session_path.write_text("x", encoding="utf-8")
        os.chmod(config.session_path, 0o644)
        os.chmod(config.session_dir, 0o755)

        runner.harden_permissions(config)

        self.assertEqual(stat.S_IMODE(config.session_path.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(config.session_dir.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE(config.state_dir.stat().st_mode), 0o700)


class LoginFlowTest(unittest.TestCase):
    """文档要求的登录录入方式：验证码写进 session/code.txt，或 tty 里直接敲。"""

    def test_reads_code_from_session_dir_and_deletes_it(self):
        with support.temp_dir() as tmp:
            config = make_config(Path(tmp))
            config.session_dir.mkdir(parents=True, exist_ok=True)
            config.code_file.write_text("12345\n", encoding="utf-8")
            provider = runner.code_provider(config)
            with patch.object(runner.sys, "stdin", io.StringIO("")):
                code = provider()
            self.assertEqual(code, "12345")
            self.assertFalse(config.code_file.exists())

    def test_falls_back_to_tty_input(self):
        class Tty(io.StringIO):
            def isatty(self):
                return True

        with support.temp_dir() as tmp:
            config = make_config(Path(tmp))
            config.session_dir.mkdir(parents=True, exist_ok=True)
            provider = runner.code_provider(config)
            with patch.object(runner.sys, "stdin", Tty("67890\n")):
                code = provider()
            self.assertEqual(code, "67890")


class EntitiesTest(unittest.TestCase):
    def test_entities_to_dicts(self):
        class Entity:
            offset = 0
            length = 4
            url = "https://115.com/s/1"

        class Msg:
            entities = [Entity()]

        self.assertEqual(
            runner.entities_to_dicts(Msg()),
            [{"type": "Entity", "offset": 0, "length": 4, "url": "https://115.com/s/1"}],
        )

    def test_peer_key(self):
        class Chat:
            username = "chan"
            id = 1

        class PrivateChat:
            username = None
            id = 5

        self.assertEqual(runner.peer_key(Chat(), -1001), "@chan")
        self.assertEqual(runner.peer_key(PrivateChat(), -1005), "id:-1005")


class LibraryLookupTest(unittest.TestCase):
    def test_caches_per_title(self):
        calls = []

        def fake_lookup(url, api_key, title, timeout):
            calls.append((url, api_key, title))
            return {"ok": True, "size_bytes": 1, "matched": None}

        with patch("tgwatch.sink.lookup_size", fake_lookup):
            lookup = runner.LibraryLookup("http://lux/lookup", "k", 5.0)
            asyncio.run(lookup.lookup("某剧"))
            asyncio.run(lookup.lookup("某剧"))
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0], ("http://lux/lookup", "k", "某剧"))

    def test_empty_title_is_unknown(self):
        lookup = runner.LibraryLookup("http://lux/lookup", "k", 5.0)
        result = asyncio.run(lookup.lookup(""))
        self.assertFalse(result["ok"])


if __name__ == "__main__":
    unittest.main()
