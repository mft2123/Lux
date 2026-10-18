import asyncio
import io
import json
import os
import sqlite3
import stat
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from unittest.mock import patch

from tests import support
from tgwatch import config as config_mod
from tgwatch import filters, links as links_mod, runner, sink as sink_mod, store as store_mod


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

    def test_news_link_is_not_delivered_by_default(self):
        """review SC5：other（新闻/网页）默认不投递，但消息留痕（skipped/no_allowed_link）。"""
        config = make_config(self.tmp)
        sink = FakeSink()
        watcher, store = self.build(config, sink=sink)
        asyncio.run(watcher.process(message(text="新片资讯 https://example.com/news/1"), config.channels[0]))
        self.assertEqual(sink.sent, [])
        self.assertEqual(store.stats()["items"], 0)
        stats = store.stats()
        self.assertEqual(stats["messages"], 1)
        self.assertEqual(stats["by_status"], {})  # 消息级记录，没有条目
        self.assertEqual(watcher.counts["skipped"], 1)

    def test_news_link_is_delivered_when_other_is_allowed(self):
        config = make_config(
            self.tmp, allowed_kinds=(*links_mod.DEFAULT_ALLOWED_KINDS, links_mod.KIND_OTHER)
        )
        sink = FakeSink()
        watcher, _ = self.build(config, sink=sink)
        asyncio.run(watcher.process(message(text="新片资讯 https://example.com/news/1"), config.channels[0]))
        self.assertEqual(len(sink.sent), 1)
        self.assertEqual(sink.sent[0]["kind"], links_mod.KIND_OTHER)

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

    def test_http_body_carries_traceability_fields(self):
        """review SC6：真正发出去的 body 要带 channel/message_id/captured_at 等溯源字段。"""
        captured = {}

        class Response:
            status = 202

            def read(self, size=None):
                return b'{"ok":true,"jobId":"1-x","duplicate":false}'

            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

        def fake_urlopen(request, timeout=None):
            captured["body"] = json.loads(request.data.decode("utf-8"))
            return Response()

        config = make_config(self.tmp)
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        watcher = runner.Watcher(
            config, store, sink_mod.HttpSink(url="http://lux:8097/ingest", api_key="k"), FakeLookup()
        )
        with patch.object(urllib.request, "urlopen", fake_urlopen):
            asyncio.run(watcher.process(message(entities=HIDDEN), config.channels[0]))

        body = captured["body"]
        self.assertEqual(body["url"], "https://115.com/s/abc")
        self.assertEqual(body["source"], "telegram")
        self.assertEqual(body["kind"], "115")
        self.assertEqual(body["channel"], "@chan")
        self.assertEqual(body["message_id"], 42)
        self.assertEqual(body["captured_at"], "2026-10-18T00:00:00+00:00")
        self.assertIsNone(body["size_bytes"])  # 正文里没写体积

    def test_unexpected_sink_exception_is_recorded(self):
        """review SC18：SinkError 之外的异常也要落 failed，不能卡在 pending 无限重试。"""

        class ExplodingSink(FakeSink):
            async def send(self, item):
                raise ValueError("cannot serialize payload")

        config = make_config(self.tmp)
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        watcher = runner.Watcher(config, store, ExplodingSink(), FakeLookup())
        asyncio.run(watcher.process(message(entities=HIDDEN), config.channels[0]))

        failed = store.items_by_status((store_mod.STATUS_FAILED,))
        self.assertEqual(len(failed), 1)
        self.assertEqual(failed[0]["attempts"], 1)
        self.assertIn("cannot serialize payload", failed[0]["reason"])

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


class DeadLetterTest(unittest.TestCase):
    """review SC4：400/403 只尝试一次就落死信；其余错误重试到上限后落死信。"""

    def setUp(self):
        self.tmp = support.mkdtemp()
        self.config = make_config(self.tmp)

    def tearDown(self):
        support.cleanup(self.tmp)

    def build(self, sink):
        store = store_mod.Store(self.config.state_db)
        self.addCleanup(store.close)
        return runner.Watcher(self.config, store, sink, FakeLookup()), store

    def _deliver_with_http_status(self, code):
        """用真的 HttpSink + 假的 urlopen 造 400/403：验证「不再重试」的整条链路。"""

        def boom(*args, **kwargs):
            raise urllib.error.HTTPError(
                "http://lux:8097/api/v1/admin/archive/ingest",
                code,
                "denied",
                {},
                io.BytesIO('{"error":"no"}'.encode("utf-8")),
            )

        watcher, store = self.build(
            sink_mod.HttpSink(url="http://lux:8097/api/v1/admin/archive/ingest", api_key="k")
        )
        with patch.object(urllib.request, "urlopen", boom):
            asyncio.run(watcher.process(message(entities=HIDDEN), self.config.channels[0]))
            dead = store.items_by_status((store_mod.STATUS_DEAD,))
            self.assertEqual(len(dead), 1, "400/403 应当直接落死信")
            self.assertEqual(dead[0]["attempts"], 1, "永久失败只允许尝试一次")
            self.assertIn("永久", dead[0]["reason"])
            self.assertIn(str(code), dead[0]["reason"])
            self.assertEqual(store.items_by_status((store_mod.STATUS_FAILED,)), [])
            # 死信不参与自动重试
            self.assertEqual(asyncio.run(watcher.retry_once()), (0, 0))
            self.assertEqual(watcher.counts["dead"], 1)
        stats = store.stats()
        self.assertEqual(stats["dead"], 1)
        self.assertTrue(any("永久" in reason for reason in stats["dead_reasons"]))

    def test_400_is_dead_after_one_attempt(self):
        self._deliver_with_http_status(400)

    def test_403_is_dead_after_one_attempt(self):
        self._deliver_with_http_status(403)

    def test_retryable_error_dies_after_max_attempts(self):
        flaky = FakeSink()
        flaky.fail = True
        watcher, store = self.build(flaky)
        asyncio.run(watcher.process(message(entities=HIDDEN), self.config.channels[0]))
        self.assertEqual(store.items_by_status((store_mod.STATUS_FAILED,))[0]["attempts"], 1)

        for _ in range(runner.MAX_DELIVERY_ATTEMPTS - 1):
            asyncio.run(watcher.retry_once())

        dead = store.items_by_status((store_mod.STATUS_DEAD,))
        self.assertEqual(len(dead), 1)
        self.assertEqual(dead[0]["attempts"], runner.MAX_DELIVERY_ATTEMPTS)
        self.assertIn("max_attempts", dead[0]["reason"])
        # 死信计数与原因能在 status/stats 里查到
        stats = store.stats()
        self.assertEqual(stats["dead"], 1)
        self.assertTrue(any("max_attempts" in reason for reason in stats["dead_reasons"]))
        # 到上限后不再被 retry_once 捞起来
        self.assertEqual(asyncio.run(watcher.retry_once()), (0, 0))

    def test_backoff_grows_and_caps(self):
        self.assertEqual(runner.backoff_seconds(300, 0), 300)
        self.assertEqual(runner.backoff_seconds(300, 1), 600)
        self.assertEqual(runner.backoff_seconds(300, 2), 1200)
        self.assertEqual(runner.backoff_seconds(300, 99), runner.MAX_BACKOFF_SECONDS)
        self.assertEqual(runner.backoff_seconds(0, 1), 2)

    def test_retry_loop_uses_exponential_backoff(self):
        watcher, _ = self.build(FakeSink())
        delays = []

        async def fake_wait(seconds):
            delays.append(seconds)
            return len(delays) >= 3  # 第三次「等到 stop」，结束循环

        async def always_fail(limit=50):
            return 1, 1

        watcher._wait = fake_wait
        watcher.retry_once = always_fail
        asyncio.run(watcher.retry_loop())
        base = watcher.retry_interval()
        self.assertEqual(delays, [base, base * 2, base * 4])

    def test_retry_loop_resets_after_a_clean_round(self):
        watcher, _ = self.build(FakeSink())
        delays = []

        async def fake_wait(seconds):
            delays.append(seconds)
            return len(delays) >= 3

        rounds = [(1, 0), (1, 1), (1, 1)]

        async def flaky(limit=50):
            return rounds.pop(0)

        watcher._wait = fake_wait
        watcher.retry_once = flaky
        asyncio.run(watcher.retry_loop())
        base = watcher.retry_interval()
        # 第一轮全成功 → 回到 base；之后连续失败 → 翻倍
        self.assertEqual(delays, [base, base, base * 2])


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


class SessionErrorClassificationTest(unittest.TestCase):
    """review SC3：只有「文件被写坏」才叫 session 损坏，权限/磁盘类错误要原样报出。"""

    def setUp(self):
        self.tmp = support.mkdtemp()
        self.config = make_config(self.tmp)
        self.config.session_dir.mkdir(parents=True, exist_ok=True)

    def tearDown(self):
        support.cleanup(self.tmp)

    def _open_and_write(self):
        connection = sqlite3.connect(self.config.session_path)
        try:
            connection.execute("SELECT count(*) FROM sqlite_master").fetchall()
            connection.execute("CREATE TABLE IF NOT EXISTS probes (id INTEGER)")
            connection.commit()
        finally:
            connection.close()

    def test_garbage_file_is_corrupt(self):
        self.config.session_path.write_bytes(b"not a database, just junk" * 32)
        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(self.config):
                self._open_and_write()
        message = str(ctx.exception)
        self.assertIn("session 文件损坏", message)
        self.assertIn(str(self.config.session_path), message)
        self.assertIn("not a database", message.lower())
        self.assertIn("重新走登录流程", message)

    def test_other_database_error_is_not_called_corrupt(self):
        # 不含 not a database / malformed 的 DatabaseError 不许被说成损坏
        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(self.config):
                raise sqlite3.DatabaseError("disk image mismatch on page 7")
        message = str(ctx.exception)
        self.assertNotIn("损坏", message)
        self.assertIn("session 读取失败", message)
        self.assertIn("disk image mismatch on page 7", message)

    @unittest.skipUnless(os.name == "posix", "需要 POSIX 权限位")
    def test_readonly_session_file_reports_operational_error(self):
        if not support.permissions_enforced(self.tmp):
            self.skipTest("文件系统不校验 chmod（root 或忽略权限位的挂载点），权限用例会失真")
        # 文件是真的 sqlite，但不可写：报「不可写」，不许叫人删 session
        connection = sqlite3.connect(self.config.session_path)
        connection.execute("CREATE TABLE sessions (id INTEGER)")
        connection.commit()
        connection.close()
        os.chmod(self.config.session_path, 0o400)
        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(self.config):
                connection = sqlite3.connect(self.config.session_path)
                try:
                    connection.execute("INSERT INTO sessions (id) VALUES (1)")
                    connection.commit()
                finally:
                    connection.close()
        message = str(ctx.exception)
        self.assertNotIn("损坏", message)
        self.assertIn("session 无法打开或写入", message)
        self.assertIn(str(self.config.session_path), message)
        # 原始 sqlite 文案要带上（readonly / unable to open），操作者才能对症
        self.assertIn(str(ctx.exception.__cause__), message)
        self.assertIn("权限", message)

    @unittest.skipUnless(os.name == "posix", "需要 POSIX 权限位")
    def test_readonly_directory_reports_operational_error(self):
        if not support.permissions_enforced(self.tmp):
            self.skipTest("文件系统不校验 chmod（root 或忽略权限位的挂载点），权限用例会失真")
        # session 目录只读：sqlite 连文件都建不出来（unable to open database file）
        directory = self.config.session_dir / "ro"
        directory.mkdir()
        os.chmod(directory, 0o500)
        self.addCleanup(os.chmod, directory, 0o700)
        config = make_config(self.tmp, session_dir=directory)
        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(config):
                connection = sqlite3.connect(config.session_path)
                try:
                    connection.execute("CREATE TABLE sessions (id INTEGER)")
                    connection.commit()
                finally:
                    connection.close()
        message = str(ctx.exception)
        self.assertNotIn("损坏", message)
        self.assertIn("session 无法打开或写入", message)
        self.assertIn(str(config.session_path), message)
        self.assertIn(str(ctx.exception.__cause__), message)
        self.assertIn("磁盘空间", message)

    def test_read_only_database_mode_reports_operational_error(self):
        """与容器里「session 属 root、进程是 10001」等价：sqlite 报 readonly database。"""
        connection = sqlite3.connect(self.config.session_path)
        connection.execute("CREATE TABLE sessions (id INTEGER)")
        connection.commit()
        connection.close()
        read_only = f"file:{self.config.session_path}?mode=ro"
        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(self.config):
                connection = sqlite3.connect(read_only, uri=True)
                try:
                    connection.execute("INSERT INTO sessions (id) VALUES (1)")
                    connection.commit()
                finally:
                    connection.close()
        message = str(ctx.exception)
        self.assertNotIn("损坏", message)
        self.assertIn("session 无法打开或写入", message)
        self.assertIn("readonly", message.lower())

    def test_missing_session_directory_reports_operational_error(self):
        # 目录不可用（拿一个普通文件当父目录）：sqlite 建不出文件
        blocker = self.tmp / "not-a-directory"
        blocker.write_text("x", encoding="utf-8")
        config = make_config(self.tmp, session_dir=blocker / "nested")
        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(config):
                connection = sqlite3.connect(config.session_path)
                try:
                    connection.execute("CREATE TABLE sessions (id INTEGER)")
                    connection.commit()
                finally:
                    connection.close()
        message = str(ctx.exception)
        self.assertNotIn("损坏", message)
        self.assertIn("session 无法打开或写入", message)
        self.assertIn(str(config.session_path), message)

    def test_errno_from_the_driver_is_kept(self):
        cause = sqlite3.OperationalError("disk I/O error")
        cause.errno = 28  # sqlite3 通常不带 errno；带上时必须原样露出来
        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(self.config):
                raise cause
        message = str(ctx.exception)
        self.assertIn("disk I/O error", message)
        self.assertIn("errno 28", message)


class PhoneValidationTest(unittest.TestCase):
    """review SC16：首次登录缺 phone 要抛 RunnerError（exit 2），不是 telethon 的裸异常。"""

    class FakeClient:
        def __init__(self):
            self.started = False
            self.kwargs = {}

        async def start(self, **kwargs):
            self.started = True
            self.kwargs = kwargs

        async def get_me(self):
            from types import SimpleNamespace

            return SimpleNamespace(id=1, username="me")

    def test_first_login_without_phone_is_runner_error(self):
        with support.temp_dir() as tmp:
            config = make_config(Path(tmp), phone="")
            client = self.FakeClient()
            with self.assertRaises(runner.RunnerError) as ctx:
                asyncio.run(runner._authorize(client, config))
        self.assertIn("phone", str(ctx.exception))
        self.assertIn("LUX_TG_PHONE", str(ctx.exception))
        self.assertFalse(client.started)

    def test_first_login_with_phone_starts(self):
        with support.temp_dir() as tmp:
            config = make_config(Path(tmp), phone="+10000000000")
            client = self.FakeClient()
            asyncio.run(runner._authorize(client, config))
        self.assertTrue(client.started)
        self.assertEqual(client.kwargs["phone"], "+10000000000")

    def test_existing_session_does_not_require_phone(self):
        with support.temp_dir() as tmp:
            config = make_config(Path(tmp), phone="")
            config.session_dir.mkdir(parents=True, exist_ok=True)
            config.session_path.write_text("x", encoding="utf-8")
            client = self.FakeClient()
            asyncio.run(runner._authorize(client, config))
        self.assertTrue(client.started)
        self.assertIsNone(client.kwargs["phone"])


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


class ShutdownTest(unittest.TestCase):
    """review SC11：关库之前必须把后台 handler 任务收尾。"""

    def test_drain_background_cancels_pending_tasks(self):
        async def scenario():
            started = asyncio.Event()

            async def forever():
                started.set()
                await asyncio.sleep(3600)

            task = asyncio.create_task(forever())
            runner._BACKGROUND.add(task)
            task.add_done_callback(runner._BACKGROUND.discard)
            await started.wait()
            await runner._drain_background("测试")
            return task

        task = asyncio.run(scenario())
        self.assertTrue(task.cancelled())
        self.assertNotIn(task, runner._BACKGROUND)

    def test_drain_background_with_no_tasks_is_noop(self):
        asyncio.run(runner._drain_background("空"))

    def test_drain_background_collects_failures(self):
        async def scenario():
            async def boom():
                raise RuntimeError("handler died")

            task = asyncio.create_task(boom())
            runner._BACKGROUND.add(task)
            task.add_done_callback(runner._BACKGROUND.discard)
            await asyncio.sleep(0)
            await runner._drain_background("测试")
            return task

        task = asyncio.run(scenario())
        self.assertTrue(task.done())
        self.assertIsNotNone(task.exception())  # 取走异常，避免事件循环报 never retrieved


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
