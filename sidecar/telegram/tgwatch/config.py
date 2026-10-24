"""配置加载：读 Lux 的 `archive-settings.json` + 环境变量覆盖。

优先级：设置文件 > 环境变量 > 内置默认值。唯一的例外是 `LUX_TG_API_KEY`，
它只在设置文件 `telegram.api_key` 为空时兜底（D1：文件优先，env 只补缺）。

`telegram` 段由 module-settings 维护，本模块 **只读**。额外读取两个顶层字段：
- `wash_tolerance_ratio`：洗版容差，与 Lux 归档流水线用同一个值（默认 0.02）
- `telegram.api_key`：调用 Lux 管理接口的 `X-Lux-Api-Key`（env `LUX_TG_API_KEY` 兜底）
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

from .filters import MODE_MANUAL, ModeError, normalize_mode
from .links import ALL_KINDS, DEFAULT_ALLOWED_KINDS, KIND_OTHER

MODULE_ROOT = Path(__file__).resolve().parent.parent

DEFAULT_SETTINGS_PATH = "/config/archive-settings.json"
DEFAULT_SESSION_DIR = MODULE_ROOT / "session"
DEFAULT_STATE_DIR = MODULE_ROOT / "state"
DEFAULT_INBOX_DIR = "/watch-inbox"
DEFAULT_LUX_BASE = "http://lux:8097"
DEFAULT_INGEST_PATH = "/api/v1/admin/archive/ingest"
DEFAULT_LOOKUP_PATH = "/api/v1/admin/archive/lookup"
DEFAULT_PLAN_PATH = "/api/v1/admin/archive/telegram/plan"
DEFAULT_MISSING_PATH = "/api/v1/admin/archive/telegram/missing"
DEFAULT_WASH_TOLERANCE = 0.02

SINK_HTTP = "http"
SINK_INBOX = "inbox"
SINKS = (SINK_HTTP, SINK_INBOX)


class ConfigError(RuntimeError):
    """配置缺失或非法；main() 捕获后打印一行中文原因并退出码 2。"""


@dataclass(frozen=True)
class ChannelConfig:
    channel: str  # "@name"、https://t.me/name 或数字 id
    mode: str = MODE_MANUAL
    follow_list: tuple[str, ...] = ()

    def key(self) -> str:
        """规范化频道标识：去掉 `@`、`t.me/` 前缀，只留 username 或数字 id。"""
        value = self.channel.strip()
        for prefix in ("https://t.me/", "http://t.me/", "t.me/"):
            if value.lower().startswith(prefix):
                return value[len(prefix) :].strip("/")
        return value.lstrip("@")

    def describe(self) -> str:
        return f"{self.key()}({self.mode})"


@dataclass(frozen=True)
class SidecarConfig:
    api_id: int
    api_hash: str
    phone: str = ""
    password: str = ""  # 2FA，只从 TG_2FA_PASSWORD 来，绝不回显
    default_mode: str = MODE_MANUAL
    channels: tuple[ChannelConfig, ...] = ()
    follow_list: tuple[str, ...] = ()
    allowed_kinds: tuple[str, ...] = DEFAULT_ALLOWED_KINDS
    wash_tolerance: float = DEFAULT_WASH_TOLERANCE
    session_dir: Path = DEFAULT_SESSION_DIR
    state_dir: Path = DEFAULT_STATE_DIR
    inbox_dir: Path = Path(DEFAULT_INBOX_DIR)
    session_name: str = "lux-telegram"
    sink: str = SINK_HTTP
    ingest_url: str = DEFAULT_LUX_BASE + DEFAULT_INGEST_PATH
    lookup_url: str = DEFAULT_LUX_BASE + DEFAULT_LOOKUP_PATH
    #: 判定接口：入新/追更/洗版由 Lux 用 catalog 判定，旁挂只执行。
    plan_url: str = DEFAULT_LUX_BASE + DEFAULT_PLAN_PATH
    #: 补集清单：Lux 报缺哪几集，旁挂据此去频道里搜。
    missing_url: str = DEFAULT_LUX_BASE + DEFAULT_MISSING_PATH
    api_key: str = ""
    backfill_limit: int = 200
    gap_limit: int = 200
    retry_seconds: int = 300
    http_timeout: float = 15.0
    dry_run: bool = False
    log_level: str = "INFO"
    settings_path: Path = Path(DEFAULT_SETTINGS_PATH)

    # ---- 派生属性 ----

    @property
    def session_path(self) -> Path:
        return self.session_dir / f"{self.session_name}.session"

    @property
    def state_db(self) -> Path:
        return self.state_dir / "state.db"

    @property
    def code_file(self) -> Path:
        """登录时人工录入验证码的落点（容器没有 tty 时用）。"""
        return self.session_dir / "code.txt"

    def mode_for(self, channel_key: str) -> str:
        for channel in self.channels:
            if channel.key() == channel_key:
                return channel.mode
        return self.default_mode

    def follow_for(self, channel_key: str) -> tuple[str, ...]:
        for channel in self.channels:
            if channel.key() == channel_key and channel.follow_list:
                return channel.follow_list
        return self.follow_list

    def masked(self) -> dict:
        """status 命令 / 日志用；永不输出 api_hash、phone、2FA、api_key 明文。"""
        return {
            "settings_path": str(self.settings_path),
            "api_id": self.api_id,
            "api_hash": mask_secret(self.api_hash),
            "phone": mask_phone(self.phone),
            "two_factor": bool(self.password),
            "mode": self.default_mode,
            "channels": [channel.describe() for channel in self.channels],
            "follow_list": list(self.follow_list),
            "allowed_kinds": list(self.allowed_kinds),
            "wash_tolerance": self.wash_tolerance,
            "session": str(self.session_path),
            "state_db": str(self.state_db),
            "sink": self.sink,
            "inbox_dir": str(self.inbox_dir),
            "ingest_url": mask_url(self.ingest_url),
            "lookup_url": mask_url(self.lookup_url),
            "plan_url": mask_url(self.plan_url),
            "missing_url": mask_url(self.missing_url),
            "api_key": mask_secret(self.api_key),
            "backfill_limit": self.backfill_limit,
            "gap_limit": self.gap_limit,
            "retry_seconds": self.retry_seconds,
            "dry_run": self.dry_run,
            "log_level": self.log_level,
        }

    def secrets(self) -> list[str]:
        """需要从日志里擦掉的明文，交给 logsafe.SecretRedactor。"""
        values = [self.api_hash, self.phone, self.password, self.api_key]
        digits = "".join(ch for ch in self.phone if ch.isdigit())
        if len(digits) >= 6:
            values.append(digits)
        return [value for value in values if value]


def mask_secret(value: str | None) -> str:
    if not value:
        return ""
    text = str(value)
    if len(text) <= 4:
        return "*" * len(text)
    return f"{text[:2]}****{text[-2:]}({len(text)})"


def mask_phone(value: str | None) -> str:
    if not value:
        return ""
    digits = "".join(ch for ch in str(value) if ch.isdigit())
    if len(digits) < 6:
        return "****"
    return f"+{digits[:3]}****{digits[-2:]}"


def mask_url(value: str | None) -> str:
    """URL 只保留 scheme+host，path/query 里的 token 一律不露。"""
    if not value:
        return ""
    text = str(value)
    scheme, _, rest = text.partition("://")
    if not rest:
        return mask_secret(text)
    host = rest.split("/", 1)[0]
    return f"{scheme}://{host}/…"


def _env(name: str, default: str = "") -> str:
    return os.environ.get(name, default).strip()


def _env_int(name: str, default: int) -> int:
    raw = _env(name)
    if not raw:
        return default
    try:
        return int(raw)
    except ValueError as error:
        raise ConfigError(f"{name} 必须是整数，实际: {raw!r}") from error


def _env_float(name: str, default: float) -> float:
    raw = _env(name)
    if not raw:
        return default
    try:
        return float(raw)
    except ValueError as error:
        raise ConfigError(f"{name} 必须是数字，实际: {raw!r}") from error


def _env_bool(name: str, default: bool) -> bool:
    raw = _env(name).lower()
    if not raw:
        return default
    if raw in ("1", "true", "yes", "on"):
        return True
    if raw in ("0", "false", "no", "off"):
        return False
    raise ConfigError(f"{name} 必须是布尔值，实际: {raw!r}")


def _split_list(value: str | None) -> tuple[str, ...]:
    if not value:
        return ()
    return tuple(part.strip() for part in str(value).split(",") if part.strip())


def _as_list(value) -> tuple[str, ...]:
    if value is None:
        return ()
    if isinstance(value, str):
        return _split_list(value)
    if isinstance(value, (list, tuple)):
        return tuple(str(item).strip() for item in value if str(item).strip())
    return ()


def load_settings_file(path: str | Path) -> dict:
    """读设置文件；文件缺失返回空 dict（此时全靠 env）。

    权限错误单独给可操作提示（review SC2）：容器以 uid 10001 只读挂载设置文件，
    Lux 保存后写成 0640 + 属组；读不到时多半是属组没对上，不能再笼统报 ConfigError。
    """
    target = Path(path)
    if not target.exists():
        return {}
    try:
        text = target.read_text(encoding="utf-8")
    except PermissionError as error:
        raise ConfigError(
            f"设置文件不可读：检查属组/权限（见 README 的部署检查清单）：{target}（{error}）。"
            "文件应为 0640 + 属组 $LUX_ARCHIVE_SETTINGS_GROUP，"
            "compose 用 group_add 让 uid 10001 加入同一个 gid"
        ) from error
    except OSError as error:
        raise ConfigError(f"无法读取设置文件 {target}: {error}") from error
    try:
        raw = json.loads(text)
    except json.JSONDecodeError as error:
        raise ConfigError(f"设置文件不是合法 JSON（{target}）：{error}") from error
    return raw if isinstance(raw, dict) else {}


def parse_channels(value, default_mode: str = MODE_MANUAL) -> tuple[ChannelConfig, ...]:
    """listen_channels 支持两种写法：

    - 简单：`["@a", "-100123456"]`（沿用全局 listen_mode）
    - 精细：`[{"channel": "@a", "mode": "追更", "follow_list": ["某剧"]}]`
    """
    channels: list[ChannelConfig] = []
    for item in value or ():
        if isinstance(item, dict):
            name = item.get("channel") or item.get("username") or item.get("id")
            if name is None or not str(name).strip():
                continue
            try:
                mode = normalize_mode(item.get("mode") or item.get("listen_mode"), default=default_mode)
            except ModeError as error:
                raise ConfigError(f"频道 {name} 的 mode 非法: {error}") from error
            follow = _as_list(item.get("follow_list"))
            channels.append(ChannelConfig(channel=str(name).strip(), mode=mode, follow_list=follow))
        elif isinstance(item, (int, str)) and str(item).strip():
            channels.append(ChannelConfig(channel=str(item).strip(), mode=default_mode))
    return tuple(channels)


def load(settings_path: str | Path | None = None) -> SidecarConfig:
    path = Path(settings_path or _env("LUX_ARCHIVE_SETTINGS", DEFAULT_SETTINGS_PATH))
    settings = load_settings_file(path)
    section = settings.get("telegram") if isinstance(settings.get("telegram"), dict) else {}

    api_id_raw = _env("LUX_TG_API_ID") or str(section.get("api_id") or "").strip()
    if not api_id_raw:
        raise ConfigError(f"缺少 api_id：在 {path} 的 telegram.api_id 里填，或设置 LUX_TG_API_ID")
    try:
        api_id = int(api_id_raw)
    except ValueError as error:
        raise ConfigError(f"api_id 必须是整数，实际: {api_id_raw!r}") from error

    api_hash = _env("LUX_TG_API_HASH") or str(section.get("api_hash") or "").strip()
    if not api_hash:
        raise ConfigError("缺少 api_hash：在设置文件 telegram.api_hash 里填，或设置 LUX_TG_API_HASH")

    phone = _env("LUX_TG_PHONE") or str(section.get("phone") or "").strip()
    # 文档约定 TG_2FA_PASSWORD；兼容带 LUX_ 前缀的写法
    password = _env("TG_2FA_PASSWORD") or _env("LUX_TG_2FA_PASSWORD")

    mode_raw = _env("LUX_TG_MODE") or section.get("listen_mode") or MODE_MANUAL
    try:
        default_mode = normalize_mode(mode_raw)
    except ModeError as error:
        raise ConfigError(str(error)) from error

    channels = parse_channels(
        _split_list(_env("LUX_TG_CHANNELS")) or section.get("listen_channels"), default_mode
    )
    follow_list = _split_list(_env("LUX_TG_FOLLOW")) or _as_list(section.get("follow_list"))

    kinds_raw = _split_list(_env("LUX_TG_ALLOWED_KINDS"))
    allowed = kinds_raw or DEFAULT_ALLOWED_KINDS
    unknown_kinds = [kind for kind in allowed if kind not in ALL_KINDS]
    if unknown_kinds:
        raise ConfigError(f"未知链接类型 {unknown_kinds}，可用: {list(ALL_KINDS)}")
    # 默认不放行 other（任意 http 网页）；只有显式开关才收（review SC5）
    allow_other = _env_bool("LUX_TG_ALLOW_OTHER", False)
    if allow_other and KIND_OTHER not in allowed:
        allowed = (*allowed, KIND_OTHER)

    sink = _env("LUX_TG_SINK", SINK_HTTP).lower()
    if sink not in SINKS:
        raise ConfigError(f"LUX_TG_SINK 只能是 {'/'.join(SINKS)}，实际: {sink!r}")

    base = (_env("LUX_TG_LUX_BASE") or DEFAULT_LUX_BASE).rstrip("/")
    ingest_url = _env("LUX_TG_INGEST_URL") or f"{base}{DEFAULT_INGEST_PATH}"
    lookup_url = _env("LUX_ARCHIVE_LOOKUP_URL") or f"{base}{DEFAULT_LOOKUP_PATH}"
    plan_url = _env("LUX_TG_PLAN_URL") or f"{base}{DEFAULT_PLAN_PATH}"
    missing_url = _env("LUX_TG_MISSING_URL") or f"{base}{DEFAULT_MISSING_PATH}"
    # D1：文件优先、env 兜底。Lux 保存设置时会把 key 写进 telegram.api_key，
    # 那才是唯一权威值；LUX_TG_API_KEY 只补「文件里还没填」的窗口期。
    api_key = str(section.get("api_key") or "").strip() or _env("LUX_TG_API_KEY")
    if sink == SINK_HTTP and not api_key:
        raise ConfigError(
            "缺少 Lux 管理 API key（sink=http 必需）：在设置文件的 telegram.api_key 里填"
            f"（{path}；管理页「API key」或 POST /api/v1/admin/api-key/rotate 生成），"
            "或设置环境变量 LUX_TG_API_KEY 兜底"
        )

    tolerance_raw = _env("LUX_TG_WASH_TOLERANCE") or settings.get("wash_tolerance_ratio")
    wash_tolerance = DEFAULT_WASH_TOLERANCE
    if tolerance_raw is not None and str(tolerance_raw).strip():
        try:
            wash_tolerance = float(tolerance_raw)
        except (TypeError, ValueError) as error:
            raise ConfigError(f"wash_tolerance_ratio 必须是数字，实际: {tolerance_raw!r}") from error
    if not 0 <= wash_tolerance <= 1:
        raise ConfigError("wash_tolerance_ratio 必须在 0~1 之间")

    backfill_limit = _env_int("LUX_TG_BACKFILL_LIMIT", 200)
    if backfill_limit < 0:
        raise ConfigError("LUX_TG_BACKFILL_LIMIT 不能为负")
    gap_limit = _env_int("LUX_TG_GAP_LIMIT", 200)
    if gap_limit < 0:
        raise ConfigError("LUX_TG_GAP_LIMIT 不能为负")

    return SidecarConfig(
        api_id=api_id,
        api_hash=api_hash,
        phone=phone,
        password=password,
        default_mode=default_mode,
        channels=channels,
        follow_list=follow_list,
        allowed_kinds=allowed,
        wash_tolerance=wash_tolerance,
        session_dir=Path(_env("LUX_TG_SESSION_DIR") or DEFAULT_SESSION_DIR),
        state_dir=Path(_env("LUX_TG_STATE_DIR") or DEFAULT_STATE_DIR),
        inbox_dir=Path(_env("LUX_TG_INBOX_DIR", DEFAULT_INBOX_DIR)),
        session_name=_env("LUX_TG_SESSION_NAME", "lux-telegram"),
        sink=sink,
        ingest_url=ingest_url,
        lookup_url=lookup_url,
        plan_url=plan_url,
        missing_url=missing_url,
        api_key=api_key,
        backfill_limit=backfill_limit,
        gap_limit=gap_limit,
        retry_seconds=_env_int("LUX_TG_RETRY_SECONDS", 300),
        http_timeout=_env_float("LUX_TG_HTTP_TIMEOUT", 15.0),
        dry_run=_env_bool("LUX_TG_DRY_RUN", False),
        log_level=_env("LUX_TG_LOG_LEVEL", "INFO").upper(),
        settings_path=path,
    )


def runtime_warnings(config: SidecarConfig) -> list[str]:
    """`check` 用：配置能读但跑起来没意义的情况。"""
    warnings: list[str] = []
    if not config.channels:
        warnings.append("telegram.listen_channels 为空：启动后不会监听任何频道")
    if config.default_mode == MODE_MANUAL and not any(
        channel.mode != MODE_MANUAL for channel in config.channels
    ):
        warnings.append("当前是「手动」模式：链接只会入库，需要 `main.py replay --all` 才投递")
    if not config.api_key:
        warnings.append("telegram.api_key 为空：洗版查询会被 Lux 拒绝，条目会挂起")
    if config.sink == SINK_INBOX and not Path(config.inbox_dir).is_dir():
        warnings.append(
            f"inbox 目录不存在（{config.inbox_dir}）：仅 LUX_TG_SINK=inbox 时需要，默认走 http"
        )
    return warnings


def runtime_problems(config: SidecarConfig) -> list[str]:
    """`check` 用：硬错误（healthcheck 靠它返回非零）。"""
    problems: list[str] = []
    # 频道为空不是硬错误：设置页要先能列出账号里的频道，才有了勾选的前提。
    # 空配置下侧车空转（/healthz、/channels 仍可用），由 runtime_warnings 提示。
    if not config.session_path.exists():
        problems.append(f"session 不存在（{config.session_path}）：等首次登录完成")
    if config.sink == SINK_HTTP and not config.api_key:
        problems.append("sink=http 但 telegram.api_key 为空")
    return problems
