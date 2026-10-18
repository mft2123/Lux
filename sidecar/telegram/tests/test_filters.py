import unittest

from tgwatch import filters


class ModeTest(unittest.TestCase):
    def test_aliases(self):
        self.assertEqual(filters.normalize_mode("new"), filters.MODE_NEW)
        self.assertEqual(filters.normalize_mode("追更"), filters.MODE_FOLLOW)
        self.assertEqual(filters.normalize_mode("ALL"), filters.MODE_FULL)
        self.assertEqual(filters.normalize_mode(""), filters.MODE_MANUAL)

    def test_unknown_mode_raises(self):
        with self.assertRaises(filters.ModeError):
            filters.normalize_mode("随便")


class DecideTest(unittest.TestCase):
    def test_manual_holds(self):
        decision = filters.decide(filters.MODE_MANUAL, title="某剧")
        self.assertEqual(decision.status, filters.STATUS_HOLD)
        self.assertEqual(decision.reason, "manual_mode")
        self.assertFalse(decision.deliver)

    def test_new_and_full_deliver(self):
        self.assertTrue(filters.decide(filters.MODE_NEW).deliver)
        self.assertTrue(filters.decide(filters.MODE_FULL).deliver)

    def test_no_link_holds(self):
        self.assertEqual(filters.decide(filters.MODE_NEW, link_count=0).reason, "no_link")

    def test_follow_hit_and_miss(self):
        hit = filters.decide(filters.MODE_FOLLOW, title="【更新】某 剧 名 S02E03 4K", follow_list=["某剧名"])
        self.assertTrue(hit.deliver)
        miss = filters.decide(filters.MODE_FOLLOW, title="别的片子", follow_list=["某剧名"])
        self.assertEqual(miss.status, filters.STATUS_SKIP)
        self.assertEqual(miss.reason, "follow_miss")

    def test_follow_only_looks_at_title(self):
        self.assertEqual(
            filters.decide(filters.MODE_FOLLOW, title="沙丘 2 2024", follow_list=["某剧名"]).reason,
            "follow_miss",
        )

    def test_follow_regex_entry(self):
        self.assertEqual(
            filters.decide(filters.MODE_FOLLOW, title="某剧 S03E01", follow_list=["/s0[12]e/"]).status,
            filters.STATUS_SKIP,
        )
        self.assertTrue(
            filters.decide(filters.MODE_FOLLOW, title="某剧 S01E01", follow_list=["/s0[12]e/"]).deliver
        )


class WashTest(unittest.TestCase):
    """洗版：体积已知 + 查询成功 + 确实更大，三者缺一就挂起。"""

    def test_bigger_is_delivered(self):
        decision = filters.decide(
            filters.MODE_WASH, size_bytes=20 * 1024**3, library_size=10 * 1024**3
        )
        self.assertTrue(decision.deliver)
        self.assertEqual(decision.reason, "wash_bigger")

    def test_within_tolerance_is_duplicate(self):
        # Lux 默认容差 2%：2% 以内算同一版本
        decision = filters.decide(
            filters.MODE_WASH,
            size_bytes=int(10 * 1024**3 * 1.01),
            library_size=10 * 1024**3,
            wash_tolerance=0.02,
        )
        self.assertEqual(decision.status, filters.STATUS_SKIP)
        self.assertEqual(decision.reason, "wash_not_bigger")

    def test_beyond_tolerance_is_wash(self):
        decision = filters.decide(
            filters.MODE_WASH,
            size_bytes=int(10 * 1024**3 * 1.05),
            library_size=10 * 1024**3,
            wash_tolerance=0.02,
        )
        self.assertTrue(decision.deliver)

    def test_no_library_copy_is_delivered(self):
        decision = filters.decide(
            filters.MODE_WASH, size_bytes=5 * 1024**3, library_size=None, library_known=True
        )
        self.assertTrue(decision.deliver)
        self.assertEqual(decision.reason, "wash_new")

    def test_unknown_size_holds(self):
        decision = filters.decide(filters.MODE_WASH, size_bytes=None, library_size=10)
        self.assertEqual(decision.status, filters.STATUS_HOLD)
        self.assertEqual(decision.reason, "size_unknown")

    def test_lookup_failure_holds(self):
        decision = filters.decide(
            filters.MODE_WASH, size_bytes=5 * 1024**3, library_size=None, library_known=False
        )
        self.assertEqual(decision.status, filters.STATUS_HOLD)
        self.assertEqual(decision.reason, "library_size_unknown")


class MatchTest(unittest.TestCase):
    def test_ignores_spaces_case_and_width(self):
        self.assertEqual(filters.match_follow("ＴＨＥ 剧集", ["the剧集"]), "the剧集")

    def test_empty_follow_list_or_title(self):
        self.assertIsNone(filters.match_follow("任意", []))
        self.assertIsNone(filters.match_follow("", ["某剧"]))

    def test_broken_regex_is_skipped(self):
        self.assertIsNone(filters.match_follow("任意", ["/[/"]))


if __name__ == "__main__":
    unittest.main()
