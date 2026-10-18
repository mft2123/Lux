"""日志保密：把 api_hash / phone / 2FA 密码 / ingest token 从所有日志里擦掉。

验收要求「日志不含 api_hash/phone 明文」。做法不是靠调用点自觉，而是在
handler 上加一层 Filter：任何日志行（含 telethon 自己打的）在落盘前都被替换。

同时也供 `masked()` 之外的临时打印用（例如异常里带上了 URL）。
"""

from __future__ import annotations

import logging
import re

_PLACEHOLDER = "****"

# 手机号：带 + 前缀的 8–15 位数字，可能夹空格/短横线
_PHONE_RE = re.compile(r"\+\d[\d\s\-()]{6,20}\d")
# 一次性验证码：Telegram 登录时 telethon 会打印 "Please enter the code ..."
_CODE_RE = re.compile(r"(?i)(code[^\d]{0,20})(\d{5,6})")


class SecretRedactor:
    """持有明文字典，把出现位置替换成 `****`；长串优先，避免截断残留。"""

    def __init__(self, secrets: list[str] | tuple[str, ...] | None = None):
        self._secrets: list[str] = []
        for secret in secrets or ():
            self.add(secret)

    def add(self, secret: str | None) -> None:
        if not secret:
            return
        value = str(secret)
        if len(value) < 4:
            return
        if value not in self._secrets:
            self._secrets.append(value)
            self._secrets.sort(key=len, reverse=True)

    def redact(self, text: str) -> str:
        if not text:
            return text
        result = text
        for secret in self._secrets:
            if secret in result:
                result = result.replace(secret, _PLACEHOLDER)
        result = _PHONE_RE.sub(_PLACEHOLDER, result)
        result = _CODE_RE.sub(lambda m: f"{m.group(1)}{_PLACEHOLDER}", result)
        return result

    def __call__(self, text: str) -> str:  # 便于直接当函数传
        return self.redact(text)


class RedactFilter(logging.Filter):
    """挂到 root handler 上；返回 True 表示照常输出（内容已就地改写）。"""

    def __init__(self, redactor: SecretRedactor):
        super().__init__()
        self.redactor = redactor

    def filter(self, record: logging.LogRecord) -> bool:
        try:
            message = record.getMessage()
        except Exception:  # pragma: no cover - 格式化失败时不要吞日志
            return True
        record.msg = self.redactor.redact(message)
        record.args = ()
        if record.exc_info:
            record.exc_text = self.redactor.redact(logging.Formatter().formatException(record.exc_info))
        if record.stack_info:
            record.stack_info = self.redactor.redact(record.stack_info)
        return True


def configure(level: str = "INFO", secrets: list[str] | None = None, quiet_libraries: bool = True) -> SecretRedactor:
    """初始化 root logger：stdout、带时间戳、全量脱敏。"""
    redactor = SecretRedactor(secrets)
    root = logging.getLogger()
    root.handlers.clear()
    handler = logging.StreamHandler()
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)-5s %(name)s: %(message)s", "%Y-%m-%d %H:%M:%S"))
    handler.addFilter(RedactFilter(redactor))
    root.addHandler(handler)
    root.setLevel(getattr(logging, level.upper(), logging.INFO))
    if quiet_libraries:
        # telethon 默认 INFO 会把每条 update 都打出来，太吵；错误仍然可见
        logging.getLogger("telethon").setLevel(logging.WARNING)
    return redactor
