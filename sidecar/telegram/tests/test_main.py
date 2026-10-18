"""`main.py status` 的输出检查：只允许出现打码后的 URL / key（review SC8 / D7）。"""

import contextlib
import io
import os
import unittest
from unittest.mock import patch

import main as main_mod

from tests import support
from tgwatch import runner

FAKE_ENV = {
    "LUX_TG_API_ID": "123456",
    "LUX_TG_API_HASH": "0000aaaa1111bbbb2222cccc3333dddd",
    "LUX_TG_API_KEY": "test-lux-api-key",
    "LUX_TG_CHANNELS": "@chan",
    "LUX_TG_INGEST_URL": "http://lux:8097/api/v1/admin/archive/ingest?token=deadbeef",
    "LUX_ARCHIVE_LOOKUP_URL": "http://lux:8097/api/v1/admin/archive/lookup?token=cafebabe",
}


class StatusCommandTest(unittest.TestCase):
    def run_status(self, extra=None):
        with support.temp_dir() as workspace:
            env = {
                **FAKE_ENV,
                "LUX_ARCHIVE_SETTINGS": str(workspace / "missing.json"),
                "LUX_TG_SESSION_DIR": str(workspace / "session"),
                "LUX_TG_STATE_DIR": str(workspace / "state"),
                **(extra or {}),
            }
            buffer = io.StringIO()
            with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(buffer):
                code = main_mod.main(["status"])
            return code, buffer.getvalue()

    def test_status_masks_urls_and_api_key(self):
        code, output = self.run_status()
        self.assertEqual(code, 0)
        self.assertNotIn("deadbeef", output)
        self.assertNotIn("cafebabe", output)
        self.assertNotIn("test-lux-api-key", output)
        self.assertIn("http://lux:8097/…", output)

    def test_status_reports_dead_letters(self):
        code, output = self.run_status()
        self.assertEqual(code, 0)
        # 空库也要把 dead 计数与原因结构打出来，运维才有据可查
        self.assertIn('"dead": 0', output)


class DeepCheckTest(unittest.TestCase):
    """review SC9：`check --deep` 用假 client 覆盖授权 / 未授权 / 连不上三种结果。

    不联网：真的 TelegramClient 被 `runner._new_client` 替换成假 client。
    """

    class FakeClient:
        def __init__(self, authorized: bool = True, error: Exception | None = None):
            self.authorized = authorized
            self.error = error
            self.connected = False
            self.disconnected = False

        async def connect(self):
            if self.error is not None:
                raise self.error
            self.connected = True

        async def is_user_authorized(self):
            return self.authorized

        async def disconnect(self):
            self.disconnected = True

    def run_cli(self, *argv, client=None, session: bool = True):
        with support.temp_dir() as workspace:
            session_dir = workspace / "session"
            session_dir.mkdir(parents=True, exist_ok=True)
            if session:
                (session_dir / "lux-telegram.session").write_bytes(b"x")
            env = {
                **FAKE_ENV,
                "LUX_ARCHIVE_SETTINGS": str(workspace / "missing.json"),
                "LUX_TG_SESSION_DIR": str(session_dir),
                "LUX_TG_STATE_DIR": str(workspace / "state"),
            }
            fake = client if client is not None else self.FakeClient()
            buffer = io.StringIO()
            with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(buffer):
                with patch.object(runner, "_new_client", lambda config: fake):
                    code = main_mod.main(list(argv))
            return code, buffer.getvalue(), fake

    def test_authorized_session_exits_zero(self):
        client = self.FakeClient(authorized=True)
        code, output, _ = self.run_cli("check", "--deep", client=client)
        self.assertEqual(code, 0)
        self.assertIn("✅", output)
        self.assertIn("is_user_authorized=True", output)
        self.assertTrue(client.connected)
        self.assertTrue(client.disconnected)

    def test_unauthorized_session_exits_nonzero(self):
        code, output, client = self.run_cli("check", "--deep", client=self.FakeClient(authorized=False))
        self.assertEqual(code, 1)
        self.assertIn("未登录", output)
        self.assertTrue(client.connected)
        self.assertTrue(client.disconnected)

    def test_connection_failure_exits_nonzero(self):
        client = self.FakeClient(error=ConnectionError("network unreachable"))
        code, output, _ = self.run_cli("check", "--deep", client=client)
        self.assertEqual(code, 1)
        self.assertIn("连接 Telegram 失败", output)
        self.assertIn("network unreachable", output)
        self.assertTrue(client.disconnected)

    def test_missing_session_exits_nonzero(self):
        code, output, client = self.run_cli("check", "--deep", session=False)
        self.assertEqual(code, 1)
        self.assertIn("session 不存在", output)
        self.assertFalse(client.connected)

    def test_plain_check_never_touches_the_network(self):
        """`check` 只验配置就绪：连 client 都不造（会话过期时它仍然会通过，见 README）。"""

        def forbidden(config):
            raise AssertionError("check 不该构造 TelegramClient")

        with support.temp_dir() as workspace:
            session_dir = workspace / "session"
            session_dir.mkdir(parents=True, exist_ok=True)
            (session_dir / "lux-telegram.session").write_bytes(b"x")
            env = {
                **FAKE_ENV,
                "LUX_ARCHIVE_SETTINGS": str(workspace / "missing.json"),
                "LUX_TG_SESSION_DIR": str(session_dir),
                "LUX_TG_STATE_DIR": str(workspace / "state"),
            }
            buffer = io.StringIO()
            with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(buffer):
                with patch.object(runner, "_new_client", forbidden):
                    code = main_mod.main(["check"])
        self.assertEqual(code, 0)
        self.assertIn("配置可用", buffer.getvalue())


if __name__ == "__main__":
    unittest.main()
