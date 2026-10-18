"""依赖锁定的静态检查（review SC10）。

requirements.txt 每一行都必须是 `name==version`，不接受范围、未钉版本或行尾注释；
telethon 与它跑起来会碰到的关键传递依赖（pyaes / rsa / python-socks）必须在里面。
"""

import re
import unittest
from pathlib import Path

MODULE_ROOT = Path(__file__).resolve().parent.parent
REQUIREMENTS = MODULE_ROOT / "requirements.txt"

# name==version：拒绝 ~= / >= / 通配、拒绝空版本
_PINNED = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*==[0-9][0-9A-Za-z.!+_-]*$")

# telethon 本身 + 它的硬依赖（pyaes、rsa）+ 可选代理后端（python-socks）
REQUIRED = ("telethon", "pyaes", "rsa", "python-socks")


def _lines(text: str) -> list[str]:
    return [line.strip() for line in text.splitlines() if line.strip() and not line.strip().startswith("#")]


def _names(text: str) -> list[str]:
    return [line.split("==", 1)[0].lower().replace("_", "-") for line in _lines(text)]


class RequirementsLockTest(unittest.TestCase):
    def setUp(self):
        self.text = REQUIREMENTS.read_text(encoding="utf-8")
        self.lines = _lines(self.text)

    def test_every_line_is_pinned_name_equals_version(self):
        self.assertTrue(self.lines, "requirements.txt 不能只剩注释")
        for line in self.lines:
            with self.subTest(line=line):
                self.assertRegex(line, _PINNED)

    def test_no_line_has_a_trailing_comment(self):
        # 行尾注释会让人以为「版本后面还能加东西」，也绕开上面的锚定检查
        for line in self.lines:
            with self.subTest(line=line):
                self.assertNotIn("#", line)

    def test_telethon_and_key_transitive_deps_are_pinned(self):
        names = _names(self.text)
        for name in REQUIRED:
            with self.subTest(name=name):
                self.assertIn(name, names)

    def test_no_duplicate_pins(self):
        names = _names(self.text)
        self.assertEqual(len(names), len(set(names)), f"重复钉了同一个包: {names}")

    def test_version_source_and_upgrade_path_are_documented(self):
        # 版本来源与升级方法必须写在文件里，否则下一个人只能猜
        self.assertIn("tgvenv", self.text)
        self.assertIn("升级方法", self.text)


if __name__ == "__main__":
    unittest.main()
