"""部署片段的静态检查：不装 PyYAML，只断言关键行存在。

review SC2：设置文件是 0640 + 属组，容器必须以附加组加入同一个 gid，否则 uid 10001
读不到 `/config/archive-settings.json`，启动即 exit 2。
"""

import unittest
from pathlib import Path

MODULE_ROOT = Path(__file__).resolve().parent.parent
COMPOSE = MODULE_ROOT / "compose.fragment.yml"


class ComposeFragmentTest(unittest.TestCase):
    def setUp(self):
        self.text = COMPOSE.read_text(encoding="utf-8")

    def test_group_add_follows_the_settings_group(self):
        self.assertIn("group_add:", self.text)
        self.assertIn("${LUX_ARCHIVE_SETTINGS_GROUP:-10001}", self.text)

    def test_still_runs_as_uid_10001(self):
        self.assertIn('user: "10001:10001"', self.text)

    def test_settings_file_is_mounted_read_only(self):
        self.assertIn("/config/archive-settings.json:ro", self.text)

    def test_deployment_checklist_is_mentioned(self):
        self.assertIn("部署检查清单", self.text)


if __name__ == "__main__":
    unittest.main()
