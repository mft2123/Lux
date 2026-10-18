"""五种监听模式的判定逻辑（纯函数，不依赖 telethon / 网络）。

| 模式 | 行为 |
| --- | --- |
| 手动 | 只记录不投递；管理员触发时才处理 |
| 上新 | 新消息含链接即投递 |
| 追更 | 仅当链接标题命中 follow_list（剧集名）时投递 |
| 洗版 | 仅当链接体积大于库中同名的现有版本时投递（问 Lux 的 `/archive/lookup`） |
| 全量 | 所有链接投递 |

判定结果 `Decision.status` 只有三种：`deliver` / `hold` / `skip`。
- deliver：写入下游（sink）并把条目置为 delivered
- hold：留在库里等人工（status=held），**不写 sink**；信息不足时一律走这里
- skip：记录原因后丢弃（同一消息不会重复判定）
"""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass

MODE_MANUAL = "手动"
MODE_NEW = "上新"
MODE_FOLLOW = "追更"
MODE_WASH = "洗版"
MODE_FULL = "全量"

MODES = (MODE_MANUAL, MODE_NEW, MODE_FOLLOW, MODE_WASH, MODE_FULL)

MODE_ALIASES = {
    "manual": MODE_MANUAL,
    "手动": MODE_MANUAL,
    "new": MODE_NEW,
    "newest": MODE_NEW,
    "上新": MODE_NEW,
    "follow": MODE_FOLLOW,
    "追更": MODE_FOLLOW,
    "wash": MODE_WASH,
    "洗版": MODE_WASH,
    "full": MODE_FULL,
    "all": MODE_FULL,
    "全量": MODE_FULL,
}

STATUS_DELIVER = "deliver"
STATUS_HOLD = "hold"
STATUS_SKIP = "skip"


@dataclass(frozen=True)
class Decision:
    status: str
    reason: str

    @property
    def deliver(self) -> bool:
        return self.status == STATUS_DELIVER


DELIVER_NEW = Decision(STATUS_DELIVER, "new_message")
DELIVER_FULL = Decision(STATUS_DELIVER, "full_mode")
DELIVER_WASH = Decision(STATUS_DELIVER, "wash_bigger")
DELIVER_WASH_NEW = Decision(STATUS_DELIVER, "wash_new")
HOLD_MANUAL = Decision(STATUS_HOLD, "manual_mode")
HOLD_NO_LINK = Decision(STATUS_HOLD, "no_link")
HOLD_SIZE_UNKNOWN = Decision(STATUS_HOLD, "size_unknown")
HOLD_LIBRARY_UNKNOWN = Decision(STATUS_HOLD, "library_size_unknown")
SKIP_FOLLOW_MISS = Decision(STATUS_SKIP, "follow_miss")
SKIP_DUPLICATE = Decision(STATUS_SKIP, "wash_not_bigger")


class ModeError(ValueError):
    """配置里写了未知的监听模式。"""


def normalize_mode(value: str | None, default: str = MODE_MANUAL) -> str:
    if value is None or not str(value).strip():
        return default
    key = str(value).strip().lower()
    if key in MODE_ALIASES:
        return MODE_ALIASES[key]
    raise ModeError(f"未知监听模式: {value!r}（可用: {'/'.join(MODES)}）")


def normalize_text(value: str | None) -> str:
    """追更匹配前的归一化：全角转半角、小写、去掉空白与标点。"""
    if not value:
        return ""
    normalized = unicodedata.normalize("NFKC", str(value)).lower()
    return re.sub(r"[\s\W_]+", "", normalized, flags=re.UNICODE)


_RE_CACHE: dict[str, re.Pattern[str]] = {}


def _regex_cached(pattern: str) -> re.Pattern[str]:
    compiled = _RE_CACHE.get(pattern)
    if compiled is None:
        compiled = re.compile(pattern, re.IGNORECASE)
        _RE_CACHE[pattern] = compiled
    return compiled


def match_follow(title: str | None, follow_list: list[str] | tuple[str, ...]) -> str | None:
    """标题命中哪一条 follow_list，未命中返回 None。

    普通字符串按「归一化后包含」匹配；写成 `/.../` 的按正则匹配。
    只匹配**链接标题**（隐藏链接的锚文本，或正文首行），不扫整段正文——
    正文里提到剧名但链接指向别的资源时不该误投。
    """
    normalized_title = normalize_text(title)
    if not normalized_title:
        return None
    for entry in follow_list or ():
        candidate = str(entry).strip()
        if not candidate:
            continue
        if len(candidate) > 2 and candidate.startswith("/") and candidate.endswith("/"):
            try:
                if _regex_cached(candidate[1:-1]).search(title or ""):
                    return candidate
            except re.error:
                continue
            continue
        needle = normalize_text(candidate)
        if needle and needle in normalized_title:
            return candidate
    return None


def decide(
    mode: str,
    *,
    title: str | None = None,
    follow_list: list[str] | tuple[str, ...] = (),
    size_bytes: int | None = None,
    library_size: int | None = None,
    library_known: bool = True,
    wash_tolerance: float = 0.02,
    link_count: int = 1,
) -> Decision:
    """模式判定。

    洗版要三件事都成立才敢投：消息里能读出体积、Lux 查询成功、体积确实更大。
    任何一项缺失就 `hold`——宁可等人，也不猜着转存。

    `library_known=False` 表示查询本身失败（网络/鉴权/接口不存在）；
    `library_size=None` 且 `library_known=True` 表示库里**没有**同名版本（新增）。
    """
    normalized = normalize_mode(mode)
    if link_count <= 0:
        return HOLD_NO_LINK

    if normalized == MODE_MANUAL:
        return HOLD_MANUAL
    if normalized == MODE_FULL:
        return DELIVER_FULL
    if normalized == MODE_NEW:
        return DELIVER_NEW
    if normalized == MODE_FOLLOW:
        return DELIVER_NEW if match_follow(title, follow_list) else SKIP_FOLLOW_MISS

    # MODE_WASH
    if size_bytes is None:
        return HOLD_SIZE_UNKNOWN
    if not library_known:
        return HOLD_LIBRARY_UNKNOWN
    if library_size is None:
        return DELIVER_WASH_NEW
    tolerance = max(0.0, min(1.0, float(wash_tolerance)))
    if int(size_bytes) > int(library_size) * (1.0 + tolerance):
        return DELIVER_WASH
    return SKIP_DUPLICATE
