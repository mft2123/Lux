import json
import os
import unittest
from pathlib import Path
from unittest.mock import patch

from tests import support
from tgwatch import config as config_mod
from tgwatch import filters

# 测试里一律用明显的假值，绝不写真实账号
FAKE_API_ID = 123456
FAKE_PHONE = "+10000000000"
FAKE_API_HASH = "0000aaaa1111bbbb2222cccc3333dddd"
FAKE_API_KEY = "test-lux-api-key"

CLEAN_ENV = {
    key: value
    for key, value in os.environ.items()
    if not key.startswith("LUX_TG_")
    and key
    not in (
        "LUX_ARCHIVE_SETTINGS",
        "LUX_ARCHIVE_LOOKUP_URL",
        "TG_2FA_PASSWORD",
        "LUX_ARCHIVE_DRY_RUN_URL",
    )
}


def write_settings(directory: Path, payload: dict) -> Path:
    path = Path(directory) / "archive-settings.json"
    path.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    return path


BASE = {
    "wash_tolerance_ratio": 0.02,
    "telegram": {
        "api_id": str(FAKE_API_ID),
        "api_hash": FAKE_API_HASH,
        "phone": FAKE_PHONE,
        "api_key": FAKE_API_KEY,
        "listen_mode": "上新",
        "listen_channels": ["@chan_a", "-1001234567890"],
        "follow_list": ["某剧"],
    },
}


class LoadTest(unittest.TestCase):
    def setUp(self):
        self.dir = support.mkdtemp()

    def tearDown(self):
        support.cleanup(self.dir)

    def load(self, payload=None, env=None):
        path = write_settings(self.dir, payload or BASE)
        environment = dict(CLEAN_ENV)
        environment.update(env or {})
        with patch.dict(os.environ, environment, clear=True):
            return config_mod.load(path)

    def test_reads_from_settings_file(self):
        cfg = self.load()
        self.assertEqual(cfg.api_id, FAKE_API_ID)
        self.assertEqual(cfg.default_mode, filters.MODE_NEW)
        self.assertEqual([c.key() for c in cfg.channels], ["chan_a", "-1001234567890"])
        self.assertEqual(cfg.channels[1].mode, filters.MODE_NEW)
        self.assertEqual(cfg.follow_list, ("某剧",))
        self.assertEqual(cfg.api_key, FAKE_API_KEY)
        self.assertAlmostEqual(cfg.wash_tolerance, 0.02)

    def test_env_overrides_file(self):
        cfg = self.load(env={"LUX_TG_MODE": "追更", "LUX_TG_CHANNELS": "@other", "LUX_TG_API_ID": "1"})
        self.assertEqual(cfg.api_id, 1)
        self.assertEqual(cfg.default_mode, filters.MODE_FOLLOW)
        self.assertEqual([c.key() for c in cfg.channels], ["other"])

    def test_channel_objects_with_per_channel_mode(self):
        payload = {
            "telegram": {
                "api_id": 42,
                "api_hash": "hash-hash-hash",
                "api_key": FAKE_API_KEY,
                "listen_mode": "上新",
                "listen_channels": [
                    {"channel": "https://t.me/chan_a", "mode": "追更", "follow_list": ["剧A"]},
                    {"channel": "-100999"},
                ],
            }
        }
        cfg = self.load(payload)
        self.assertEqual(cfg.channels[0].key(), "chan_a")
        self.assertEqual(cfg.channels[0].mode, filters.MODE_FOLLOW)
        self.assertEqual(cfg.channels[0].follow_list, ("剧A",))
        self.assertEqual(cfg.channels[1].mode, filters.MODE_NEW)
        self.assertEqual(cfg.follow_for("chan_a"), ("剧A",))
        self.assertEqual(cfg.mode_for("chan_a"), filters.MODE_FOLLOW)
        self.assertEqual(cfg.mode_for("unknown"), filters.MODE_NEW)

    def test_missing_settings_file_falls_back_to_env(self):
        missing = self.dir / "nope.json"
        with patch.dict(
            os.environ,
            {
                **CLEAN_ENV,
                "LUX_TG_API_ID": "7",
                "LUX_TG_API_HASH": "hashhash",
                "LUX_TG_PHONE": "+1234",
                "LUX_TG_API_KEY": "k",
            },
            clear=True,
        ):
            cfg = config_mod.load(missing)
        self.assertEqual(cfg.api_id, 7)
        self.assertEqual(cfg.channels, ())

    def test_missing_api_id_raises(self):
        with self.assertRaises(config_mod.ConfigError):
            self.load({"telegram": {"api_hash": "x"}})

    def test_missing_api_hash_raises(self):
        with self.assertRaises(config_mod.ConfigError):
            self.load({"telegram": {"api_id": 1}})

    def test_bad_mode_raises(self):
        with self.assertRaises(config_mod.ConfigError):
            self.load({"telegram": {"api_id": 1, "api_hash": "h", "listen_mode": "乱写"}})

    def test_http_sink_requires_api_key(self):
        payload = {"telegram": {"api_id": 1, "api_hash": "h"}}
        with self.assertRaises(config_mod.ConfigError) as ctx:
            self.load(payload)
        self.assertIn("api_key", str(ctx.exception))

    def test_api_key_prefers_settings_file_over_env(self):
        """review SC1 / D1：设置文件里的 key 是权威值，env 不能盖掉它。"""
        cfg = self.load(env={"LUX_TG_API_KEY": "key-from-env"})
        self.assertEqual(cfg.api_key, FAKE_API_KEY)

    def test_api_key_falls_back_to_env(self):
        payload = {"telegram": {"api_id": 1, "api_hash": "h", "api_key": ""}}
        cfg = self.load(payload, env={"LUX_TG_API_KEY": "key-from-env"})
        self.assertEqual(cfg.api_key, "key-from-env")

    def test_missing_api_key_error_is_actionable(self):
        payload = {"telegram": {"api_id": 1, "api_hash": "h"}}
        with self.assertRaises(config_mod.ConfigError) as ctx:
            self.load(payload)
        message = str(ctx.exception)
        self.assertIn("telegram.api_key", message)
        self.assertIn("LUX_TG_API_KEY", message)
        self.assertIn("API key", message)

    def test_env_only_startup_works(self):
        """只给 env 也要能启动（SC1 验收的三种情况之一）。"""
        payload = {"telegram": {"api_id": 1, "api_hash": "h"}}
        cfg = self.load(
            payload,
            env={
                "LUX_TG_API_KEY": "key-from-env",
                "LUX_TG_CHANNELS": "@a",
            },
        )
        self.assertEqual(cfg.api_key, "key-from-env")
        self.assertNotIn("key-from-env", json.dumps(cfg.masked(), ensure_ascii=False))

    def test_settings_only_startup_works(self):
        cfg = self.load()  # BASE 里只有文件、没有 LUX_TG_API_KEY
        self.assertEqual(cfg.api_key, FAKE_API_KEY)
        self.assertNotIn(FAKE_API_KEY, json.dumps(cfg.masked(), ensure_ascii=False))

    def test_inbox_sink_does_not_require_api_key(self):
        payload = {"telegram": {"api_id": 1, "api_hash": "h", "api_key": ""}}
        cfg = self.load(payload, env={"LUX_TG_SINK": "inbox"})
        self.assertEqual(cfg.sink, config_mod.SINK_INBOX)

    def test_unknown_sink_raises(self):
        with self.assertRaises(config_mod.ConfigError):
            self.load(env={"LUX_TG_SINK": "spool"})

    def test_unknown_kind_raises(self):
        with self.assertRaises(config_mod.ConfigError):
            self.load(env={"LUX_TG_ALLOWED_KINDS": "115,网盘"})

    def test_other_is_not_allowed_by_default(self):
        """review SC5：默认白名单里没有 other。"""
        cfg = self.load()
        self.assertNotIn("other", cfg.allowed_kinds)
        self.assertIn("115", cfg.allowed_kinds)
        self.assertIn("magnet", cfg.allowed_kinds)

    def test_allow_other_switch_adds_other(self):
        cfg = self.load(env={"LUX_TG_ALLOW_OTHER": "1"})
        self.assertIn("other", cfg.allowed_kinds)
        # 显式开关只加 other，不影响其余默认项
        self.assertIn("115", cfg.allowed_kinds)
        self.assertNotIn("telegram", cfg.allowed_kinds)

    def test_allow_other_off_keeps_other_out(self):
        cfg = self.load(env={"LUX_TG_ALLOW_OTHER": "0"})
        self.assertNotIn("other", cfg.allowed_kinds)

    def test_explicit_whitelist_with_other(self):
        cfg = self.load(env={"LUX_TG_ALLOWED_KINDS": "115,other"})
        self.assertEqual(cfg.allowed_kinds, ("115", "other"))

    def test_bad_allow_other_value_raises(self):
        with self.assertRaises(config_mod.ConfigError):
            self.load(env={"LUX_TG_ALLOW_OTHER": "maybe"})

    def test_bad_bool_raises(self):
        with self.assertRaises(config_mod.ConfigError):
            self.load(env={"LUX_TG_DRY_RUN": "maybe"})

    def test_bad_wash_tolerance_raises(self):
        payload = {"wash_tolerance_ratio": 3, "telegram": {"api_id": 1, "api_hash": "h", "api_key": "k"}}
        with self.assertRaises(config_mod.ConfigError):
            self.load(payload)

    def test_two_factor_from_documented_env_name(self):
        cfg = self.load(env={"TG_2FA_PASSWORD": "hunter2hunter2"})
        self.assertEqual(cfg.password, "hunter2hunter2")
        self.assertTrue(cfg.masked()["two_factor"])
        self.assertNotIn("hunter2hunter2", json.dumps(cfg.masked(), ensure_ascii=False))

    def test_default_dirs_follow_brief(self):
        cfg = self.load()
        self.assertEqual(cfg.session_dir, config_mod.MODULE_ROOT / "session")
        self.assertEqual(cfg.session_path, config_mod.MODULE_ROOT / "session" / "lux-telegram.session")
        self.assertEqual(cfg.state_dir, config_mod.MODULE_ROOT / "state")
        self.assertEqual(cfg.inbox_dir, Path("/watch-inbox"))
        # 默认走 Lux ingest（review F1 要求打通投递链路）
        self.assertEqual(cfg.sink, config_mod.SINK_HTTP)
        self.assertEqual(cfg.ingest_url, "http://lux:8097/api/v1/admin/archive/ingest")
        self.assertEqual(cfg.lookup_url, "http://lux:8097/api/v1/admin/archive/lookup")

    def test_paths_and_session_name(self):
        cfg = self.load(
            env={
                "LUX_TG_SESSION_DIR": str(self.dir / "s"),
                "LUX_TG_STATE_DIR": str(self.dir / "st"),
                "LUX_TG_INBOX_DIR": str(self.dir / "inbox"),
                "LUX_TG_SESSION_NAME": "acct",
                "LUX_TG_INGEST_URL": "http://lux.local/ingest",
            }
        )
        self.assertEqual(cfg.session_path, self.dir / "s" / "acct.session")
        self.assertEqual(cfg.state_db, self.dir / "st" / "state.db")
        self.assertEqual(cfg.inbox_dir, self.dir / "inbox")
        self.assertEqual(cfg.ingest_url, "http://lux.local/ingest")


class SettingsPermissionTest(unittest.TestCase):
    """review SC2：设置文件读不到时要给可操作的属组/权限提示，不能笼统报 ConfigError。"""

    def setUp(self):
        self.dir = support.mkdtemp()
        self.addCleanup(support.cleanup, self.dir)

    def test_permission_error_message_is_actionable(self):
        path = write_settings(self.dir, BASE)
        with patch.dict(os.environ, CLEAN_ENV, clear=True), patch.object(
            Path, "read_text", side_effect=PermissionError(13, "Permission denied")
        ):
            with self.assertRaises(config_mod.ConfigError) as ctx:
                config_mod.load(path)
        message = str(ctx.exception)
        self.assertIn("设置文件不可读", message)
        self.assertIn("部署检查清单", message)
        self.assertIn("LUX_ARCHIVE_SETTINGS_GROUP", message)
        self.assertIn(str(path), message)
        self.assertIn("Permission denied", message)

    @unittest.skipUnless(os.name == "posix", "需要 POSIX 权限位")
    def test_unreadable_file_on_disk_is_actionable(self):
        if not support.permissions_enforced(self.dir):
            self.skipTest("文件系统不校验 chmod（root 或忽略权限位的挂载点）")
        path = write_settings(self.dir, BASE)
        os.chmod(path, 0o000)
        self.addCleanup(os.chmod, path, 0o600)
        with patch.dict(os.environ, CLEAN_ENV, clear=True):
            with self.assertRaises(config_mod.ConfigError) as ctx:
                config_mod.load(path)
        self.assertIn("设置文件不可读", str(ctx.exception))

    def test_missing_file_still_falls_back_to_env(self):
        # 文件不存在 ≠ 权限错误：仍然走 env 兜底，不报「不可读」
        with patch.dict(
            os.environ,
            {**CLEAN_ENV, "LUX_TG_API_ID": "7", "LUX_TG_API_HASH": "h", "LUX_TG_API_KEY": "k"},
            clear=True,
        ):
            cfg = config_mod.load(self.dir / "missing.json")
        self.assertEqual(cfg.api_id, 7)

    def test_broken_json_is_reported_as_json_error(self):
        path = self.dir / "archive-settings.json"
        path.write_text("{not json", encoding="utf-8")
        with patch.dict(os.environ, CLEAN_ENV, clear=True):
            with self.assertRaises(config_mod.ConfigError) as ctx:
                config_mod.load(path)
        self.assertIn("JSON", str(ctx.exception))


class MaskTest(unittest.TestCase):
    def test_masked_hides_secrets(self):
        with support.temp_dir() as tmp:
            path = write_settings(tmp, BASE)
            with patch.dict(os.environ, CLEAN_ENV, clear=True):
                cfg = config_mod.load(path)
        blob = json.dumps(cfg.masked(), ensure_ascii=False)
        self.assertNotIn(cfg.api_hash, blob)
        self.assertNotIn(FAKE_PHONE, blob)
        self.assertNotIn(FAKE_API_KEY, blob)
        self.assertNotIn(cfg.api_hash[-6:], blob)

    def test_urls_masked(self):
        cfg = config_mod.SidecarConfig(
            api_id=1,
            api_hash="h" * 16,
            api_key="secret-key",
            ingest_url="https://lux.local/api/v1/admin/archive/ingest?token=deadbeef",
        )
        blob = json.dumps(cfg.masked(), ensure_ascii=False)
        self.assertNotIn("deadbeef", blob)
        self.assertNotIn("secret-key", blob)
        self.assertIn("lux.local", blob)


class ProblemTest(unittest.TestCase):
    def test_problems_when_nothing_configured(self):
        with support.temp_dir() as tmp:
            path = write_settings(
                tmp,
                {"telegram": {"api_id": 1, "api_hash": "hhhh", "listen_mode": "手动", "listen_channels": []}},
            )
            with patch.dict(
                os.environ,
                {
                    **CLEAN_ENV,
                    "LUX_TG_SESSION_DIR": str(tmp / "session"),
                    "LUX_TG_API_KEY": "k",
                },
                clear=True,
            ):
                cfg = config_mod.load(path)
            problems = " ".join(config_mod.runtime_problems(cfg))
            warnings = " ".join(config_mod.runtime_warnings(cfg))
        # 空频道是警告而不是硬错误：设置页要先能列出频道才有得勾选（见 config.runtime_problems
        # 的注释），这里断言的是当前约定而不是旧行为。
        self.assertIn("listen_channels 为空", warnings)
        self.assertIn("session 不存在", problems)
        self.assertIn("手动", warnings)

    def test_no_problems_when_ready(self):
        with support.temp_dir() as tmp:
            session_dir = tmp / "session"
            session_dir.mkdir()
            (session_dir / "lux-telegram.session").write_text("x", encoding="utf-8")
            path = write_settings(
                tmp,
                {"telegram": {"api_id": 1, "api_hash": "hhhh", "api_key": "k", "listen_channels": ["@a"]}},
            )
            with patch.dict(
                os.environ,
                {**CLEAN_ENV, "LUX_TG_SESSION_DIR": str(session_dir)},
                clear=True,
            ):
                cfg = config_mod.load(path)
            self.assertEqual(config_mod.runtime_problems(cfg), [])


if __name__ == "__main__":
    unittest.main()
