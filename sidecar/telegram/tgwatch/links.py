"""链接提取与归一化（纯逻辑，不依赖 telethon）。

Telegram 频道里的资源链接有三种常见形态：

1. 明链：正文里直接写 `https://115.com/s/xxxx`
2. 隐藏链接：正文是标题，链接挂在 `MessageEntityTextUrl` 上（抓不到就会漏掉大半资源）
3. magnet / ed2k 明文

对外接口只吃「纯数据」：`text: str` + `entities: list[dict]`。runner 负责把
telethon 的 entity 对象转成 dict，这样本模块的测试不需要装 telethon。

entity dict 约定：
    {"type": "MessageEntityTextUrl" | "MessageEntityUrl" | "MessageEntityEmail", 
     "offset": int, "length": int, "url": str | None}

offset/length 与 Telegram 一致，单位是 **UTF-16 码元**，不是 Python 字符。
"""

from __future__ import annotations

import html
import re
import unicodedata
import urllib.parse
from dataclasses import dataclass

# 资源链接的 kind 分类；`allowed_kinds` 配置用这些名字
KIND_115 = "115"
KIND_MAGNET = "magnet"
KIND_ED2K = "ed2k"
KIND_ALIYUN = "aliyun"
KIND_QUARK = "quark"
KIND_BAIDU = "baidu"
KIND_TELEGRAM = "telegram"
KIND_OTHER = "other"

ALL_KINDS = (
    KIND_115,
    KIND_MAGNET,
    KIND_ED2K,
    KIND_ALIYUN,
    KIND_QUARK,
    KIND_BAIDU,
    KIND_TELEGRAM,
    KIND_OTHER,
)

# 默认只放行真正的资源链接（网盘/磁力）：
# - 频道自己引用的 t.me 链接默认丢弃（KIND_TELEGRAM 不在默认集合里）；
# - 新闻/网页等 `other` 也默认丢弃，要收必须显式 `LUX_TG_ALLOW_OTHER=1`（review SC5）。
DEFAULT_ALLOWED_KINDS = (
    KIND_115,
    KIND_MAGNET,
    KIND_ED2K,
    KIND_ALIYUN,
    KIND_QUARK,
    KIND_BAIDU,
)

_HOST_KINDS = (
    (KIND_115, ("115.com", "115cdn.com", "anxia.com", "115vod.com")),
    (KIND_ALIYUN, ("aliyundrive.com", "alipan.com")),
    (KIND_QUARK, ("pan.quark.cn", "quark.cn")),
    (KIND_BAIDU, ("pan.baidu.com", "yun.baidu.com")),
    (KIND_TELEGRAM, ("t.me", "telegram.me", "telegram.dog")),
)

_URL_RE = re.compile(
    # ASCII 的 []{} 必须排除：频道常用 `[文字](链接)` 的 Markdown，不排除就会把
    # 锚文本和目标链接连成一串（`url](url`，2026-10-25 线上踩到，17 条挂起全是这个）。
    # 圆括号 `()` 反而要保留：ed2k 文件名里常有 `(2026)`、`(tmdbid=…)`，
    # 排掉会把链接从第一个 `(` 处截断。结尾多出来的 `)` 由 `_TRAILING_JUNK` 剥掉。
    r"(?:https?://|magnet:\?|ed2k://)[^\s<>\"'\[\]{}，。、；！？（）〔〕【】《》「」\u3000]+",
    re.IGNORECASE,
)

# ed2k 的完整形态：ed2k://|file|<名称>|<字节数>|<32 位十六进制 hash>|/
# 缺 size/hash 的半截链接转不了，Lux 会以 400「ed2k 链接缺少 hash」拒收。
_ED2K_COMPLETE = re.compile(r"^ed2k://\|file\|.+\|\d+\|[0-9a-fA-F]{32}\|/$", re.IGNORECASE)
# 行内判断「这个 ed2k 已经完整」用：`|/` 后面还可能跟着正文，不能整行锚定。
_ED2K_HAS_HASH = re.compile(r"\|\d+\|[0-9a-fA-F]{32}\|/")

# 链接自带的作品名：ed2k 的 `|file|<名称>|`、magnet 的 `dn=` 参数。
_ED2K_NAME = re.compile(r"^ed2k://\|file\|([^|]+)\|", re.IGNORECASE)
_MAGNET_NAME = re.compile(r"[?&]dn=([^&]+)", re.IGNORECASE)

# 链接结尾常被正文标点粘住，需要剥掉
_TRAILING_JUNK = "。，、；：！？）】》」』\"'”’,.;:!?)]}>~*"

# 只认带分号的 HTML 实体（Telegram 把正文里的 & 转义成 &amp;）
_HTML_ENTITY_RE = re.compile(r"&(?:#[0-9]+|#[xX][0-9a-fA-F]+|[a-zA-Z][a-zA-Z0-9]{1,31});")

_SIZE_RE = re.compile(
    # 只认带明确字节含义的单位：4K/8K 是分辨率、8M 多半是码率，不能被当成体积
    r"(?<![\w.])(\d+(?:\.\d+)?)\s*(TiB|TB|GiB|GB|MiB|MB|KiB|KB|T|G)\b",
    re.IGNORECASE,
)

_SIZE_UNITS = {
    "kb": 1024,
    "kib": 1024,
    "mb": 1024**2,
    "mib": 1024**2,
    "gb": 1024**3,
    "gib": 1024**3,
    "g": 1024**3,
    "tb": 1024**4,
    "tib": 1024**4,
    "t": 1024**4,
}


@dataclass(frozen=True)
class Link:
    """一条被提取出来的链接。"""

    url: str
    kind: str
    anchor: str = ""  # 隐藏链接的锚文本 / 明链为空
    source: str = "text"  # "entity" 或 "text"，便于排查漏抓


def utf16_slice(text: str, offset: int, length: int) -> str:
    """按 Telegram 的 UTF-16 码元下标切子串。

    Telegram 的 entity offset 以 UTF-16 码元计。emoji 占 2 个码元，直接
    `text[offset:offset+length]` 会错位，所以这里过一遍 UTF-16 字节。
    """
    if offset < 0 or length <= 0:
        return ""
    raw = text.encode("utf-16-le")
    start = offset * 2
    end = start + length * 2
    if start >= len(raw):
        return ""
    try:
        return raw[start:end].decode("utf-16-le", errors="ignore")
    except UnicodeDecodeError:  # pragma: no cover - errors="ignore" 已兜住
        return ""


def utf16_len(text: str) -> int:
    return len(text.encode("utf-16-le")) // 2


def classify(url: str) -> str:
    lowered = url.lower()
    if lowered.startswith("magnet:"):
        return KIND_MAGNET
    if lowered.startswith("ed2k://"):
        return KIND_ED2K
    for kind, hosts in _HOST_KINDS:
        for host in hosts:
            # 只认 host 边界，避免 evil-115.com 之类误判
            if f"//{host}/" in lowered or f"//{host}." in lowered or lowered.endswith(f"//{host}"):
                return kind
    return KIND_OTHER


def normalize_url(url: str) -> str:
    """去掉正文标点、包裹符号和尾部的空转义，并还原被 HTML 转义的链接。

    频道正文里的链接常被转义成 `...?a=1&amp;b=2`，不还原就会把 `&amp;` 原样送进
    Lux，下载端拿到的 query 是错的（review SC15）。
    """
    candidate = _unescape_entities(url or "").strip().strip("<>《》【】「」\"'")
    while candidate and candidate[-1] in _TRAILING_JUNK:
        candidate = candidate[:-1]
    # magnet 里的 dn/tr 参数不要动，只归一化协议前缀大小写
    if candidate.lower().startswith("magnet:"):
        return "magnet:" + candidate[len("magnet:") :]
    return candidate


def _unescape_entities(value: str) -> str:
    """只还原**带分号**的 HTML 实体（`&amp;` / `&#38;`）。

    不带分号的写法（如 query 里的 `&not=2`）是正常参数，html.unescape 会把它当成
    实体 `&not` 吃掉，所以这里先用正则圈定范围再还原。
    """
    if "&" not in value:
        return value
    return _HTML_ENTITY_RE.sub(lambda match: html.unescape(match.group(0)), value)


def extract_links(text: str | None, entities: list[dict] | None = None) -> list[Link]:
    """从正文 + entity 提取去重后的链接，保持出现顺序。

    entity（含隐藏链接）优先，其后是正文正则扫描；同一 URL 只保留第一次出现
    （entity 命中的会被标记 source="entity"）。
    """
    body = text or ""
    found: list[Link] = []
    seen: set[str] = set()

    def push(url: str, anchor: str, source: str) -> None:
        cleaned = normalize_url(url)
        if not cleaned or cleaned in seen:
            return
        if not _looks_like_link(cleaned):
            return
        seen.add(cleaned)
        found.append(Link(url=cleaned, kind=classify(cleaned), anchor=anchor.strip(), source=source))

    for entity in entities or ():
        kind = str(entity.get("type", ""))
        url = entity.get("url") or ""
        if not url:
            continue
        anchor = utf16_slice(body, int(entity.get("offset", 0)), int(entity.get("length", 0)))
        if kind.endswith("TextUrl"):
            push(url, anchor, "entity")
        elif kind.endswith("Url"):
            # 明链 entity：锚文本就是链接本身
            push(url, "", "entity")
        elif kind.endswith("Email"):
            push(url if url.startswith("mailto:") else f"mailto:{url}", "", "entity")

    for match in _URL_RE.finditer(_join_wrapped_ed2k(body)):
        push(match.group(0), "", "text")

    return found


def _join_wrapped_ed2k(text: str) -> str:
    """把被硬换行截断的 ed2k 接回去。

    频道按固定列宽断行：`ed2k://|file|某剧.S01E03...DDP` 另起一行接着
    `5.1.mkv|5364823894|<hash>|/`。不接回去，正则只能拿到没有 hash 的半截链接
    （2026-10-25 线上 34 条死信全是这个原因，且都断在 `DDP` 之后）。
    """
    lines = text.splitlines()
    if len(lines) < 2:
        return text
    out: list[str] = []
    index = 0
    while index < len(lines):
        stripped = lines[index].strip()
        if stripped.lower().startswith("ed2k://") and not _ED2K_HAS_HASH.search(stripped):
            merged = stripped
            tail = index + 1
            while tail < len(lines) and tail <= index + 3 and not _ED2K_HAS_HASH.search(merged):
                merged += lines[tail].strip()
                tail += 1
            out.append(merged)
            index = tail
            continue
        out.append(lines[index])
        index += 1
    return "\n".join(out)


def _looks_like_link(url: str) -> bool:
    lowered = url.lower()
    if lowered.startswith("ed2k://"):
        # 半截 ed2k（缺 size/hash）转不了，送去 Lux 只会换一个 400 死信，直接不收。
        return bool(_ED2K_COMPLETE.match(url))
    if lowered.startswith("magnet:"):
        return True
    if lowered.startswith(("http://", "https://")):
        rest = lowered.split("://", 1)[1]
        # 至少要有 host，且不能只有协议名
        return bool(rest) and "." in rest.split("/", 1)[0]
    return False


def split_kinds(links: list[Link], allowed_kinds: tuple[str, ...] | list[str] | None) -> list[Link]:
    if not allowed_kinds:
        return list(links)
    allowed = set(allowed_kinds)
    return [link for link in links if link.kind in allowed]


def parse_size(text: str | None) -> int | None:
    """从正文里猜资源体积（字节）。多个命中取最大值。

    频道常用 `2.5G` / `8.3 GiB` / `单集 1.2GB`；`4K`/`8K`（分辨率）、`8M`（码率）
    这类没有字节含义的写法不参与匹配。抓不到返回 None，由 filters 决定挂起。
    """
    if not text:
        return None
    best: int | None = None
    for match in _SIZE_RE.finditer(text):
        number = float(match.group(1))
        multiplier = _SIZE_UNITS.get(match.group(2).lower())
        if multiplier is None:
            continue
        value = int(number * multiplier)
        if best is None or value > best:
            best = value
    return best


_LINE_JUNK = " \t\u3000|·•-—"


def _anchor_title(link: Link) -> str:
    """链接自己的锚文本（隐藏链接的文字）；锚文本本身就是链接时不认。"""
    anchor = (link.anchor or "").strip()
    if anchor and not _looks_like_link(anchor) and len(anchor) >= 2:
        return _clean_title(anchor)
    return ""


def _first_line_title(text: str | None) -> str:
    for raw_line in (text or "").splitlines():
        line = raw_line.strip(_LINE_JUNK)
        if line:
            return _clean_title(line)
    return ""


def title_from(text: str | None, links: list[Link] | None = None) -> str:
    """给「追更 / 洗版」判定用的标题：优先隐藏链接的锚文本，其次首行正文。

    这是「一条消息一个标题」的老语义，只适合**单链接**消息；一条消息里有多条
    链接时请逐个用 `link_title`，否则第一个链接的标题会被套给所有链接（SC7）。
    """
    for link in links or ():
        title = _anchor_title(link)
        if title:
            return title
    return _first_line_title(text)


def _name_from_link(url: str, kind: str) -> str:
    """链接自带的作品名（ed2k 文件名 / magnet 的 `dn=`），拿不到返回空串。

    这些频道常在正文里放品牌行（`✅ 少爷:请您检阅`），拿首行当标题会让每条资源
    都判成同一个「库里没有」的假作品；链接里的发布名才是判定链要的东西。
    """
    if kind == KIND_ED2K:
        match = _ED2K_NAME.match(url)
        if not match:
            return ""
        name = match.group(1)
    elif kind == KIND_MAGNET:
        match = _MAGNET_NAME.search(url)
        if not match:
            return ""
        name = urllib.parse.unquote(match.group(1))
    else:
        return ""
    name = name.strip()
    name = re.sub(r"\.[A-Za-z0-9]{1,4}$", "", name)  # 去扩展名（.mkv/.mp4/.ts…）
    return _clean_title(name) if name else ""


def link_title(link: Link, text: str | None, *, fallback_to_text: bool = False) -> str:
    """**单个**链接自己的标题（review SC7）。

    一条消息里常有多个链接，各自挂着不同的锚文本。这里只认该链接自己的锚文本，
    不拿同一条消息里别的链接、也不拿整段正文去凑。

    链接自带发布名（ed2k 文件名 / magnet `dn`）时优先用它：频道的正文首行往往是
    品牌行，比链接里的文件名差得多。

    拿不到独立标题时返回空串。`fallback_to_text=True` 只给**单链接**消息用——
    那种情况下正文首行确实就是这条链接的标题；多链接消息绝不能兜底，调用方
    应当把该链接记 `held`（原因 `title_ambiguous`），不要猜。
    """
    derived = _name_from_link(link.url, link.kind)
    if derived:
        return derived
    title = _anchor_title(link)
    if title:
        return title
    if fallback_to_text:
        return _first_line_title(text)
    return ""


def _clean_title(value: str) -> str:
    collapsed = re.sub(r"\s+", " ", unicodedata.normalize("NFKC", value)).strip()
    return collapsed[:200]


def slugify(value: str, fallback: str = "link") -> str:
    """把标题压成文件名安全的短 slug（inbox 文件名、日志用）。"""
    normalized = unicodedata.normalize("NFKC", value or "").lower()
    pieces = re.findall(r"[0-9a-z\u4e00-\u9fff]+", normalized)
    slug = "-".join(pieces)[:60].strip("-")
    return slug or fallback
