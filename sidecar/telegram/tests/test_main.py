"""`main.py status` 的输出检查：只允许出现打码后的 URL / key（review SC8 / D7）。"""

import contextlib
import io
import os
import unittest
from unittest.mock import patch

import main as main_mod

from tests import support

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


if __name__ == "__main__":
    unittest.main()
