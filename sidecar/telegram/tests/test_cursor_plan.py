"""首轮游标与 Lux 判定接入（全部离线）。"""

import asyncio
import unittest

from tests import support
from tgwatch import config as config_mod
from tgwatch import filters, planner as planner_mod, runner, sink as sink_mod, store as store_mod


HIDDEN = [{"type": "MessageEntityTextUrl", "offset": 0, "length": 9, "url": "https://115.com/s/abc"}]


class FakePlanner:
    def __init__(self, decisions=None, error=None):
        self.decisions = decisions or {}
        self.error = error
        self.calls = []

    def plan(self, channel_id, candidates):
        self.calls.append((channel_id, list(candidates)))
        if self.error:
            raise self.error
        return self.decisions


class CursorAndPlanTest(unittest.TestCase):
    def setUp(self):
        self.tmp = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.tmp)

    def config(self, **overrides):
        base = dict(
            api_id=1,
            api_hash="hash",
            api_key="k",
            session_dir=self.tmp / "session",
            state_dir=self.tmp / "state",
            inbox_dir=self.tmp / "inbox",
            default_mode=filters.MODE_NEW,
            channels=(config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_NEW),),
        )
        base.update(overrides)
        return config_mod.SidecarConfig(**base)

    def build(self, planner=None, sink=None):
        config = self.config()
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        watcher = runner.Watcher(config, store, sink or FakeSink(), None, planner)
        return watcher, store, config

    def message(self, msg_id=42, text="某剧 第二季 4K"):
        return runner.IncomingMessage(
            channel="@chan",
            chat_id=-100123,
            msg_id=msg_id,
            date="2026-10-18T00:00:00+00:00",
            text=text,
            entities=HIDDEN,
        )

    # ---- 首轮只建游标 ----

    def test_first_run_establishes_cursor_without_processing_history(self):
        watcher, store, config = self.build()

        class Client:
            async def iter_messages(self, entity, **kwargs):
                assert kwargs.get("limit") == 1, "首轮只该读一条"
                for msg_id in (300,):
                    yield _Message(msg_id)

        cursor = asyncio.run(runner._establish_cursor(Client(), watcher, config.channels[0], object()))
        self.assertEqual(cursor, 300)
        self.assertEqual(store.last_message_id("chan"), 300)
        # 建游标不等于处理消息：一条都不该进库。
        self.assertEqual(store.stats()["messages"], 0)
        self.assertEqual(store.stats()["items"], 0)

    def test_first_run_with_no_messages_leaves_cursor_empty(self):
        watcher, store, config = self.build()

        class Client:
            async def iter_messages(self, entity, **kwargs):
                return
                yield  # pragma: no cover - 空异步生成器

        self.assertEqual(
            asyncio.run(runner._establish_cursor(Client(), watcher, config.channels[0], object())),
            0,
        )
        self.assertEqual(store.last_message_id("chan"), 0)

    # ---- 判定接口 ----

    def test_plan_skip_is_recorded_and_not_delivered(self):
        sink = FakeSink()
        planner = FakePlanner({"https://115.com/s/abc": planner_mod.Planned("skip", "版本不比它差")})
        watcher, store, config = self.build(planner=planner, sink=sink)
        watcher.channel_ids["chan"] = "123"

        asyncio.run(watcher.process(self.message(), config.channels[0]))

        self.assertEqual(sink.sent, [])
        item = store.items_by_status((store_mod.STATUS_SKIPPED,))[0]
        self.assertIn("plan:skip", item["reason"])
        self.assertIn("版本不比它差", item["reason"])
        # 候选带着解析出来的集号和清晰度上报。
        _, candidates = planner.calls[0]
        self.assertEqual(candidates[0]["link"], "https://115.com/s/abc")

    def test_plan_follow_is_delivered(self):
        sink = FakeSink()
        planner = FakePlanner({"https://115.com/s/abc": planner_mod.Planned("follow", "缺 S01E03")})
        watcher, store, config = self.build(planner=planner, sink=sink)
        watcher.channel_ids["chan"] = "123"

        asyncio.run(watcher.process(self.message(), config.channels[0]))

        self.assertEqual(len(sink.sent), 1)
        self.assertEqual(store.stats()["by_status"].get(store_mod.STATUS_DELIVERED), 1)

    def test_planner_failure_falls_back_to_local_rules(self):
        sink = FakeSink()
        planner = FakePlanner(error=planner_mod.PlannerError("Lux 判定接口 HTTP 502"))
        watcher, store, config = self.build(planner=planner, sink=sink)
        watcher.channel_ids["chan"] = "123"

        asyncio.run(watcher.process(self.message(), config.channels[0]))

        # 接口挂了不能变成"什么都转存"或"什么都不转"：本地规则照旧（上新=投递）。
        self.assertEqual(len(sink.sent), 1)
        self.assertEqual(store.stats()["by_status"].get(store_mod.STATUS_DELIVERED), 1)

    def test_channel_without_lux_id_skips_the_planner(self):
        sink = FakeSink()
        planner = FakePlanner({"https://115.com/s/abc": planner_mod.Planned("skip", "x")})
        watcher, store, config = self.build(planner=planner, sink=sink)

        asyncio.run(watcher.process(self.message(), config.channels[0]))

        self.assertEqual(planner.calls, [])
        self.assertEqual(len(sink.sent), 1)


class _Message:
    def __init__(self, msg_id):
        self.id = msg_id


class FakeSink:
    name = "fake"

    def __init__(self):
        self.sent = []

    async def send(self, item):
        self.sent.append(item)

    def describe(self):
        return {"sink": self.name}


class CompletionPlanTest(unittest.TestCase):
    def test_plan_is_one_entry_per_series_with_episode_set(self):
        payload = {
            "series": [
                {
                    "title": "繁花",
                    "seasons": [
                        {"season": 1, "missing": [3, 1, 2]},
                        {"season": 2, "missing": [5]},
                    ],
                },
                {"title": "  ", "seasons": [{"season": 1, "missing": [1]}]},
                {"title": "无季", "seasons": []},
            ]
        }
        plan = runner.completion_plan(payload)
        self.assertEqual(len(plan), 1)
        title, missing = plan[0]
        self.assertEqual(title, "繁花")
        self.assertEqual(missing, {(1, 1), (1, 2), (1, 3), (2, 5)})

    def test_empty_payload_has_no_plan(self):
        self.assertEqual(runner.completion_plan({}), [])
        self.assertEqual(runner.completion_plan({"series": []}), [])

    def test_episodes_are_read_out_of_message_text(self):
        text = "🎥 国漫: 武神主宰 (2020) S01E702 热更 2160p WEB-DL"
        self.assertEqual(runner.episodes_in_text(text), {(1, 702)})
        self.assertEqual(runner.episodes_in_text("某剧 S1.E3 1080p"), {(1, 3)})
        self.assertEqual(runner.episodes_in_text("第12集 更新"), {(0, 12)})
        self.assertEqual(
            runner.episodes_in_text("S01E01 与 S01E02"), {(1, 1), (1, 2)}
        )
        self.assertEqual(runner.episodes_in_text("没有集号"), set())


class ReceiptTest(unittest.TestCase):
    """回执三态：submitted 不重发、uncertain 不自动重发、清理不淘汰未完成。"""

    def setUp(self):
        self.tmp = support.mkdtemp()
        self.store = store_mod.Store(self.tmp / "state.db")
        self.addCleanup(self.store.close)

    def tearDown(self):
        support.cleanup(self.tmp)

    def test_plan_then_submit(self):
        previous = self.store.plan_receipt("https://115.com/s/a", "@chan", 42, "某剧")
        self.assertEqual(previous, "")
        self.assertEqual(self.store.receipt("https://115.com/s/a")["status"], store_mod.RECEIPT_PLANNED)

        self.store.mark_receipt("https://115.com/s/a", store_mod.RECEIPT_SUBMITTED)
        # 第二次登记返回 submitted：调用方据此跳过发送。
        self.assertEqual(
            self.store.plan_receipt("https://115.com/s/a", "@chan", 42, "某剧"),
            store_mod.RECEIPT_SUBMITTED,
        )

    def test_uncertain_survives_and_is_reported(self):
        self.store.plan_receipt("https://115.com/s/b", "@chan", 1, None)
        self.store.mark_receipt("https://115.com/s/b", store_mod.RECEIPT_UNCERTAIN)
        self.assertEqual(
            self.store.plan_receipt("https://115.com/s/b", "@chan", 1, None),
            store_mod.RECEIPT_UNCERTAIN,
        )
        self.assertEqual(self.store.receipt_counts().get(store_mod.RECEIPT_UNCERTAIN), 1)

    def test_trim_only_evicts_submitted(self):
        for index in range(6):
            url = f"https://115.com/s/{index}"
            self.store.plan_receipt(url, "@chan", index, None)
            if index >= 4:
                continue  # 最后两条保持 planned
            self.store.mark_receipt(url, store_mod.RECEIPT_SUBMITTED)
        # 上限压到 3：应删掉最老的 submitted，planned 一条都不能动。
        removed = self.store.trim_receipts(cap=3)
        self.assertEqual(removed, 3)
        counts = self.store.receipt_counts()
        self.assertEqual(counts.get(store_mod.RECEIPT_SUBMITTED), 1)
        self.assertEqual(counts.get(store_mod.RECEIPT_PLANNED), 2)

    def test_trim_never_evicts_uncertain(self):
        for index in range(3):
            self.store.plan_receipt(f"https://115.com/s/u{index}", "@chan", index, None)
            self.store.mark_receipt(f"https://115.com/s/u{index}", store_mod.RECEIPT_UNCERTAIN)
        self.assertEqual(self.store.trim_receipts(cap=1), 0)
        self.assertEqual(self.store.receipt_counts().get(store_mod.RECEIPT_UNCERTAIN), 3)


class DeliverReceiptTest(unittest.TestCase):
    def setUp(self):
        self.tmp = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.tmp)

    def config(self):
        return config_mod.SidecarConfig(
            api_id=1,
            api_hash="hash",
            api_key="k",
            session_dir=self.tmp / "session",
            state_dir=self.tmp / "state",
            inbox_dir=self.tmp / "inbox",
            default_mode=filters.MODE_NEW,
            channels=(config_mod.ChannelConfig(channel="@chan", mode=filters.MODE_NEW),),
        )

    def build(self, sink):
        config = self.config()
        store = store_mod.Store(config.state_db)
        self.addCleanup(store.close)
        watcher = runner.Watcher(config, store, sink, None, None)
        return watcher, store, config

    def message(self):
        return runner.IncomingMessage(
            channel="@chan",
            chat_id=-100123,
            msg_id=7,
            date="2026-10-18T00:00:00+00:00",
            text="某剧 第二季 4K",
            entities=HIDDEN,
        )

    def test_success_writes_a_submitted_receipt_and_blocks_a_second_send(self):
        sink = FakeSink()
        watcher, store, config = self.build(sink)
        asyncio.run(watcher.process(self.message(), config.channels[0]))
        self.assertEqual(len(sink.sent), 1)
        self.assertEqual(
            store.receipt("https://115.com/s/abc")["status"], store_mod.RECEIPT_SUBMITTED
        )

        # 同一链接再入队（例如 replay）：回执说不重发。
        item_id = store.items_by_status((store_mod.STATUS_DELIVERED,))[0]["id"]
        store.set_status(int(item_id), store_mod.STATUS_PENDING)
        asyncio.run(watcher.deliver([int(item_id)]))
        self.assertEqual(len(sink.sent), 1, "submitted 的链接不该再发一次")

    def test_timeout_marks_uncertain_and_stops_automatic_retry(self):
        class FlakySink(FakeSink):
            def __init__(self):
                super().__init__()
                self.fail = sink_mod.SinkError("Lux 超时: 15s", uncertain=True)
                self.attempts = 0

            async def send(self, item):
                self.attempts += 1
                raise self.fail

        sink = FlakySink()
        watcher, store, config = self.build(sink)
        asyncio.run(watcher.process(self.message(), config.channels[0]))
        self.assertEqual(sink.attempts, 1)
        self.assertEqual(
            store.receipt("https://115.com/s/abc")["status"], store_mod.RECEIPT_UNCERTAIN
        )

        # 自动重试不应再发（结果未知），条目被挂起等人工确认。
        asyncio.run(watcher.retry_once())
        self.assertEqual(sink.attempts, 1, "uncertain 不该被自动重发")
        held = store.items_by_status((store_mod.STATUS_HELD,))
        self.assertTrue(any(item["reason"] == "receipt:uncertain" for item in held))

        # 人工确认后 --force 才允许再发。
        asyncio.run(watcher.deliver([int(held[0]["id"])], force=True))
        self.assertEqual(sink.attempts, 2)

    def test_connection_refused_is_retryable_not_uncertain(self):
        class DownSink(FakeSink):
            def __init__(self):
                super().__init__()
                self.attempts = 0

            async def send(self, item):
                self.attempts += 1
                raise sink_mod.SinkError("连接 Lux 失败: Connection refused")

        sink = DownSink()
        watcher, store, config = self.build(sink)
        asyncio.run(watcher.process(self.message(), config.channels[0]))
        self.assertEqual(
            store.receipt("https://115.com/s/abc")["status"], store_mod.RECEIPT_PLANNED
        )
        asyncio.run(watcher.retry_once())
        self.assertEqual(sink.attempts, 2, "没发出去的错误应该重试")


if __name__ == "__main__":
    unittest.main()
