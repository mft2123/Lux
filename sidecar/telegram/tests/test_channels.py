import asyncio
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tgwatch import runner


class _Entity:
    def __init__(self, ident, username=""):
        self.id = ident
        self.username = username


class _Dialog:
    def __init__(self, title, is_channel=True, entity=None):
        self.title = title
        self.is_channel = is_channel
        self.entity = entity


class _Client:
    def __init__(self, dialogs):
        self._dialogs = dialogs
        self.disconnected = False

    async def connect(self):
        return None

    async def get_dialogs(self):
        return self._dialogs

    async def disconnect(self):
        self.disconnected = True


class ListChannelsTests(unittest.TestCase):
    """「刷新频道列表」的契约：只给频道、字段稳定、断开连接。"""

    def _config(self, directory):
        session = Path(directory) / "lux-telegram.session"
        session.write_bytes(b"")
        return mock.Mock(session_path=session)

    def test_returns_only_channels_with_stable_fields(self):
        dialogs = [
            _Dialog("爱影热更频道", True, _Entity(2769844611)),
            _Dialog("普通群", False, _Entity(1, "agroup")),
            _Dialog("多多资源库", True, _Entity(1903194870, "iduosou")),
            _Dialog("没有实体", True, None),
        ]
        client = _Client(dialogs)
        with tempfile.TemporaryDirectory() as directory:
            channels = asyncio.run(
                runner.list_channels(
                    self._config(directory), client_factory=lambda _c: client, timeout=5
                )
            )
        self.assertEqual([c["title"] for c in channels], ["爱影热更频道", "多多资源库"])
        self.assertEqual(channels[0]["telegramId"], "2769844611")
        self.assertEqual(channels[0]["username"], "")
        self.assertEqual(channels[1]["username"], "iduosou")
        self.assertTrue(client.disconnected)

    def test_missing_session_is_a_clear_error(self):
        config = mock.Mock(session_path=Path("/nonexistent/lux.session"))
        with self.assertRaises(runner.RunnerError) as caught:
            asyncio.run(runner.list_channels(config, client_factory=lambda _c: None))
        self.assertIn("session 不存在", str(caught.exception))


if __name__ == "__main__":
    unittest.main()