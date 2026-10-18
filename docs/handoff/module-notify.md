# 模块：Telegram 通知（module-notify）

目标：把归档/播放/任务事件推送到 Telegram（bot `sendMessage`），凭证来自
`archive-settings.json` 的 `telegram` 段。配置 **以文件为准**，环境变量只补文件缺失的键。
通知是 **best effort**：发送失败只记 WARN，绝不影响归档或播放流程。

## 交付状态（2026-10-18）

已实现并接入：

- 独立模块 `src/application/notify/`（`mod.rs` / `config.rs` / `event.rs` / `format.rs` / `send.rs`）
  - 说明：handoff 原定 `src/application/archive/notify.rs`，因为播放和未来的签到/115/strm
    模块也要调用，改为顶层 `application::notify`；归档 `run_once` 已接入，文件边界仍只新增本目录。
- 归档链路：`run_once`（非 dry-run）结束后发送「单条事件 + 汇总」
- 播放链路：`PLAYBACK_STARTED` 时发送「正在播放」（`src/api/playback.rs` 只加构造调用）
- 管理接口：`GET /api/v1/admin/notify/status`、`POST /api/v1/admin/notify/test`、
  `POST /api/v1/admin/notify/send`（给其他模块 / Python 旁挂调用）

## 通知类别

与 chill `wechat_notify.json` 的 key 对齐，`telegram.notify` 支持英文 key 或中文标签：

| key | 中文 | 触发点 |
| --- | --- | --- |
| `playback` | 播放 | Lux `PLAYBACK_STARTED`（已接） |
| `media_added` | 入库 | 预留 API（Lux `MEDIA_ADDED` 尚未接） |
| `organize_complete` | 整理/归档 | 归档整理成功（已接） |
| `wash_result` | 洗版 | 归档洗版（已接） |
| `duplicate` | 重复 | 归档发现重复条目（已接，逐条受开关控制） |
| `resource_transfer` | 转存 | 转存→待整理（已接） |
| `drive115_upload` | 115妙传/秒传 | 预留 API（115 模块接入） |
| `media_organize_task` | 媒体整理 | 预留 API |
| `checkin` | 签到 | 预留 API（签到任务 / 旁挂调用） |
| `task_complete` | 其他任务（strm 等） | 归档汇总（已接）+ 预留 API |
| `failure` | 失败 | 归档失败条目（已接） |

`telegram.notify` 缺省 = 全部开启；数组 = 只开列出的；对象 = 逐项布尔。

## 配置

设置文件：`LUX_ARCHIVE_SETTINGS`，默认 `/config/archive-settings.json`（只读，不由本模块写）。

```json
{
  "telegram": {
    "bot_token": "123:abc",
    "chat_id": "5867802360",
    "notify": ["播放", "入库", "归档", "失败", "洗版", "重复", "转存", "115妙传", "媒体整理", "签到", "其他"],
    "api_base": "https://api.telegram.org",
    "message_thread_id": null,
    "silent": false,
    "min_interval_ms": 1200,
    "max_per_minute": 20,
    "dedupe_seconds": 60,
    "timezone_offset_hours": 8
  }
}
```

环境变量兜底（**仅补文件缺失的键，文件优先**）：`LUX_TG_BOT_TOKEN`、`LUX_TG_CHAT_ID`、`LUX_TG_API_BASE`、
`LUX_TG_THREAD_ID`、`LUX_TG_SILENT`、`LUX_TG_NOTIFY`（逗号分隔）、`LUX_TG_MIN_INTERVAL_MS`、
`LUX_TG_MAX_PER_MINUTE`、`LUX_TG_DEDUPE_SECONDS`、`LUX_TG_TIMEZONE_OFFSET_HOURS`。

bot_token/chat_id 任一缺失：静默跳过（debug 日志），状态接口显示 `configured=false`。

## 消息

HTML parse_mode，chill 风格中文卡片：标题 emoji + 字段（用户/客户端/IP/类型/库位/大小/耗时…）
+ 简介 + 时间 + `— Lux`。单条 ≤ **3900 UTF-16 码元**（Telegram 按 UTF-16 计 4096；
emoji 算 2），超出先截简介、再逐字段截断，末尾 `…`（截断不破坏 HTML 实体），
动态文本全部 HTML 转义；URL 只接受 http/https 且整段链接放得下才附加。

## 发送策略

- 单次归档最多每类 10 条单条通知（含重复），其余由汇总承载（汇总含各计数与前 3 个片名）
- dry-run 不发通知
- 全局串行 + 最小间隔（默认 1.2s）+ 每分钟上限（默认 20），429 时按 `retry_after` 重试一次
- 相同 `dedupe_key` 在窗口内只发一次（播放按 session+条目，签到等可由调用方传入）
- 计数：内存态 `sent/failed/skipped/lastSuccessAt/lastFailureAt/lastError`
- 日志与接口永不输出 token/完整 URL；`chat_id` 与 `api_base` 只返回打码值
  （chat_id 用 settings 的 `head6…(len)` 格式，api_base 去掉凭据与路径）

## 接口

- `GET /api/v1/admin/notify/status` → 配置态、类别开关、计数、限流参数
- `POST /api/v1/admin/notify/test` → 发测试消息，返回 `{ok, skipped, messageId, error}`，
  不需要 CSRF 之外的特殊权限（admin）
- `POST /api/v1/admin/notify/send`：

```json
{
  "category": "task",
  "title": "STRM 生成完成",
  "fields": [{"label": "🎞️ 生成", "value": "123"}],
  "overview": "可选",
  "url": "https://…",
  "silent": false,
  "dedupe_key": "strm:2026-10-18"
}
```

`category` 接受 key 或中文标签；返回 `{ok, skipped, category, messageId, error}`，
参数错误返回 400 `INVALID_REQUEST`。旁挂可用 admin API Key 调用。

## 接入点（给其他模块）

Rust：`crate::application::notify::spawn_event(NotifyEvent::new(...))`，或
`spawn_events(vec![...])`。构造器：`NotifyEvent::playback(PlaybackNotice)`、
`spawn_archive_report(&ArchiveReport)`；通用事件用 builder（`.field/.overview/.url/.dedupe_key`）。

Python / 外部：`POST /api/v1/admin/notify/send`。

## 测试

`cargo test --locked --lib notify`：配置解析（中文数组/对象/缺省）、文件优先与 env 补缺、
类别别名、HTML 转义、按类别截断、3900 UTF-16 码元上限、时间偏移、去重窗口、限流、请求体、
429 重试解析、打码、状态序列化。

## 待办 / 边界

- `media_added` 需在扫描器发 `MEDIA_ADDED` 处加一行调用（本次未动扫描器，按用户选择）
- `drive115_upload` / `media_organize_task` / `checkin` 等待对应模块接入
- 设置页 UI 由 module-settings 提供；本模块只读设置文件
- 不做持久化重试队列（chill 行为对齐：失败记日志）

## 验收整改（2026-10-18，分支 `feat/notify-settings`）

依据 `docs/handoff/fix-notify-settings.md`：

- **A 配置优先级**：改为消费 `ArchiveSettings::load()`，删除 notify 自己的设置文件解析；
  `LUX_TG_*` 只补文件缺失的键（文件优先），单测覆盖「文件值覆盖 env」与「env 补缺」。
- **C 重复通知**：新增 `duplicate` 类别，逐条发送且受 `telegram.notify` 开关控制；汇总保留重复计数。
- **E 小项**：status 的 `api_base` 打码（去凭据/路径）；`chat_id` 复用 settings 的
  `mask_secret`（`head6…(len)`）；消息长度按 UTF-16 码元计算；播放通知上下文在
  `state.webhooks` 与通知都未配置时短路，不做无用的条目查询。

说明：整改单中的 **B（单文件挂载 PUT）** 与 **D（save 不落 env 派生值）** 属 settings 模块，
本分支未改动，交由设置侧整改。

## 第二轮整改（2026-10-18，审核组代做）

- **类别清单由后端下发**：`GET/PUT /api/v1/admin/archive/settings` 的响应带 `known_categories`
  （`{key,label,emoji}`），设置页据此渲染。新增类别只改 `notify/config.rs`，前端不用动，
  也不会再出现"保存一次就把「重复」静默关掉"。
- **保存不改变形态**：设置页没动通知项时，回传的是原始 `null` 或对象；只有真正勾选过才写数组。
- **逐条事件带去重键**：转存/整理/洗版/重复/失败事件都带 `dedupe_key`（含条目名与原因），
  同一个坏条目每个去重窗口只提醒一次，避免每轮轮询刷屏。
- **去重键在发送成功后写入**：发送失败不会再被自己的去重窗口抑制一个周期。
- **播放通知带 IP**：三个播放上报点都为 PlaybackStarted 传 `remote_ip`（此前只在停止时传）。
- 凭据仍只读设置文件；`telegram.api_key` 是旁挂用的管理 key，不参与通知发送。
