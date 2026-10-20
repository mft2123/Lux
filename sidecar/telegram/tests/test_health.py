import unittest
from unittest import mock

from tgwatch import health


class ProbeTests(unittest.TestCase):
    """The probe is what Lux's 测试连接 button calls, so its four answers matter."""

    def setUp(self):
        self.config = object()

    def test_missing_configuration_is_unconfigured(self):
        with mock.patch.object(
            health.config_mod,
            "runtime_problems",
            return_value=["缺少 API Hash", "缺少手机号"],
        ):
            status, body = health.probe(self.config, deep=False)
        self.assertEqual(status, 503)
        self.assertEqual(body["status"], "unconfigured")
        self.assertIn("API Hash", body["detail"])

    def test_shallow_probe_does_not_touch_telegram(self):
        with mock.patch.object(health.config_mod, "runtime_problems", return_value=[]), mock.patch.object(
            health.runner, "check_session"
        ) as check:
            status, body = health.probe(self.config, deep=False)
        check.assert_not_called()
        self.assertEqual((status, body["status"]), (200, "configured"))

    def test_deep_probe_reports_a_live_session(self):
        async def fake_check(config):
            return True, "session 已登录（is_user_authorized=True）"

        with mock.patch.object(health.config_mod, "runtime_problems", return_value=[]), mock.patch.object(
            health.runner, "check_session", fake_check
        ):
            status, body = health.probe(self.config, deep=True)
        self.assertEqual((status, body["status"]), (200, "logged_in"))
        self.assertIn("is_user_authorized", body["detail"])

    def test_deep_probe_reports_a_dead_session(self):
        async def fake_check(config):
            return False, "session 未登录或已失效"

        with mock.patch.object(health.config_mod, "runtime_problems", return_value=[]), mock.patch.object(
            health.runner, "check_session", fake_check
        ):
            status, body = health.probe(self.config, deep=True)
        self.assertEqual((status, body["status"]), (503, "auth_required"))

    def test_probe_never_raises(self):
        async def exploding_check(config):
            raise RuntimeError("network down")

        with mock.patch.object(health.config_mod, "runtime_problems", return_value=[]), mock.patch.object(
            health.runner, "check_session", exploding_check
        ):
            status, body = health.probe(self.config, deep=True)
        self.assertEqual(status, 503)
        self.assertEqual(body["status"], "failed")
        self.assertIn("network down", body["detail"])


if __name__ == "__main__":
    unittest.main()
