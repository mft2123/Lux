import unittest

from tgwatch import links


class Utf16Test(unittest.TestCase):
    def test_emoji_offsets_are_utf16_units(self):
        text = "🎬标题"
        # emoji 占 2 个 UTF-16 码元，所以锚文本从 offset=2 开始
        self.assertEqual(links.utf16_slice(text, 2, 2), "标题")
        self.assertEqual(links.utf16_len("🎬标题"), 4)

    def test_negative_and_out_of_range(self):
        self.assertEqual(links.utf16_slice("abc", -1, 2), "")
        self.assertEqual(links.utf16_slice("abc", 99, 2), "")


class ExtractTest(unittest.TestCase):
    def test_hidden_link_from_entity(self):
        text = "某剧 第二季 4K"
        entities = [{"type": "MessageEntityTextUrl", "offset": 0, "length": 9, "url": "https://115.com/s/abcd"}]
        found = links.extract_links(text, entities)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].url, "https://115.com/s/abcd")
        self.assertEqual(found[0].kind, links.KIND_115)
        self.assertEqual(found[0].anchor, "某剧 第二季 4K")
        self.assertEqual(found[0].source, "entity")

    def test_plain_url_in_text(self):
        found = links.extract_links("下载 https://115.com/s/xyz。")
        self.assertEqual([link.url for link in found], ["https://115.com/s/xyz"])
        self.assertEqual(found[0].source, "text")

    def test_entity_and_text_do_not_duplicate(self):
        text = "https://115.com/s/xyz"
        entities = [{"type": "MessageEntityUrl", "offset": 0, "length": 21, "url": "https://115.com/s/xyz"}]
        self.assertEqual(len(links.extract_links(text, entities)), 1)

    def test_magnet_trailing_punctuation_stripped(self):
        found = links.extract_links("magnet:?xt=urn:btih:abc123），")
        self.assertEqual(found[0].kind, links.KIND_MAGNET)
        self.assertFalse(found[0].url.endswith("）"))

    def test_telegram_link_classified(self):
        found = links.extract_links("见 https://t.me/otherchannel/12")
        self.assertEqual(found[0].kind, links.KIND_TELEGRAM)

    def test_bare_scheme_rejected(self):
        self.assertEqual(links.extract_links("https://"), [])

    def test_split_kinds_filters_telegram_links(self):
        text = "https://t.me/x/1 https://115.com/s/y"
        found = links.extract_links(text)
        kept = links.split_kinds(found, links.DEFAULT_ALLOWED_KINDS)
        self.assertEqual([link.kind for link in kept], [links.KIND_115])

    def test_default_kinds_drop_other_and_telegram(self):
        """review SC5：默认只放行已知网盘/磁力，other 要显式开关。"""
        self.assertNotIn(links.KIND_OTHER, links.DEFAULT_ALLOWED_KINDS)
        self.assertNotIn(links.KIND_TELEGRAM, links.DEFAULT_ALLOWED_KINDS)
        for kind in (links.KIND_115, links.KIND_MAGNET, links.KIND_ED2K,
                     links.KIND_ALIYUN, links.KIND_QUARK, links.KIND_BAIDU):
            self.assertIn(kind, links.DEFAULT_ALLOWED_KINDS)

    def test_news_link_is_other_and_dropped_by_default(self):
        found = links.extract_links("新片资讯 https://example.com/news/2026/10 快看")
        self.assertEqual([link.kind for link in found], [links.KIND_OTHER])
        self.assertEqual(links.split_kinds(found, links.DEFAULT_ALLOWED_KINDS), [])
        kept = links.split_kinds(found, (*links.DEFAULT_ALLOWED_KINDS, links.KIND_OTHER))
        self.assertEqual([link.kind for link in kept], [links.KIND_OTHER])


class HtmlEntityTest(unittest.TestCase):
    """review SC15：正文里的 `&amp;` 必须还原成 `&`，否则 Lux 拿到的 query 是错的。"""

    def test_amp_in_plain_link_is_unescaped(self):
        found = links.extract_links("下载 https://115.com/s/abc?p=1&amp;q=2")
        self.assertEqual([link.url for link in found], ["https://115.com/s/abc?p=1&q=2"])

    def test_amp_in_hidden_link_is_unescaped(self):
        entities = [
            {
                "type": "MessageEntityTextUrl",
                "offset": 0,
                "length": 2,
                "url": "https://115.com/s/abc?a=1&amp;b=2",
            }
        ]
        found = links.extract_links("某剧", entities)
        self.assertEqual(found[0].url, "https://115.com/s/abc?a=1&b=2")

    def test_numeric_entity_is_unescaped(self):
        found = links.extract_links("https://pan.quark.cn/s/1?x=1&#38;y=2")
        self.assertEqual(found[0].url, "https://pan.quark.cn/s/1?x=1&y=2")

    def test_semicolonless_query_is_untouched(self):
        # `&not=` / `&copy=` 是正常 query 参数，不能被 html.unescape 当成实体吃掉
        found = links.extract_links("https://115.com/s/abc?not=1&copy=2")
        self.assertEqual(found[0].url, "https://115.com/s/abc?not=1&copy=2")

    def test_escaped_and_plain_forms_dedupe_to_one_link(self):
        text = "https://115.com/s/abc?a=1&amp;b=2 和 https://115.com/s/abc?a=1&b=2"
        self.assertEqual(len(links.extract_links(text)), 1)


class ClassifyTest(unittest.TestCase):
    def test_host_boundary(self):
        self.assertEqual(links.classify("https://115.com/s/1"), links.KIND_115)
        self.assertEqual(links.classify("https://115cdn.com/s/1"), links.KIND_115)
        self.assertEqual(links.classify("https://evil-115.com/s/1"), links.KIND_OTHER)
        self.assertEqual(links.classify("https://pan.quark.cn/s/1"), links.KIND_QUARK)
        self.assertEqual(links.classify("ed2k://|file|x|1|hash|/"), links.KIND_ED2K)


class SizeTest(unittest.TestCase):
    def test_various_units(self):
        self.assertEqual(links.parse_size("大小 2.5G"), int(2.5 * 1024**3))
        self.assertEqual(links.parse_size("8.3GiB"), int(8.3 * 1024**3))
        self.assertEqual(links.parse_size("700 MB"), 700 * 1024**2)

    def test_takes_max(self):
        text = "单集 1.2GB，全集 24G"
        self.assertEqual(links.parse_size(text), 24 * 1024**3)

    def test_none_when_absent(self):
        self.assertIsNone(links.parse_size("没有体积"))
        self.assertIsNone(links.parse_size(None))

    def test_quality_tag_is_not_a_size(self):
        # 「4K / 8K」是分辨率，「8M」是码率，都不能当体积（review F7）
        self.assertIsNone(links.parse_size("某剧 第二季 4K HDR"))
        self.assertIsNone(links.parse_size("8K 修复版"))
        self.assertIsNone(links.parse_size("S01E01 4K"))
        self.assertIsNone(links.parse_size("码率 8M"))
        # 真体积仍然取得出来
        self.assertEqual(links.parse_size("4K 完整版 12G"), 12 * 1024**3)


class TitleTest(unittest.TestCase):
    def test_prefers_hidden_link_anchor(self):
        found = links.extract_links(
            "正文首行\n某剧.S02.2160p",
            [{"type": "MessageEntityTextUrl", "offset": 5, "length": 18, "url": "https://115.com/s/1"}],
        )
        self.assertEqual(links.title_from("正文首行\n某剧.S02.2160p", found), "某剧.S02.2160p")

    def test_falls_back_to_first_line(self):
        self.assertEqual(links.title_from("\n  沙丘2 2024  \n更多"), "沙丘2 2024")

    def test_empty(self):
        self.assertEqual(links.title_from(None), "")


class SlugTest(unittest.TestCase):
    def test_slug(self):
        self.assertEqual(links.slugify("沙丘 2 (2024)"), "沙丘-2-2024")
        self.assertEqual(links.slugify("!!!"), "link")


if __name__ == "__main__":
    unittest.main()
