"""判定交给 Lux：`POST /api/v1/admin/archive/telegram/plan`。

旁挂只负责读消息、按结论执行、回写游标；"这部片库里有没有、缺哪集、要不要
洗版"全部由 Lux 用 catalog 判定（见 `src/application/telegram`）。把权重逻辑
留在一处，避免 Python 侧再抄一份、两边慢慢漂移。

Lux 不可达时**不猜**：调用方退回 `filters` 的保守规则（该挂起的仍然挂起），
坏掉的判定接口不能让旁挂开始乱转存。
"""

from __future__ import annotations

import json
import re
import urllib.error
import urllib.request
from dataclasses import dataclass

ACTION_NEW = "new_release"
ACTION_FOLLOW = "follow"
ACTION_REMUX = "remux"
ACTION_SKIP = "skip"

#: 只有这三种结论会真的去转存；`skip` 一律不投递。
DELIVER_ACTIONS = frozenset({ACTION_NEW, ACTION_FOLLOW, ACTION_REMUX})

_QUALITY = re.compile(r"(2160p?|4k|uhd|1080p?|720p?|480p?)", re.IGNORECASE)
_EPISODE = re.compile(r"[Ss](\d{1,3})[\s._-]?[Ee](\d{1,4})")
_CN_EPISODE = re.compile(r"第\s*(\d{1,4})\s*[集话]")


class PlannerError(RuntimeError):
    """Lux 判定接口不可用（网络、HTTP 错误或返回的不是 JSON）。"""


class CompletionClient:
    """补集清单：Lux 报「哪部剧缺哪几集」，旁挂负责去频道里搜。

    只读接口；拿到清单后要不要搜、搜到要不要转存，都是调用方的决定
    （`complete` 默认 dry-run）。
    """

    def __init__(self, url: str, api_key: str = "", timeout: float = 20.0, transport=None):
        self.url = url
        self.api_key = api_key
        self.timeout = timeout
        self._transport = transport or self._get_json

    def missing(self) -> dict:
        payload = self._transport()
        if not isinstance(payload, dict):
            raise PlannerError("补集接口返回的不是对象")
        return payload

    def _get_json(self) -> dict:
        headers = {}
        if self.api_key:
            headers["X-Lux-Api-Key"] = self.api_key
        request = urllib.request.Request(self.url, headers=headers, method="GET")
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:  # noqa: S310
                raw = response.read()
        except urllib.error.HTTPError as error:
            raise PlannerError(f"补集接口 HTTP {error.code}") from error
        except urllib.error.URLError as error:
            raise PlannerError(f"补集接口不可达：{error.reason}") from error
        try:
            payload = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, ValueError) as error:
            raise PlannerError("补集接口返回的不是 JSON") from error
        if not isinstance(payload, dict):
            raise PlannerError("补集接口返回的不是对象")
        return payload

@dataclass(frozen=True)
class Planned:
    action: str
    reason: str

    @property
    def deliver(self) -> bool:
        return self.action in DELIVER_ACTIONS


def parse_quality(title: str) -> str:
    """从标题里取清晰度标记，取不到返回空串（Lux 会当作"未标注"）。"""
    match = _QUALITY.search(title or "")
    return match.group(1).lower() if match else ""


def parse_episode(title: str) -> tuple[int | None, int | None]:
    """`S01E03` / `S1.E3` / `第3集` → `(季, 集)`；取不到的半边是 None。"""
    text = title or ""
    match = _EPISODE.search(text)
    if match:
        return int(match.group(1)), int(match.group(2))
    match = _CN_EPISODE.search(text)
    if match:
        return None, int(match.group(1))
    return None, None


def candidate_for(url: str, title: str, size_bytes: int | None = None) -> dict:
    """拼一个候选给 Lux；缺的字段不写，别用 0 冒充"有值"。"""
    season, episode = parse_episode(title)
    item: dict = {"title": (title or "").strip(), "link": url}
    if season is not None:
        item["season"] = season
    if episode is not None:
        item["episode"] = episode
    quality = parse_quality(title)
    if quality:
        item["quality"] = quality
    if size_bytes:
        item["sizeBytes"] = int(size_bytes)
    return item


class Planner:
    """一次判定调用 = 一条消息里的全部候选。"""

    def __init__(self, url: str, api_key: str = "", timeout: float = 20.0, transport=None):
        self.url = url
        self.api_key = api_key
        self.timeout = timeout
        self._transport = transport or self._post_json

    def plan(self, channel_id: str, candidates: list[dict]) -> dict[str, Planned]:
        """返回 `{链接: 结论}`；Lux 没提到的链接不在返回里（调用方按本地规则处理）。"""
        if not candidates:
            return {}
        body = json.dumps(
            {
                "channelId": channel_id,
                "messages": [{"messageId": 1, "candidates": candidates}],
            },
            ensure_ascii=False,
        ).encode("utf-8")
        payload = self._transport(body)
        decisions = payload.get("decisions") or []
        planned: dict[str, Planned] = {}
        for decision in decisions:
            link = str(decision.get("link") or "")
            if not link:
                continue
            planned[link] = Planned(
                action=str(decision.get("action") or ACTION_SKIP),
                reason=str(decision.get("reason") or ""),
            )
        return planned

    def _post_json(self, body: bytes) -> dict:
        headers = {"content-type": "application/json"}
        if self.api_key:
            headers["X-Lux-Api-Key"] = self.api_key
        request = urllib.request.Request(self.url, data=body, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:  # noqa: S310 - 自填 URL
                raw = response.read()
        except urllib.error.HTTPError as error:
            raise PlannerError(f"Lux 判定接口 HTTP {error.code}") from error
        except urllib.error.URLError as error:
            raise PlannerError(f"Lux 判定接口不可达：{error.reason}") from error
        try:
            payload = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, ValueError) as error:
            raise PlannerError("Lux 判定接口返回的不是 JSON") from error
        if not isinstance(payload, dict):
            raise PlannerError("Lux 判定接口返回的不是对象")
        return payload
