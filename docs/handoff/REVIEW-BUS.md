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


## 第三轮：审核组接手实现（2026-10-18）

磨合不畅，审核组从本轮起**同时承担实现**：在原有分支上改代码、跑测试、建镜像、合并。
工作分支 `integration/round2`（基于 `fork/phase-a`，已合并 settings/notify/sidecar 三个模块分支），
按模块逐条提交，提交信息保留原编号。

---

## 模块：设置 — ✅ READY（`integration/round2`）

提交：`2a84ec99`（S1/S2/F4/S3/S4/S5/S10/S11/S12）、`6eadd2d1`（S7 写锁）
逐条:
- ✅ S1 越界值 400：`apply_update` 先在副本上合并并校验，失败不落地；`sanitized()` 不再抢先钳制
- ✅ S2/F4 provenance：`document_for_save` 只写「文件已有 + 与 env 不同」的键；env 值不再被固化
- ✅ S3 `telegram.api_key` 字段（打码/留空保留/前端密码框），旁挂不再因保存设置而丢 key
- ✅ S4 原地写前备份 `*.json.bak`，失败回滚；临时文件先 chmod 再 rename
- ✅ S5 对象型 `listen_channels` 与未知键在保存时原样保留（`existing` 为底稿合并）
- ✅ S7 PUT 进程内串行（`ARCHIVE_SETTINGS_WRITE_LOCK`）
- ✅ S10 `api_base` 打码；S11 类型错误 400；S12 只校验本次改动涉及的路径
- ⏭️ S8 临时文件名 pid+自增（跨进程不可撞，未再加随机后缀）；S6 由后端下发类别清单解决
测试：`cargo test --locked --lib archive` → 71 通过（含 settings 8 个、季判定 3 个、ingest 9 个）

---

## 模块：通知 — ✅ READY（`integration/round2`）

提交：`4c979394`（N3/N4/N5）、`6732754b`（N1/N2 + api_key 输入）、`?`（N12 Debug 打码，见本轮末提交）
逐条:
- ✅ N1 设置页类别由后端 `known_categories` 下发（含「重复」），不再硬编码
- ✅ N2 未改动的保存回传原形态（`null`/对象），只有勾选过才写数组
- ✅ N3 PlaybackStarted 也带 `remote_ip`（三个上报点统一）
- ✅ N4 转存/整理/洗版/重复/失败事件都带 `dedupe_key`，坏条目每窗口只提醒一次
- ✅ N5 去重键改为发送成功后写入，失败不再自我抑制
- ✅ N12 `NotifyConfig` 手写 Debug，token/chat_id/api_base 打码
- ⏭️ N6/N7（行内截断、字段预算）、N9、N10、N11、N13 记录在案，留待下一轮
测试：`cargo test --locked --lib notify` → 41 通过；`--lib playback` → 97 通过

---

## 模块：旁挂 — ✅ READY（`integration/round2`）

提交（模块方）：`3b8d2ec9` `f2ff9698` `0fdcbf14` `42529388` `4ac4e1fa`；
审核组补的测试/实现：`db03e57c`（测试清理 + inbox 0644）
逐条:
- ✅ SC3 session 错误分类（只有 DatabaseError 损坏才叫"损坏"）
- ✅ SC4 400/403 落死信、其余 20 次退避重试、`status` 可查 dead、`replay --id` 放行
- ✅ SC5 默认只收已知网盘/磁力（`LUX_TG_ALLOW_OTHER=1` 才收 other）
- ✅ SC6 payload 补齐 `source/kind/channel/message_id/size_bytes/captured_at`
- ✅ SC1（旁挂侧）key 文件优先、env 兜底；SC2（旁挂侧）`group_add` + 部署检查清单 + 权限错误提示
- ✅ SC8 status 打码、SC14 文档/注释、SC15 HTML 实体、SC16 phone 校验、SC17 假号码、SC18 deliver 兜底、SC11 关停顺序
- ⏭️ SC7/SC9/SC10/SC12/SC13 保留（理由见模块方汇报）
测试：`python3 -m unittest discover -s tests -t .` → **184 OK**（3 条权限用例在 ZFS 上 skip）

---

## 模块：归档 / ingest — ✅ READY（`integration/round2`）

提交：`75410572` I1、`9953f560` I3、`d4f60545` I4、`19b13ea5` I5、`eb2a3f9c` I2、`e7d3f6c5` I6、
`8b14ec6a` I8、`981356c1` I9、`ca02a8e4` I10、`c2df166c` I11、`76583ad0` I16、`6f4e6d23` I12；
审核组补：`?`（I6 的 `spawn_blocking` 调用点，`admin_jobs.rs`）、`2718341e`（I16 的 `1|0` 兼容解析）
逐条:
- ✅ I1 队列消费逐条 best effort：写状态/移动失败只记账并继续，不再拖垮整轮；记账幂等（下一轮只重试移动）
- ✅ I2 进程内锁包住「查重+落盘」；撞名时 jobId 与文件名一起加后缀，202 三者一致
- ✅ I3 作业文件/状态/已处理统一 0600
- ✅ I4 `mask_link_for_log`：日志只留 scheme://host
- ✅ I5 lookup 只匹配标题层、年份一致、短标题精确、续集不误配（4 组用例）
- ✅ I6 walk 深度上限 + 不跟随符号链接 + 60s 缓存 + `spawn_blocking`（72k 条目 bench：冷 137ms / 命中 5.9µs）
- ✅ I8 `mode` 只认 5 种；I9 作业判定共用；I10 URL 必须有 host/target；I11 全角归一；I12 读失败不再当"没有重复"
- ⏭️ I7（幂等 TTL/轮转）、I13（202 状态语义）、I14（duplicate 的 file 路径）、I15 已由审核组在 admin_jobs 完成
测试：`cargo test --locked --lib archive` → 71 通过（2 ignored 为手工 bench）

---

## 已知的 trunk 问题（非本轮引入）

- `application::metadata::tests::scan_local_nfo_page_flushes_actor_credits_and_isolates_invalid_nfo`
  在 `fork/phase-a`（`385c421e`）上单独跑也失败：断言哪个条目失败依赖内部顺序，属于既有不稳定用例。
  本轮所有模块测试（archive/notify/playback、sidecar）不受影响；整库 `cargo test --lib` 因此仍报 1 failed。
