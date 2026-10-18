import asyncio
import io
import json
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from tests import support
from tgwatch import sink as sink_mod


def sample_item(**overrides):
    base = {
        "id": 7,
        "url": "https://115.com/s/abc",
        "kind": "115",
        "title": "某剧 S01",
        "anchor": "",
        "size_bytes": 1024,
        "mode": "上新",
        "channel": "@chan",
        "msg_id": 42,
        "msg_date": "2026-10-18T00:00:00+00:00",
    }
    base.update(overrides)
    return base


class PayloadTest(unittest.TestCase):
    def test_http_payload_is_documented_contract(self):
        # Lux LinkJob：url + title + mode + 溯源字段（review SC6）
        self.assertEqual(
            sink_mod.payload(sample_item()),
            {
                "url": "https://115.com/s/abc",
                "title": "某剧 S01",
                "mode": "上新",
                "source": "telegram",
                "kind": "115",
                "channel": "@chan",
                "message_id": 42,
                "size_bytes": 1024,
                "captured_at": "2026-10-18T00:00:00+00:00",
            },
        )

    def test_traceability_fields_are_all_present(self):
        body = sink_mod.payload(sample_item())
        for key in ("source", "kind", "channel", "message_id", "size_bytes", "captured_at"):
            self.assertIn(key, body)
            self.assertNotIn(body[key], (None, ""), f"{key} 不能为空")

    def test_unknown_size_is_null_and_message_id_is_int(self):
        body = sink_mod.payload(sample_item(size_bytes=None, msg_id="77"))
        self.assertIsNone(body["size_bytes"])
        self.assertEqual(body["message_id"], 77)

    def test_source_can_be_overridden_by_the_item(self):
        self.assertEqual(sink_mod.payload(sample_item(source="replay"))["source"], "replay")

    def test_title_falls_back_to_anchor(self):
        self.assertEqual(sink_mod.payload(sample_item(title="", anchor="锚文本"))["title"], "锚文本")


class InboxFilenameTest(unittest.TestCase):
    def test_pattern_is_ts_slug(self):
        name = sink_mod.inbox_filename(sample_item(), now=1792000000)
        self.assertRegex(name, r"^\d{8}-\d{6}-.+\.txt$")
        self.assertTrue(name.endswith(".txt"))

    def test_slug_from_title(self):
        name = sink_mod.inbox_filename(sample_item(title="Dune Part Two 2024"), now=1792000000)
        self.assertIn("dune-part-two-2024", name)

    def test_empty_title_falls_back(self):
        name = sink_mod.inbox_filename(sample_item(title=""), now=1792000000)
        self.assertRegex(name, r"^\d{8}-\d{6}-link\.txt$")

    def test_chinese_title_kept(self):
        name = sink_mod.inbox_filename(sample_item(title="沙丘 2"), now=1792000000)
        self.assertIn("沙丘-2", name)


class InboxSinkTest(unittest.TestCase):
    def setUp(self):
        self.dir = support.mkdtemp()
        self.addCleanup(support.cleanup, self.dir)

    def test_writes_one_file_per_item(self):
        sink = sink_mod.InboxSink(directory=self.dir)
        asyncio.run(sink.send(sample_item()))
        asyncio.run(sink.send(sample_item(url="https://115.com/s/def")))
        files = sorted(self.dir.glob("*.txt"))
        self.assertEqual(len(files), 2)
        first_line = files[0].read_text(encoding="utf-8").splitlines()[0]
        self.assertTrue(first_line.startswith("https://115.com/s/"))

    def test_body_first_line_is_url_then_comments(self):
        sink = sink_mod.InboxSink(directory=self.dir)
        asyncio.run(sink.send(sample_item()))
        lines = next(self.dir.glob("*.txt")).read_text(encoding="utf-8").splitlines()
        self.assertEqual(lines[0], "https://115.com/s/abc")
        self.assertTrue(all(line.startswith("#") or not line for line in lines[1:]))
        self.assertIn("mode=上新", lines[1])

    def test_same_second_same_title_does_not_overwrite(self):
        sink = sink_mod.InboxSink(directory=self.dir)
        for _ in range(3):
            asyncio.run(sink.send(sample_item()))
        files = sorted(self.dir.glob("*.txt"))
        self.assertEqual(len(files), 3)
        urls = {path.read_text(encoding="utf-8").splitlines()[0] for path in files}
        self.assertEqual(urls, {"https://115.com/s/abc"})
        self.assertTrue(any(__import__("re").search(r"-\d+\.txt$", path.name) for path in files))

    def test_no_temp_files_left_behind(self):
        sink = sink_mod.InboxSink(directory=self.dir)
        asyncio.run(sink.send(sample_item()))
        self.assertEqual(list(self.dir.glob(".*.tmp")), [])

    def test_file_is_readable_by_other_users(self):
        import os
        import stat

        sink = sink_mod.InboxSink(directory=self.dir)
        asyncio.run(sink.send(sample_item()))
        target = next(self.dir.glob("*.txt"))
        mode = stat.S_IMODE(target.stat().st_mode)
        if os.name == "posix":
            self.assertTrue(mode & 0o044, f"mode={oct(mode)}")

    def test_write_failure_raises_sink_error(self):
        sink = sink_mod.InboxSink(directory=self.dir / "missing")

        def boom(item):
            raise OSError("disk full")

        sink._write = boom  # type: ignore[method-assign]
        with self.assertRaises(sink_mod.SinkError):
            asyncio.run(sink.send(sample_item()))


class BuildTest(unittest.TestCase):
    def setUp(self):
        self.dir = support.mkdtemp()
        self.addCleanup(support.cleanup, self.dir)

    def config(self, **overrides):
        base = dict(
            dry_run=False,
            sink="http",
            inbox_dir=self.dir,
            ingest_url="http://lux:8097/api/v1/admin/archive/ingest",
            lookup_url="http://lux:8097/api/v1/admin/archive/lookup",
            api_key="test-key",
            http_timeout=5.0,
        )
        base.update(overrides)
        return SimpleNamespace(**base)

    def test_http_is_default(self):
        sink = sink_mod.build(self.config())
        self.assertIsInstance(sink, sink_mod.HttpSink)
        self.assertTrue(sink.describe()["authorized"])

    def test_inbox_opt_in(self):
        self.assertIsInstance(sink_mod.build(self.config(sink="inbox")), sink_mod.InboxSink)

    def test_dry_run_is_null(self):
        self.assertIsInstance(sink_mod.build(self.config(dry_run=True)), sink_mod.NullSink)


class HttpSinkTest(unittest.TestCase):
    def test_http_error_becomes_sink_error(self):
        sink = sink_mod.HttpSink(url="http://lux:8097/ingest")

        def boom(*args, **kwargs):
            raise urllib.error.HTTPError("http://lux:8097/ingest", 500, "boom", {}, io.BytesIO(b'{"e":1}'))

        with patch.object(urllib.request, "urlopen", boom):
            with self.assertRaises(sink_mod.SinkError) as ctx:
                asyncio.run(sink.send(sample_item()))
        self.assertIn("500", str(ctx.exception))

    def test_401_mentions_api_key(self):
        sink = sink_mod.HttpSink(url="http://lux:8097/ingest", api_key="bad")

        def boom(*args, **kwargs):
            raise urllib.error.HTTPError("http://lux:8097/ingest", 401, "no", {}, io.BytesIO(b"{}"))

        with patch.object(urllib.request, "urlopen", boom):
            with self.assertRaises(sink_mod.SinkError) as ctx:
                asyncio.run(sink.send(sample_item()))
        self.assertIn("api_key", str(ctx.exception))

    def test_connection_error_becomes_sink_error(self):
        sink = sink_mod.HttpSink(url="http://lux:8097/ingest")

        def boom(*args, **kwargs):
            raise urllib.error.URLError("refused")

        with patch.object(urllib.request, "urlopen", boom):
            with self.assertRaises(sink_mod.SinkError) as ctx:
                asyncio.run(sink.send(sample_item()))
        # 网络错误可重试
        self.assertFalse(ctx.exception.permanent)

    def test_400_and_403_are_permanent(self):
        """review SC4：Lux 的 400/403 重试无意义，必须标成永久失败。"""
        for code in (400, 403):
            sink = sink_mod.HttpSink(url="http://lux:8097/ingest", api_key="k")

            def boom(*args, _code=code, **kwargs):
                raise urllib.error.HTTPError(
                    "http://lux:8097/ingest", _code, "denied", {}, io.BytesIO(b'{"error":"x"}')
                )

            with patch.object(urllib.request, "urlopen", boom):
                with self.assertRaises(sink_mod.SinkError) as ctx:
                    asyncio.run(sink.send(sample_item()))
            self.assertTrue(ctx.exception.permanent, f"{code} 应当是永久失败")
            self.assertIn(str(code), str(ctx.exception))
            self.assertIn("永久", str(ctx.exception))

    def test_5xx_is_retryable(self):
        sink = sink_mod.HttpSink(url="http://lux:8097/ingest", api_key="k")

        def boom(*args, **kwargs):
            raise urllib.error.HTTPError("http://lux:8097/ingest", 503, "down", {}, io.BytesIO(b""))

        with patch.object(urllib.request, "urlopen", boom):
            with self.assertRaises(sink_mod.SinkError) as ctx:
                asyncio.run(sink.send(sample_item()))
        self.assertFalse(ctx.exception.permanent)

    def test_success_posts_full_body_with_api_key_header(self):
        captured = {}

        class Response:
            status = 202

            def read(self, size=None):
                return b'{"ok":true,"jobId":"1-x","duplicate":false}'

            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

        def fake_urlopen(request, timeout=None):
            captured["url"] = request.full_url
            captured["body"] = json.loads(request.data.decode("utf-8"))
            captured["api_key"] = request.get_header("X-lux-api-key")
            return Response()

        sink = sink_mod.HttpSink(url="http://lux:8097/api/v1/admin/archive/ingest", api_key="test-key")
        with patch.object(urllib.request, "urlopen", fake_urlopen):
            asyncio.run(sink.send(sample_item()))
        self.assertEqual(captured["url"], "http://lux:8097/api/v1/admin/archive/ingest")
        self.assertEqual(captured["api_key"], "test-key")
        self.assertEqual(captured["body"], sink_mod.payload(sample_item()))
        # 线上 body 必须带齐溯源字段（review SC6）
        self.assertEqual(captured["body"]["channel"], "@chan")
        self.assertEqual(captured["body"]["message_id"], 42)


class LookupTest(unittest.TestCase):
    def test_reads_size_bytes(self):
        class Response:
            def read(self, size=None):
                return b'{"title":"x","matched":"x","size_bytes":123}'

            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

        captured = {}

        def fake_urlopen(request, timeout=None):
            captured["url"] = request.full_url
            captured["api_key"] = request.get_header("X-lux-api-key")
            return Response()

        with patch.object(urllib.request, "urlopen", fake_urlopen):
            result = sink_mod.lookup_size("http://lux:8097/api/v1/admin/archive/lookup", "k", "某剧 S01")
        self.assertTrue(result["ok"])
        self.assertEqual(result["size_bytes"], 123)
        self.assertIn("title=%E6%9F%90%E5%89%A7", captured["url"])
        self.assertEqual(captured["api_key"], "k")

    def test_no_library_copy_is_ok_with_none(self):
        class Response:
            def read(self, size=None):
                return b'{"title":"x","matched":null,"size_bytes":null}'

            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

        with patch.object(urllib.request, "urlopen", lambda *a, **k: Response()):
            result = sink_mod.lookup_size("http://lux/lookup", "k", "某剧")
        self.assertTrue(result["ok"])
        self.assertIsNone(result["size_bytes"])

    def test_http_error_is_not_ok(self):
        def boom(*args, **kwargs):
            raise urllib.error.HTTPError("http://lux/lookup", 404, "no", {}, io.BytesIO(b"{}"))

        with patch.object(urllib.request, "urlopen", boom):
            result = sink_mod.lookup_size("http://lux/lookup", "k", "某剧")
        self.assertFalse(result["ok"])

    def test_empty_title_or_url_is_not_ok(self):
        self.assertFalse(sink_mod.lookup_size("http://lux/lookup", "k", "")["ok"])
        self.assertFalse(sink_mod.lookup_size("", "k", "某剧")["ok"])


class SecretTest(unittest.TestCase):
    def test_describe_never_leaks_api_key(self):
        sink = sink_mod.HttpSink(url="http://lux/ingest", api_key="very-secret")
        described = json.dumps(sink.describe())
        self.assertNotIn("very-secret", described)

    def test_describe_masks_ingest_url(self):
        """review SC8：status 打印的 ingest_url 只留 scheme://host（D7）。"""
        sink = sink_mod.HttpSink(
            url="http://lux:8097/api/v1/admin/archive/ingest?token=deadbeef", api_key="k"
        )
        described = sink.describe()
        self.assertEqual(described["url"], "http://lux:8097/…")
        blob = json.dumps(described)
        self.assertNotIn("deadbeef", blob)
        self.assertNotIn("/api/", blob)
        self.assertTrue(described["authorized"])


if __name__ == "__main__":
    unittest.main()
