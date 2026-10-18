# 设置组整改单（第二轮 · 2026-10-18）

审核对象：`feat/notify-settings` @1b90afb4 的 `src/application/archive/settings.rs`、
`src/api/admin/admin_jobs.rs`（settings 路由）、`web/src/features/settings/ArchiveSettingsSection.tsx`。
依据：`SOURCE-MAP.md` 里逐条 file:line 的只读审核。决策见 `REVIEW-BUS.md` 的 D1–D10。

结论：**CHANGES**。F2/F1/F8 已达标，F4 未做，另外新增 2 条严重项。

## 一、必须修（合并前）

### S1 [严重] PUT 越界数值被静默钳制 → 200
- 位置：`settings.rs:269`（`apply_update` 里先 `.sanitized()`）、`settings.rs:315-327`（钳制逻辑）、
  `admin_jobs.rs:1332`（validate 在 sanitize 之后）
- 现象：`PUT {"poll_seconds":7200}` 返回 200 并写入 0（等于关掉轮询）；`tolerance=4.0` 变 0.02。
  与 `admin_jobs.rs:1313` 注释「rejected instead of clamped」和 §9 文档直接冲突。
- 改法：校验必须在钳制之前；非法值返回 400 `INVALID_REQUEST`，不得改写用户输入。
- 验收：`poll_seconds=7200`、`settle_seconds=-1`、`wash_tolerance_ratio=4.0`、`"abc"`/`1.5` 全部 400；
  合法边界（0、3600、1.0）仍 200。

### S2 = F4 [严重] `save()` 把 env 派生值写进文件
- 位置：`settings.rs:237-240`（`save_to` 全量序列化 `self`）、`admin_jobs.rs:1322-1341`（GET→改→PUT）
- 现象：`ArchiveSettings::load()` 先与 env 合并，GET 回显合并值，前端原样回传，PUT 把整结构写回。
  首次 UI 保存后 `LUX_ARCHIVE_*` 被固化，之后改 env 失效。模块方上一轮已认可 provenance 方案但未落地
  （全仓无 `provenance` / `present_keys`）。
- 改法（按 D1/D2）：
  1. `load_from_with_base` 记录文件里**实际出现**的键集合（provenance）；
  2. `apply_update` 只把「值与该键 env 基值不同」的键标为 file-owned；
  3. `save_to` 只序列化 file-owned 键。
- 验收：只设 `LUX_ARCHIVE_WATCH_DIRS` 时，UI 保存一次后文件里不出现 `watch_dirs`，env 仍生效；
  在 UI 里改过的键才落盘。补一条往返单测。

### S3 [严重] `telegram.api_key` 无归属，保存即被抹掉
- 位置：`settings.rs:76-114`（`TelegramSettings` 无 `api_key`）、`settings.rs:236-241`（整份重写）、
  `settings.rs:262-268`（`apply_update` 只保留 4 个 secret）
- 影响：旁挂从 `telegram.api_key` 读管理 key（`sidecar config.py:309-313`），管理员在 UI 保存任意一项都会
  把它删掉 → 旁挂重启 exit 2，ingest/lookup 全部失败。这是旁挂上线的唯一阻塞项（见旁挂组 SC1）。
- 改法（按 D3）：`TelegramSettings` 增加 `api_key: String`，`masked()` 打码，空值/打码值原样保留，
  `apply_update` 视为 secret；前端加密码框（留空=不改）。旁挂仍允许 `LUX_TG_API_KEY` 覆盖。
- 验收：写入 `telegram.api_key` → GET 显示打码 → PUT（api_key 留空）后文件里原值仍在 → 旁挂可读到。

### S5 [一般] 对象型 `listen_channels` 被丢弃
- 位置：`settings.rs:663-681`（`string_list_field` 的 `_ => None`）、`settings.rs:394-396`
- 现象：旁挂支持 `listen_channels: [{"channel":"@a","mode":"追更","follow_list":["某剧"]}]`
  （`sidecar config.py:244-261`、README:46-47），Lux 结构体只认字符串数组 → GET 看不到、PUT 后逐频道配置被清空。
- 改法：保留原始 JSON（`Vec<serde_json::Value>` 或自定义 enum），读时兼容两种形态，写时原样回写。
- 验收：对象形态经 GET→PUT 往返后逐字段不变，旁挂能读到每频道 mode/follow_list。

### S7 [一般] 并发 PUT 无锁
- 位置：`admin_jobs.rs:1322-1341`（read-modify-write）、`settings.rs:550-560`（原地写回退两次 truncate）
- 改法：进程内 `Mutex`（或文件锁）串行化 settings 写入；原地写回退时先备份再写、失败回滚。
- 验收：并发 20 个 PUT 后文件仍是完整 JSON，且最后一次写入生效。

## 二、小项（合并后可排期，建议同批做）

- S4 [一般] 原地写先 `truncate` 再 `write`，写失败/崩溃留下半截文件且旧值不可恢复 → 先写 `.bak`、`fsync`、
  失败回滚；`set_private` 在 `?` 之后不执行（`settings.rs:550-560`）。
- S6 [一般] 前端 `NOTIFY_CATEGORIES` 只有 10 项缺「重复」（`ArchiveSettingsSection.tsx:16-27`）→ 按 D5 由后端
  `known_categories` 下发，前端只渲染。
- S8 [小项] 临时文件名仅 pid+自增（`settings.rs:488-490,531-535`），跨进程可能同名 → 加随机后缀。
- S9 [小项] `rename` 后才 chmod；`set_private` 失败返回 500 但文件已替换（`settings.rs:536-547`）→ 先 chmod 临时文件再 rename。
- S10 [小项] `masked()` 未处理 `telegram.api_base`，与 notify status 的打码不一致，URL 内凭据可能回显。
- S11 [小项] 数值不可解析（`-1`/`1.5`/`"abc"`）静默忽略并 200 → 统一 400（与 S1 同批）。
- S12 [小项] validate 跑在 env+文件合并值上，env 里目录失效时任何 PUT 都 400 → 只校验本次写入的字段。

## 三、已核实无问题（不用重做）

- F2 回退路径同时覆盖 `ErrorKind::{CrossesDevices,ResourceBusy}` 与 `raw_os_error` EBUSY/EXDEV（`settings.rs:562-577`），
  单测 `settings.rs:938-955`；已存在文件路径也会 0600。
- F1：`ArchiveConfig::from_env` 读 settings（`archive.rs:118-130`），轮询每轮重读并重建 service；
  `LUX_ARCHIVE_ROOT` / `LUX_TMDB_LANGUAGE` 的 env-only 例外与 §8 文档一致。
- F8：`notify/config.rs:13` 复用 `mask_secret`，输出 `head6…(len)`。
- 打码值回传是精确等值比较（`settings.rs:464-476`），不会因长度相同误判（代价：凭证无法清空，可接受）。
- 前端 5 个密钥字段初始为空、打码值只进 placeholder；payload 里空密钥整键省略。
- 路径类校验（绝对路径 + 存在 + `listen_mode` 白名单）有效；对象型之外的空数组、`..`、符号链接不构成缺陷。

## 四、汇报格式

逐条一个提交，信息带编号：`fix(settings): S1 reject out-of-range values instead of clamping`。
完成后更新 `REVIEW-BUS.md` 自己那节（`状态: READY`、每条 `✅ 提交哈希 + 证据`、测试结果、遗留），
并 `git commit -m "bus(settings): ready for review" -- docs/handoff/REVIEW-BUS.md`。
测试要求（D10）：`cargo test --locked --lib archive` 全绿 + `pnpm build` 通过 + 单文件挂载下 PUT 实测一次。
