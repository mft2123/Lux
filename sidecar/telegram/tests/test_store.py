import unittest
from pathlib import Path

from tests import support
from tgwatch import store as store_mod


def item_kwargs(**overrides):
    base = dict(
        msg_key="1:10",
        channel="@chan",
        msg_id=10,
        msg_date="2026-10-18T00:00:00+00:00",
        url="https://115.com/s/abc",
        kind="115",
        anchor="某剧 S01",
        title="某剧 S01",
        size_bytes=1024,
        mode="上新",
        status=store_mod.STATUS_PENDING,
    )
    base.update(overrides)
    return base


class StoreTest(unittest.TestCase):
    def setUp(self):
        self.dir = support.mkdtemp()
        self.store = store_mod.Store(self.dir / "state.db")

    def tearDown(self):
        self.store.close()
        support.cleanup(self.dir)

    def test_add_item_dedupes_by_message_and_url(self):
        first = self.store.add_item(**item_kwargs())
        self.assertIsNotNone(first)
        self.assertIsNone(self.store.add_item(**item_kwargs()))
        # 同一消息的不同链接可以各占一条
        self.assertIsNotNone(self.store.add_item(**item_kwargs(url="https://115.com/s/def")))

    def test_record_message_is_idempotent(self):
        kwargs = dict(
            msg_key="1:10",
            channel="@chan",
            msg_id=10,
            msg_date="",
            mode="上新",
            link_count=1,
            status=store_mod.STATUS_PENDING,
            reason="new_message",
        )
        self.assertTrue(self.store.record_message(**kwargs))
        self.assertFalse(self.store.record_message(**kwargs))
        self.assertTrue(self.store.seen("1:10"))

    def test_status_transitions_and_delivered_at(self):
        item_id = self.store.add_item(**item_kwargs())
        self.store.set_status(item_id, store_mod.STATUS_FAILED, "boom", bump_attempts=True)
        item = self.store.get_item(item_id)
        self.assertEqual(item["status"], store_mod.STATUS_FAILED)
        self.assertEqual(item["attempts"], 1)
        self.assertIsNone(item["delivered_at"])

        self.store.set_status(item_id, store_mod.STATUS_DELIVERED)
        item = self.store.get_item(item_id)
        self.assertIsNotNone(item["delivered_at"])
        self.assertEqual(item["reason"], "boom")  # COALESCE 保留旧原因

    def test_items_by_status_filter_and_order(self):
        held = self.store.add_item(**item_kwargs(status=store_mod.STATUS_HELD))
        self.store.add_item(**item_kwargs(url="https://115.com/s/x", status=store_mod.STATUS_DELIVERED))
        rows = self.store.items_by_status((store_mod.STATUS_HELD,))
        self.assertEqual([row["id"] for row in rows], [held])
        self.assertEqual(self.store.items_by_status((store_mod.STATUS_HELD,), channel="@other"), [])

    def test_promote_only_moves_retryable(self):
        held = self.store.add_item(**item_kwargs(status=store_mod.STATUS_HELD))
        delivered = self.store.add_item(**item_kwargs(url="https://115.com/s/y", status=store_mod.STATUS_DELIVERED))
        changed = self.store.promote([held, delivered])
        self.assertEqual(changed, 1)
        self.assertEqual(self.store.get_item(held)["status"], store_mod.STATUS_PENDING)
        self.assertEqual(self.store.get_item(delivered)["status"], store_mod.STATUS_DELIVERED)

    def test_promote_can_resurrect_dead_explicitly(self):
        """死信默认不再自动重试，但 `replay --id` 必须能人工放行（review SC4）。"""
        dead = self.store.add_item(
            **item_kwargs(status=store_mod.STATUS_DEAD, reason="Lux 永久拒绝（400，不再重试）")
        )
        self.assertEqual(self.store.promote([dead]), 1)
        self.assertEqual(self.store.get_item(dead)["status"], store_mod.STATUS_PENDING)
        self.assertEqual(self.store.get_item(dead)["reason"], "released")

    def test_stats_expose_dead_count_and_reasons(self):
        self.store.add_item(**item_kwargs(status=store_mod.STATUS_DEAD, reason="400 forever"))
        self.store.add_item(
            **item_kwargs(url="https://115.com/s/q", status=store_mod.STATUS_DEAD, reason="403 forever")
        )
        stats = self.store.stats()
        self.assertEqual(stats["dead"], 2)
        self.assertEqual(stats["dead_reasons"], {"400 forever": 1, "403 forever": 1})
        self.assertEqual(stats["by_status"][store_mod.STATUS_DEAD], 2)

    def test_stats(self):
        self.store.add_item(**item_kwargs(status=store_mod.STATUS_HELD))
        self.store.add_item(**item_kwargs(url="https://115.com/s/z", status=store_mod.STATUS_HELD))
        self.store.record_message(
            msg_key="1:10", channel="@chan", msg_id=10, msg_date="", mode="上新", link_count=2,
            status=store_mod.STATUS_HELD,
        )
        stats = self.store.stats()
        self.assertEqual(stats["messages"], 1)
        self.assertEqual(stats["items"], 2)
        self.assertEqual(stats["by_status"][store_mod.STATUS_HELD], 2)

    def test_reopen_keeps_data(self):
        path = self.dir / "state.db"
        self.store.add_item(**item_kwargs())
        self.store.close()
        reopened = store_mod.Store(path)
        try:
            self.assertEqual(reopened.stats()["items"], 1)
        finally:
            reopened.close()
        self.store = store_mod.Store(path)  # tearDown 用

    # ---- 频道游标 ----

    def test_cursor_starts_at_zero(self):
        self.assertEqual(self.store.last_message_id("@chan"), 0)

    def test_cursor_only_advances(self):
        self.assertEqual(self.store.advance_message_id("@chan", 10), 10)
        self.assertEqual(self.store.advance_message_id("@chan", 7), 10)
        self.assertEqual(self.store.advance_message_id("@chan", 11), 11)
        self.assertEqual(self.store.last_message_id("@chan"), 11)

    def test_cursor_is_per_channel(self):
        self.store.advance_message_id("@a", 5)
        self.store.advance_message_id("@b", 9)
        self.assertEqual(self.store.last_message_id("@a"), 5)
        self.assertEqual(self.store.last_message_id("@b"), 9)

    def test_cursor_ignores_non_positive_ids(self):
        self.store.advance_message_id("@chan", 4)
        self.assertEqual(self.store.advance_message_id("@chan", 0), 4)
        self.assertEqual(self.store.advance_message_id("@chan", -3), 4)


if __name__ == "__main__":
    unittest.main()
