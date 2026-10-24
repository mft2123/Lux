"""判定接口客户端：解析、候选拼装、错误路径（全部离线）。"""

import json
import unittest
import urllib.error

from tgwatch import planner


class CandidateTests(unittest.TestCase):
    def test_quality_is_taken_from_the_title(self):
        self.assertEqual(planner.parse_quality("繁花 2160p WEB-DL"), "2160p")
        self.assertEqual(planner.parse_quality("Some.Movie.4K.HDR"), "4k")
        self.assertEqual(planner.parse_quality("老片 1080P"), "1080p")
        self.assertEqual(planner.parse_quality("没有标注"), "")

    def test_episode_forms(self):
        self.assertEqual(planner.parse_episode("繁花 S01E03 4K"), (1, 3))
        self.assertEqual(planner.parse_episode("繁花 S1.E3"), (1, 3))
        self.assertEqual(planner.parse_episode("某剧 第12集"), (None, 12))
        self.assertEqual(planner.parse_episode("整包无集号"), (None, None))

    def test_candidate_omits_missing_fields(self):
        item = planner.candidate_for("https://115.com/s/x", "繁花 S01E03 4K", 2_000_000_000)
        self.assertEqual(item["title"], "繁花 S01E03 4K")
        self.assertEqual(item["link"], "https://115.com/s/x")
        self.assertEqual(item["season"], 1)
        self.assertEqual(item["episode"], 3)
        self.assertEqual(item["quality"], "4k")
        self.assertEqual(item["sizeBytes"], 2_000_000_000)

        bare = planner.candidate_for("https://115.com/s/y", "无名")
        self.assertNotIn("season", bare)
        self.assertNotIn("episode", bare)
        self.assertNotIn("quality", bare)
        self.assertNotIn("sizeBytes", bare)


class PlanTests(unittest.TestCase):
    def plan_with(self, payload, decisions=None):
        seen = {}

        def transport(body):
            seen["body"] = json.loads(body.decode("utf-8"))
            return payload

        client = planner.Planner("http://lux/api", "key", transport=transport)
        self.addCleanup(lambda: None)
        result = client.plan("-100", decisions if decisions is not None else [{"title": "繁花", "link": "u"}])
        return result, seen

    def test_decisions_are_keyed_by_link(self):
        result, seen = self.plan_with(
            {
                "decisions": [
                    {"link": "u", "action": "follow", "reason": "缺 S01E03"},
                    {"link": "v", "action": "skip", "reason": "版本不比它差"},
                ]
            }
        )
        self.assertTrue(result["u"].deliver)
        self.assertFalse(result["v"].deliver)
        self.assertEqual(result["u"].reason, "缺 S01E03")
        self.assertEqual(seen["body"]["channelId"], "-100")
        self.assertEqual(seen["body"]["messages"][0]["candidates"][0]["link"], "u")

    def test_unknown_action_is_not_delivered(self):
        result, _ = self.plan_with({"decisions": [{"link": "u", "action": "weird"}]})
        self.assertFalse(result["u"].deliver)

    def test_empty_candidates_make_no_request(self):
        called = []

        def transport(body):
            called.append(body)
            return {}

        client = planner.Planner("http://lux/api", transport=transport)
        self.assertEqual(client.plan("-100", []), {})
        self.assertEqual(called, [])

    def test_http_error_becomes_planner_error(self):
        def transport(body):
            raise planner.PlannerError("Lux 判定接口 HTTP 502")

        client = planner.Planner("http://lux/api", transport=transport)
        with self.assertRaises(planner.PlannerError):
            client.plan("-100", [{"title": "x", "link": "u"}])

    def test_real_transport_maps_http_errors(self):
        # 不走网络：把 urlopen 换成抛 HTTPError 的桩，验证异常被翻译成 PlannerError。
        import unittest.mock as mock

        error = urllib.error.HTTPError("http://lux/api", 401, "unauthorized", {}, None)
        client = planner.Planner("http://lux/api", "key")
        with mock.patch("urllib.request.urlopen", side_effect=error):
            with self.assertRaises(planner.PlannerError) as caught:
                client.plan("-100", [{"title": "x", "link": "u"}])
        self.assertIn("401", str(caught.exception))

    def test_real_transport_rejects_non_json(self):
        import unittest.mock as mock

        class Response:
            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

            def read(self):
                return b"<html>nginx</html>"

        client = planner.Planner("http://lux/api", "key")
        with mock.patch("urllib.request.urlopen", return_value=Response()):
            with self.assertRaises(planner.PlannerError) as caught:
                client.plan("-100", [{"title": "x", "link": "u"}])
        self.assertIn("JSON", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
