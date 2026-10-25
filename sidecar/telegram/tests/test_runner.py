import asyncio
import contextlib
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



def _restore_mode(path, mode: int) -> None:
    """Restore a permission bit for cleanup; the temporary tree may be gone."""
    try:
        os.chmod(path, mode)
    except FileNotFoundError:
        pass

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
        # 补集测试要快：不真睡 5 秒，也不让默认单轮预算截断测试里的清单
        complete_pause=0.0,
        complete_max_searches=0,
        complete_max_hits=0,
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


class PerLinkTitleTest(unittest.TestCase):
    """review SC7：一条消息多个链接时，每个链接用自己的标题；拿不到就挂起不猜。"""

    def setUp(self):
        self.tmp = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.tmp)

    def build(self, config, lookup=None, sink=None):
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        watcher = runner.Watcher(config, store, sink or FakeSink(), lookup or FakeLookup())
        return watcher, store

    # 「第一集」挂隐藏链接，另一个链接只有明文 URL（没有锚文本）
    ONE_ANCHOR_TEXT = "第一集 https://115.com/s/two"
    ONE_ANCHOR_ENTITIES = [
        {"type": "MessageEntityTextUrl", "offset": 0, "length": 3, "url": "https://115.com/s/one"}
    ]

    def test_link_without_own_title_is_held_not_guessed(self):
        config = make_config(self.tmp)
        sink = FakeSink()
        watcher, store = self.build(config, sink=sink)
        asyncio.run(
            watcher.process(message(self.ONE_ANCHOR_TEXT, self.ONE_ANCHOR_ENTITIES), config.channels[0])
        )

        # 有锚文本的那条正常投递，标题就是它自己的锚文本
        self.assertEqual(len(sink.sent), 1)
        self.assertEqual(sink.sent[0]["url"], "https://115.com/s/one")
        self.assertEqual(sink.sent[0]["title"], "第一集")
        # 没有独立标题的那条挂起，原因可见、且没有拿整段正文当标题
        held = store.items_by_status((store_mod.STATUS_HELD,))
        self.assertEqual(len(held), 1)
        self.assertEqual(held[0]["url"], "https://115.com/s/two")
        self.assertEqual(held[0]["reason"], "title_ambiguous")
        self.assertEqual(held[0]["title"], "")
        self.assertNotEqual(held[0]["title"], self.ONE_ANCHOR_TEXT)
        # 一条消息一行留痕，两个条目
        self.assertEqual(store.stats()["messages"], 1)
        self.assertEqual(store.stats()["items"], 2)
        self.assertEqual(watcher.counts["delivered"], 1)
        self.assertEqual(watcher.counts["held"], 1)

    def test_single_link_still_falls_back_to_first_line(self):
        """单链接消息没有锚文本时，首行正文就是它的标题（保持老行为）。"""
        config = make_config(self.tmp)
        sink = FakeSink()
        watcher, _ = self.build(config, sink=sink)
        asyncio.run(
            watcher.process(
                message(text="沙丘2 2024 https://115.com/s/plain 12G"), config.channels[0]
            )
        )
        self.assertEqual(len(sink.sent), 1)
        self.assertTrue(sink.sent[0]["title"])
        self.assertIn("沙丘2 2024", sink.sent[0]["title"])

    # 两个链接各自有锚文本
    TWO_ANCHORS_TEXT = "某剧A 某剧B 12G"
    TWO_ANCHORS_ENTITIES = [
        {"type": "MessageEntityTextUrl", "offset": 0, "length": 3, "url": "https://115.com/s/a"},
        {"type": "MessageEntityTextUrl", "offset": 4, "length": 3, "url": "https://115.com/s/b"},
    ]

    def test_wash_looks_up_each_link_by_its_own_title(self):
        config = make_config(
            self.tmp,
            channels=(config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_WASH),),
        )
        lookup = FakeLookup(size_bytes=5 * 1024**3)
        sink = FakeSink()
        watcher, _ = self.build(config, lookup=lookup, sink=sink)

        asyncio.run(
            watcher.process(message(self.TWO_ANCHORS_TEXT, self.TWO_ANCHORS_ENTITIES), config.channels[0])
        )

        # 两条链接各查各的标题，不再用「整条消息一个标题」查一次
        self.assertEqual(lookup.calls, ["某剧A", "某剧B"])
        self.assertEqual(len(sink.sent), 2)
        self.assertEqual([item["title"] for item in sink.sent], ["某剧A", "某剧B"])

    def test_wash_holds_untitled_link_without_querying_the_library(self):
        config = make_config(
            self.tmp,
            channels=(config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_WASH),),
        )
        lookup = FakeLookup(size_bytes=5 * 1024**3)
        sink = FakeSink()
        watcher, store = self.build(config, lookup=lookup, sink=sink)

        asyncio.run(
            watcher.process(message(self.ONE_ANCHOR_TEXT + " 12G", self.ONE_ANCHOR_ENTITIES), config.channels[0])
        )

        # 只按有标题的那条查库；缺标题的那条不查、不猜
        self.assertEqual(lookup.calls, ["第一集"])
        self.assertEqual([item["url"] for item in sink.sent], ["https://115.com/s/one"])
        held = store.items_by_status((store_mod.STATUS_HELD,))
        self.assertEqual([item["reason"] for item in held], ["title_ambiguous"])


class MessageKeyTest(unittest.TestCase):
    """review SC12：去重键统一成 `channel:msg_id`，backfill 与 handler 必须一致。"""

    def setUp(self):
        self.tmp = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.tmp)

    def test_backfill_and_handler_produce_the_same_key(self):
        channel = config_mod.ChannelConfig(channel="@chan")
        # backfill：`iter_messages` 拿到的消息没有 chat_id 属性 → 老代码造出 0:42
        backfill_message = FakeMessage(42, "某剧", HIDDEN, chat_id=None)
        # handler：`event.chat_id` 是 -100… → 老代码造出 -100123:42
        handler_message = FakeMessage(42, "某剧", HIDDEN, chat_id=-100123)

        backfill = runner.incoming_from_message(backfill_message, channel.key())
        handler = runner.incoming_from_message(handler_message, channel.key(), -100123)

        # channel.key() 会把 `@` 去掉，所以键是 `chan:42`
        self.assertEqual(backfill.key, "chan:42")
        self.assertEqual(backfill.key, f"{channel.key()}:42")
        self.assertEqual(handler.key, "chan:42")
        self.assertEqual(backfill.key, handler.key)
        # 旧格式（chat_id:msg_id、0:msg_id）不再生成
        self.assertNotEqual(backfill.key, f"{backfill.chat_id}:{backfill.msg_id}")
        self.assertNotEqual(handler.key, f"{handler.chat_id}:{handler.msg_id}")
        self.assertFalse(backfill.key.startswith("0:"))
        self.assertNotIn(str(-100123), handler.key)

    def test_key_uses_the_normalized_channel_label(self):
        channel = config_mod.ChannelConfig(channel="https://t.me/chan")
        self.assertEqual(channel.key(), "chan")
        incoming = runner.IncomingMessage(channel=channel.key(), chat_id=0, msg_id=7, date="")
        self.assertEqual(incoming.key, "chan:7")

    def test_same_message_from_backfill_then_handler_is_delivered_once(self):
        config = make_config(self.tmp)
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        sink = FakeSink()
        watcher = runner.Watcher(config, store, sink, FakeLookup())
        channel = config.channels[0]

        async def scenario():
            await watcher.process(
                runner.incoming_from_message(FakeMessage(42, "某剧", HIDDEN, chat_id=None), channel.key()),
                channel,
            )
            # 同一条消息随后又从 handler 进来（event.chat_id 是 -100…）
            await watcher.process(
                runner.incoming_from_message(
                    FakeMessage(42, "某剧", HIDDEN, chat_id=-100123), channel.key(), -100123
                ),
                channel,
            )

        asyncio.run(scenario())
        self.assertEqual(len(sink.sent), 1, "同一 channel/msg_id 只能投递一次")
        stats = store.stats()
        self.assertEqual(stats["messages"], 1)
        self.assertEqual(stats["items"], 1)


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
        # 两条消息带的是同一个分享链接：回执（阶段 4）保证同一条链接只转存一次，
        # 但两条消息都各自入库，避免丢掉「这条消息出现过」的事实。
        self.assertEqual(len(urls), 1)
        self.assertEqual(store.stats()["items"], 2)
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
        self.addCleanup(_restore_mode, directory, 0o700)
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


try:  # 只有装了 telethon 的环境才跑「真 client」用例（容器里装了，开发机常常没装）
    from telethon import TelegramClient as _RealTelegramClient
except ImportError:  # pragma: no cover - 取决于环境
    _RealTelegramClient = None


@unittest.skipUnless(_RealTelegramClient is not None, "需要 telethon：pip install -r requirements.txt")
class RealSessionCorruptionTest(unittest.TestCase):
    """review SC13：垃圾 session 文件走**真的** `TelegramClient`，不手工 raise。

    老用例是 `raise sqlite3.DatabaseError("file is not a database")`——它只测了
    `session_errors` 的分类表，并没有证明 telethon 真的会抛这个异常、也没有证明
    我们的 with 块罩得住构造/连接阶段。这里把垃圾字节写进 session 路径，让
    telethon 自己去读它。
    """

    def setUp(self):
        self.tmp = support.mkdtemp()
        self.config = make_config(self.tmp)
        self.config.session_dir.mkdir(parents=True, exist_ok=True)

    def tearDown(self):
        support.cleanup(self.tmp)

    def test_garbage_session_becomes_runner_error(self):
        self.config.session_path.write_bytes(b"\x00\x01this is not a database" * 64)

        def open_client() -> None:
            # telethon 1.45 的 SQLiteSession 在构造时就 `select ... from sqlite_master`，
            # 所以垃圾文件在这里就炸；万一某个版本改成延迟读取，下面 connect() 之前
            # 读 auth key 也会先碰到 sqlite（不会真连网）。
            client = _RealTelegramClient(
                str(self.config.session_path), 123456, "0123456789abcdef0123456789abcdef"
            )
            asyncio.run(client.connect())

        with self.assertRaises(runner.RunnerError) as ctx:
            with runner.session_errors(self.config):
                open_client()
        message = str(ctx.exception)
        self.assertIn("session 文件损坏", message)
        self.assertIn(str(self.config.session_path), message)
        self.assertIn("not a database", message.lower())


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


class RecordingWatcher:
    """补集核的最小 watcher 形状：只记 process 过什么（真实 Watcher 的契约子集）。"""

    def __init__(self):
        self.seen = []
        self.counts = {"delivered": 0}

    async def process(self, incoming, channel):
        self.seen.append((incoming.msg_id, channel.key()))


class FloodClient:
    """搜索一开口就被 Telegram FloodWait 拒（异常带 seconds）。"""

    def __init__(self, seconds=300):
        self.seconds = seconds
        self.calls = 0

    async def iter_messages(self, entity, **kwargs):
        self.calls += 1

        class FloodWait(RuntimeError):
            pass

        error = FloodWait(f"FLOOD_WAIT_{self.seconds}")
        error.seconds = self.seconds
        raise error
        yield  # pragma: no cover - 占位：让函数成为 async 生成器


class CompleteSearchTest(unittest.TestCase):
    """`_complete_search` / `_periodic_complete`：补集核、防风控节流与常驻定时循环。"""

    def setUp(self):
        self.tmp = support.mkdtemp()
        self.channel = config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_FOLLOW)

    def tearDown(self):
        support.cleanup(self.tmp)

    def test_search_only_processes_missing_episodes(self):
        messages = [
            FakeMessage(1, "某剧 S01E02 1080p"),
            FakeMessage(2, "某剧 S01E03 2160p"),
            FakeMessage(3, "不相关消息"),
        ]
        watcher = RecordingWatcher()
        result = asyncio.run(
            runner._complete_search(
                FakeClient(messages), watcher, [(self.channel, object())], [("某剧", {(1, 3)})], 50
            )
        )
        self.assertEqual((result.scanned, result.matched, result.attempted), (3, 1, 1))
        self.assertEqual(result.flood_seconds, 0.0)
        self.assertEqual(watcher.seen, [(2, "chan")])

    def test_search_survives_a_failing_query(self):
        class ExplodingClient:
            async def iter_messages(self, entity, **kwargs):
                raise RuntimeError("search down")
                yield  # pragma: no cover - 占位：让函数成为 async 生成器

        watcher = RecordingWatcher()
        result = asyncio.run(
            runner._complete_search(
                ExplodingClient(), watcher, [(self.channel, object())], [("某剧", {(1, 3)})], 50
            )
        )
        self.assertEqual((result.scanned, result.matched, result.attempted), (0, 0, 1))
        self.assertEqual(watcher.seen, [])

    def test_search_budget_splits_a_big_list_across_rounds(self):
        """单轮搜索预算：本轮只搜一次，next_start 指到下一部剧，第二轮接着搜。"""
        plan = [("剧A", {(1, 1)}), ("剧B", {(1, 2)})]
        client = FakeClient([FakeMessage(1, "剧A S01E01")])
        watcher = RecordingWatcher()
        first = asyncio.run(
            runner._complete_search(
                client, watcher, [(self.channel, object())], plan, 50, max_searches=1
            )
        )
        self.assertEqual(first.attempted, 1)
        self.assertEqual(first.next_start, 1)
        self.assertEqual(client.calls[0]["search"], "剧A")

        second = asyncio.run(
            runner._complete_search(
                client,
                watcher,
                [(self.channel, object())],
                plan,
                50,
                max_searches=1,
                start=first.next_start,
            )
        )
        self.assertEqual(second.attempted, 1)
        self.assertEqual(client.calls[1]["search"], "剧B")

    def test_search_hit_budget_caps_a_whole_season_burst(self):
        """命中预算：整季命中也不会一口气全灌下去（消息从最老的开始处理）。"""
        messages = [
            FakeMessage(1, "某剧 S01E01 1080p"),
            FakeMessage(2, "某剧 S01E02 1080p"),
            FakeMessage(3, "某剧 S01E03 1080p"),
        ]
        watcher = RecordingWatcher()
        result = asyncio.run(
            runner._complete_search(
                FakeClient(messages),
                watcher,
                [(self.channel, object())],
                [("某剧", {(1, 1), (1, 2), (1, 3)})],
                50,
                max_hits=1,
            )
        )
        self.assertEqual(result.matched, 1)
        self.assertEqual(watcher.seen, [(3, "chan")])

    def test_search_pauses_between_queries(self):
        pauses = []

        async def fake_sleep(seconds):
            pauses.append(seconds)

        plan = [("剧A", {(1, 1)}), ("剧B", {(1, 2)})]
        with patch.object(runner.asyncio, "sleep", fake_sleep):
            result = asyncio.run(
                runner._complete_search(
                    FakeClient([]), RecordingWatcher(), [(self.channel, object())], plan, 50, pause=7.5
                )
            )
        self.assertEqual(result.attempted, 2)
        self.assertEqual(pauses, [7.5])

    def test_search_stops_on_flood_wait(self):
        client = FloodClient(seconds=300)
        watcher = RecordingWatcher()
        plan = [("剧A", {(1, 1)}), ("剧B", {(1, 2)})]
        result = asyncio.run(
            runner._complete_search(client, watcher, [(self.channel, object())], plan, 50)
        )
        self.assertEqual(result.attempted, 1)
        self.assertEqual(result.flood_seconds, 300.0)
        self.assertEqual(client.calls, 1)
        self.assertEqual(watcher.seen, [])

    def test_periodic_complete_rounds_and_survives_errors(self):
        """第一轮真跑、第二轮拿不到清单也不能拖死循环；取消即退出。"""

        async def scenario():
            config = make_config(self.tmp, complete_interval=0)
            calls = []

            async def fake_missing(cfg):
                calls.append(1)
                if len(calls) == 1:
                    return {"series": []}, [("某剧", {(1, 3)})]
                raise runner.planner_mod.PlannerError("Lux 不可达")

            watcher = RecordingWatcher()
            with patch.object(runner, "_missing_plan", fake_missing):
                task = asyncio.create_task(
                    runner._periodic_complete(
                        config,
                        FakeClient([FakeMessage(2, "某剧 S01E03 2160p")]),
                        watcher,
                        [(self.channel, object())],
                    )
                )
                deadline = asyncio.get_running_loop().time() + 5.0
                while len(calls) < 3:
                    if asyncio.get_running_loop().time() > deadline:
                        task.cancel()
                        raise AssertionError(f"定时补集只跑了 {len(calls)} 轮")
                    await asyncio.sleep(0.01)
                task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await task
            return watcher.seen, calls

        seen, calls = asyncio.run(scenario())
        self.assertGreaterEqual(len(calls), 3)
        self.assertEqual(seen, [(2, "chan")])


class ChannelIdTest(unittest.TestCase):
    """判定接口的频道主键：必须和 Lux 设置里的裸 id 一致（2026-10-25 线上踩到 404）。"""

    def test_strips_the_channel_mark(self):
        self.assertEqual(runner.lux_channel_id(-1002769844611), "2769844611")
        self.assertEqual(runner.lux_channel_id("-1004358697065"), "4358697065")

    def test_leaves_plain_and_group_ids_alone(self):
        self.assertEqual(runner.lux_channel_id(2769844611), "2769844611")
        self.assertEqual(runner.lux_channel_id(-123456), "-123456")

    def test_on_event_is_awaitable(self):
        """telethon 会 await 回调；同步 def 会让每条消息抛 NoneType 错误。"""
        self.assertTrue(asyncio.iscoroutinefunction(runner.Watcher.on_event))


if __name__ == "__main__":
    unittest.main()
