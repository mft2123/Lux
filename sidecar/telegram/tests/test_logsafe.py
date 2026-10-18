import logging
import unittest

from tgwatch import logsafe

API_HASH = "aa11bb22cc33dd44ee55ff66aa77bb88"
PHONE = "+10000000000"
TOKEN = "supersecret-ingest-token"


class RedactorTest(unittest.TestCase):
    def setUp(self):
        self.redactor = logsafe.SecretRedactor([API_HASH, PHONE, TOKEN])

    def test_hash_removed(self):
        text = f"用的是 api_hash={API_HASH} 别忘了"
        out = self.redactor.redact(text)
        self.assertNotIn(API_HASH, out)
        self.assertNotIn(API_HASH[:8], out)
        self.assertIn("****", out)

    def test_phone_removed_even_without_plus(self):
        digits = "".join(ch for ch in PHONE if ch.isdigit())
        redactor = logsafe.SecretRedactor([API_HASH, PHONE, TOKEN, digits])
        out = redactor.redact(f"phone={digits}")
        self.assertNotIn(digits, out)

    def test_token_removed(self):
        self.assertNotIn(TOKEN, self.redactor.redact(f"Authorization: Bearer {TOKEN}"))

    def test_unknown_phone_pattern_removed(self):
        out = self.redactor.redact("另一个号码 +8613900112233 也别露")
        self.assertNotIn("13900112233", out)

    def test_login_code_removed(self):
        out = self.redactor.redact("Please enter the code you received: 54321")
        self.assertNotIn("54321", out)

    def test_normal_text_untouched(self):
        text = "投递 3 条链接，大小 24G，时间 2026-10-18 12:00:00"
        self.assertEqual(self.redactor.redact(text), text)

    def test_short_secret_ignored(self):
        redactor = logsafe.SecretRedactor(["ab"])
        self.assertEqual(redactor.redact("abc"), "abc")


class FilterTest(unittest.TestCase):
    def test_filter_rewrites_record(self):
        redactor = logsafe.SecretRedactor([API_HASH])
        record = logging.LogRecord("t", logging.INFO, __file__, 1, "hash is %s", (API_HASH,), None)
        logsafe.RedactFilter(redactor).filter(record)
        self.assertNotIn(API_HASH, record.getMessage())
        self.assertEqual(record.args, ())

    def test_filter_redacts_exception_text(self):
        redactor = logsafe.SecretRedactor([TOKEN])
        try:
            raise RuntimeError(f"失败 token={TOKEN}")
        except RuntimeError:
            import sys

            record = logging.LogRecord("t", logging.ERROR, __file__, 1, "boom", (), sys.exc_info())
        logsafe.RedactFilter(redactor).filter(record)
        self.assertIn("****", record.exc_text)
        self.assertNotIn(TOKEN, record.exc_text)


if __name__ == "__main__":
    unittest.main()
