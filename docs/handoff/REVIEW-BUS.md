# 评审总线（REVIEW-BUS）

> 规则见 `WORKFLOW.md`。任何状态变化都要提交，提交信息 `bus(<模块>): ...`。
> 模块方开工前先看自己那节；审核方每轮更新结论。

## 总控交接（2026-10-18）

原总控对话框上下文用尽，审核职责移交**审核组对话框**。接手方开工先读：
`WORKFLOW.md`（规则）、本文件（状态）、`PROGRESS.md`（进度）、`fix-*.md`（各模块整改清单）。

硬约束（不变）：镜像只由总控构建，模块方不得覆盖 `lux:phase-a` 标签；合并走 `/tmp/lux-merge`
干净克隆；`sidecar/**` 只有一个写入者（旁挂组）。

---

## 本轮审核（2026-10-18 第二轮 · 审核组）

范围：设置 / 通知 / 旁挂 / Rust 侧 ingest 四个在飞模块，外加归档核心的 F1。
结论：**F1 已修并合入 `fork/phase-a`；四个模块全部 CHANGES，本轮一个都不合并。**

整改单（每个工作组一份，含全部 file:line 证据与验收标准）：

- 设置组：`fix-settings-round2.md`
- 通知组：`fix-notify-round2.md`
- 旁挂组：`fix-sidecar-round2.md`
- 归档/ingest 组：`fix-ingest-round2.md`

### 决策（冻结，模块方按此执行，不必再来问）

| 编号 | 决定 |
| --- | --- |
| D1 | 配置优先级统一 **文件 > env > 默认**；env 只补文件缺失的键（`LUX_ARCHIVE_ROOT` / `LUX_TMDB_LANGUAGE` 仍 env-only）。 |
| D2 | **禁止静默钳制**：PUT 里越界的数值（poll/settle/容差）必须 400，不得改写后返回 200。 |
| D3 | 新增 `telegram.api_key` 字段（打码显示、留空不改、原样保留）；旁挂仍可用 `LUX_TG_API_KEY` 覆盖。 |
| D4 | 设置文件保存后 `chmod 0640` + `chgrp $LUX_ARCHIVE_SETTINGS_GROUP`（未设则保持 0600）；旁挂 compose 用 `group_add` 取得同组读权限。 |
| D5 | 通知类别清单由后端下发（settings 响应增加 `known_categories`），前端不再硬编码。 |
| D6 | `duplicate` 默认开启；PUT 不拒绝未知类别（前向兼容），读取时忽略未知项。 |
| D7 | 分享链接在日志/状态里只允许出现 `scheme://host`；query、password 一律遮蔽。 |
| D8 | 允许一次端到端 TG 实测：消息前缀「【Lux 验收测试】」，凭据取自 `/vol5/1000/build/archive-settings.json`，只发一条。 |
| D9 | 设置读取失败的 WARN：每进程只记一次，之后降为 debug。 |
| D10 | 合并门禁：`cargo test --locked --lib archive|notify|playback` 全绿、`pnpm build` 通过、全仓 grep 真实手机号 / api_id / token 0 命中。 |

---

## F1【严重】剧集包必须按季判定 — ✅ 已修（`42134498` + `6413a8b0`）

- 根因：入库判定以**整个目录包**为单位比体积。`聊斋 (1996) {tmdb-68678}` 包里有 Season 01+02，
  库里只有 Season 01，整包被判重复搬走 → 库里永远缺 Season 02。
- 修法（`src/application/archive.rs`）：新增 `organize_series_pack` / `apply_season_plan` /
  `plan_season_pack` / `season_dirs`。剧集包按每个 `Season NN` 子目录独立判定：
  库里没有 → 并入；体积等效或更小 → 该季进 `重复目录/<原包名>/Season NN`；明显更大 → 洗版该季。
  全部季都重复才退回整包判定；合并后重新生成该剧镜像（已有 sidecar 不覆盖）；包内零散文件并入片名目录；
  已处理/已失败的季目录不会被当成 leftover 二次搬动（`6413a8b0`）。
- 单测：`cargo test --locked --lib archive` → **34 通过**（新增 3 个：两季包只并缺季 / 全重复包 / 季洗版）。
- 测试库数据已修（审核组执行并核验）：
  - `重复目录/聊斋 (1996) {tmdb-68678}/Season 02`（40 个文件）已并入
    `媒体目录/电视剧/国内/1996/聊斋 (1996)/Season 02`；
  - 镜像 `聊斋 (1996)/Season 02` 的 80 个 sidecar 就位，逐条校验 strm 目标存在（bad=0）；
  - `重复目录/聊斋 (1996) {tmdb-68678}/` 现在只剩 Season 01。
- **端到端实测**（新镜像 `lux:phase-a` = `2897000ddab9`，独立容器 + 独立配置副本，poll=0 手动触发）：
  - 合成两季包 `刑事侦缉档案4 (1999) {tmdb-210560}`（Season 04 已在库、Season 05 为新季）放入 `待整理/`；
  - `POST /api/v1/admin/archive/run?dry_run=false` → `organized=1 (Season 05)`、`duplicates=1 (Season 04)`、`failed=0`；
  - 核验：库里出现 `Season 05`，`重复目录/…{tmdb-210560}/` 只有 `Season 04`，包目录已消失，
    镜像 `Season 05/Test.S05E01.strm` 指向 `…/Season 05/Test.S05E01.mkv`；
  - 对照：同一包在**旧镜像**上整包判重复（`lux-testlib` 日志 05:51:38 `duplicates:1`），即修复前的行为；
  - 测试产物已清理，`lux-testlib` 已用新镜像重建并 healthy（8099）。
- 规范：`docs/archive-pipeline.md` 新增 §5.1「剧集包按季判定」。
- 顺带发现（归 ingest 组，见 I16）：文档写 `dry_run=1|0`，接口只接受 `true|false`，`?dry_run=1` 返回 400。

---

## 模块：设置（worktree `lux-settings` / 分支 `feat/notify-settings`）

状态: CHANGES
分支 / 提交: `feat/notify-settings` @ 1b90afb4（包含 `feat/settings` @87a77c48 的 4 个提交 + F2 补丁 24090385）
逐条:
- ✅ F2 单文件挂载回退（EBUSY/EXDEV 双覆盖 + WARN + 0600）— `settings.rs:562-577`
- ✅ F1 配置优先级（`archive.rs:118-130`，轮询每轮重读）
- ✅ F8 chat_id 打码统一（`notify/config.rs:13` 复用 `mask_secret`）
- ❌ F4 provenance **未实现**（全仓无 provenance / present_keys，`save_to` 仍全量序列化）
必须修（合并前）:
- S1 [严重] PUT 越界数值被 `sanitized()` 抢先钳制后返回 200（`settings.rs:269,315-327`）— 按 D2 直接 400
- F4/S2 [严重] `save()` 写回 env 派生值，UI 保存一次即把 env 值固化进文件
- S3 [严重] `telegram.api_key` 不存在于 `TelegramSettings`，整份重写时被抹掉 → 旁挂启动失败（按 D3 加字段）
- S5 [一般] 对象型 `listen_channels` 被 `string_list_field` 丢弃，逐频道 `mode/follow_list` 随 UI 保存清空
- S7 [一般] 并发 PUT 无锁（read-modify-write 丢更新；原地写两次 truncate 可互相截断）
其余: S4/S6/S8/S9/S10/S11/S12，见 `fix-settings-round2.md`
测试: `cargo test --locked --lib archive` 34 通过（审核组在合并后的 trunk 上实跑）

---

## 模块：通知（分支 `feat/notify-settings`）

状态: CHANGES
分支 / 提交: `feat/notify-settings` @ 1b90afb4（80ef8046 / 1401b09d / 191dd852 / 814fe434 / 2e15f133）
逐条:
- ✅ A 配置优先级（只消费 `ArchiveSettings::load()`，env 只补缺）
- ✅ C 重复事件（`duplicate` 类别 + 逐条 + 汇总计数）
- ✅ E 全部小项（api_base 打码、chat_id 统一、3900 UTF-16、播放上下文短路）
必须修（合并前）:
- N1 [严重] 设置页类别清单缺「重复」（`ArchiveSettingsSection.tsx:16-27`）→ UI 保存一次就把
  `notify:null`（全开）写成 10 项数组，duplicate 被永久静默关闭；按 D5 改后端下发
- N2 [一般] 同一处把对象形态归一成数组，丢掉「未列出=开启」语义（`{"playback":true}` → 只剩播放）
- N3 [一般] 播放通知「IP」恒为空（三个调用点只在 stopped 时传 remote_ip，而通知只发 started）
- N4 [一般] 失败事件无 dedupe_key、无跨轮预算，转存失败每轮重发同一批
- N5 [一般] 去重键在发送成功前写入（`mod.rs:175-189`），发送失败反被抑制一个窗口
其余: N6–N13，见 `fix-notify-round2.md`
测试: `cargo test --locked --lib notify` 40 通过（审核组实跑）

---

## 模块：旁挂（worktree `lux-telegram-sidecar` / 分支 `feat/telegram-sidecar`）

状态: CHANGES（F0 并发冲突已解除：`sidecar/` 已在 `feat/telegram-sidecar` 提交、只有一个写入者。
注意：主 checkout `/vol5/1000/build/lux` 里还有一份**未跟踪**的旧 `sidecar/` 副本，
合并时以 `lux-telegram-sidecar` worktree 为准，不要把那份旧副本 `git add` 进去。）
分支 / 提交: `feat/telegram-sidecar` @ 4ac4e1fa（3b8d2ec9 / f2ff9698 / 0fdcbf14 / 42529388 / 4ac4e1fa）
逐条:
- ✅ F1 投递链路（默认 http → `/archive/ingest`，spool 已删）
- ✅ F2 session 0600 / 目录 0700 / 非 root
- ✅ F3 真实手机号与 api_id 已清（全树 grep 无真号）
- ✅ F4 最新 N 条回填、F5 catch_up+游标、F6 损坏 session 干净报错、F7 4K/8K、F8 lookup 认证头
- ✅ F9 打包（`.dockerignore`、无 session 时 healthcheck 非零）
必须修（合并前）:
- SC1 [严重] 旁挂读 `telegram.api_key`，Lux 设置模型没有该字段（与 S3 同源；按 D3/D4 解决）
- SC2 [严重] Lux 写 settings 固定 0600，旁挂以 uid 10001 只读挂载读不到 → 启动即 exit 2（按 D4 解决）
- SC3 [一般] `session_errors` 把 `OperationalError`（无权限/磁盘满）也报成"session 损坏"
- SC4 [一般] 永久失败（400/403）无死信，每 300s 重试到永远
- SC5 [一般] 默认放行 `other`，任意 http 链接都进 `链接队列/`
- SC6 [一般] payload 缺 `source/kind/channel/message_id/size_bytes`，状态文件无法对账
其余: SC7–SC18，见 `fix-sidecar-round2.md`
测试: `python -m unittest discover -s tests -t .` 123 通过 / 1 skip（审核组在临时副本实跑）

---

## 模块：归档 / Rust 侧 ingest（分支 `feat/telegram-ingest`）

状态: CHANGES
分支 / 提交: `feat/telegram-ingest` @ 43f152b1（f45a9ecf + d0271c43 + 43f152b1）
逐条:
- ✅ 契约：`POST …/ingest`（202/400，按 URL 幂等）、`GET …/lookup`、`status.pending_link_jobs`
- ✅ 鉴权与 CSRF 分支顺序、路径穿越防护（slug 白名单 + 生产方 jobId 被覆盖）
- ✅ 消费器不下载、不写媒体目录；`无效/` 隔离；状态文件 0600
必须修（合并前）:
- I1 [严重] 队列消费的 `append_link_state(...)?` / `move_entry(...)?` 冒泡到 `run_once`（队列在最前）
  → 单条作业 IO 失败会让整轮归档不执行，且状态已追加、移动失败的作业下一轮重复记账
- I2 [一般] ingest 的 check-then-write 无锁；`unique_target` 让文件名与 `jobId` 不一致
- I3 [一般] 作业文件 0644 vs 状态文件 0600（同一条含 `?password=` 的链接两处权限不同）
- I4 [一般] 日志明文打印完整分享链接（含 password），违反 D7
- I5 [一般] lookup 名称启发式误匹配（`流浪地球2`↔`流浪地球`、短标题、跨年份；匹配还发生在类型/地区/年份层）
- I6 [一般] lookup 每请求全树遍历 + 递归求和，无缓存；跟随符号链接；在 async handler 里同步阻塞
其余: I7–I15，见 `fix-ingest-round2.md`
新增: I16 [小项] `run` / `metadata` 的 `dry_run` 文档写 `1|0`，接口是 bool（`?dry_run=1` → 400
`provided string was not true or false`）——改文档或兼容 `1/0/true/false`（审核组实测踩到）
测试: `cargo test --locked --lib archive` 34 通过（含 F1 新增用例）

---

## 下一轮期待

1. 四个工作组各修「必须修」项，逐条一个提交（信息带编号，如 `fix(settings): S1 reject out-of-range PUT`）。
2. 改完更新本文件自己那节：`状态: READY` + 每条 `✅ <提交哈希> 验收证据` + 测试结果 + 遗留。
3. 审核组按 D10 复核；PASS 后由总控合并、重建 `lux:phase-a`、更新 `PROGRESS.md`。
