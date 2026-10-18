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
- ✅ S8 临时文件名 pid+自增+纳秒 nonce（跨进程也不会撞）；S6 由后端 `known_categories` 解决（`aaad6826`）
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
- ✅ N6/N7 截断不再切断 HTML 实体、字段按转义后码元封顶 400（`aaad6826`）
- ✅ N8 `telegram.notify` 是字符串时 WARN 一次并当 `null`（`1d6783f5`）
- ✅ N9 设置读失败每进程只 WARN 一次、N10 `api_base` 校验（禁 query/fragment）
- ✅ N11 归档中途中止会发一条去重的失败通知、N13 文档改为「准入串行」
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
- ✅ SC7 多链接逐链接取标题（缺标题的链接 held/title_ambiguous）— `d6c77749`
- ✅ SC9 `check --deep` + Dockerfile healthcheck 深检 — `60465bf6`
- ✅ SC10 依赖全钉（telethon/pyaes/rsa/pyasn1/python-socks）— `d68a1c86`
- ✅ SC12 去重键统一 `channel:msg_id`（旧键不再命中，最坏重复一次，Lux URL 幂等兜底）— `4481cd34`
- ✅ SC13 真 sqlite/client 损坏用例 + api_smoke 覆盖 catch_up/min_id — `35fd9219`
测试：`python3 -m unittest discover -s tests -t .` → **207 OK**（tgvenv 下 3 skip 为 ZFS 权限用例）

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
- ✅ I7 `force: true` 重新入队已消费的 URL（幂等键不设 TTL，这是显式重试口）— `aaad6826`
- ✅ I13 202 的 `status` 改为入队态 `queued` — `aaad6826`
- ✅ I14 状态记录的是实际落盘文件名（`job_file`），duplicate 响应的路径一定存在 — 由 ingest 侧实现，审核复核
- ✅ I15 鉴权先于 body 解析 — `6eadd2d1`
测试：`cargo test --locked --lib archive` → 71 通过（2 ignored 为手工 bench）

---

## 既有 trunk 问题：已修（第四轮）

`scan_local_nfo_page_flushes_actor_credits_and_isolates_invalid_nfo` 早于本轮就失败/不稳定，第四轮定位到两个原因：

1. **真 bug**：演员 credit 分块提交失败后，`people.json` 关系快照仍是「当前」，而 credit 已回滚 →
   之后任何扫描都不会再补这些 credit（静默丢失）。现在失败分块会为每个受影响条目失效关系快照，下一轮会重做。
2. **并发锁**：NFO 富化并发执行时，people index 的短读改写事务会被 WAL 以 `SQLITE_BUSY_SNAPSHOT` 立即拒绝
   （busy_timeout 不生效）。持久化现在对该瞬时错误重试 3 次。
3. 测试自身还依赖「扫描器是否已预写关系快照」，现在测试先清掉快照，让两部电影确定都需要同步。

整库 `cargo test --locked --lib` → **1080 passed / 0 failed**（26 ignored 为手工 bench 等）。

---

## 第三轮收尾（2026-10-18 · 审核组）

第三轮把剩余小项全部收掉：S8；N6/N7/N8/N9/N10/N11/N13；SC7/SC9/SC10/SC12/SC13；I7/I13/I14。

- 代码：`aaad6826`（N6/N7/N9/N10/N11/N13/I7/I13/S8）、`1d6783f5`（N8）、旁挂 `d6c77749`…`35fd9219`。
- 测试：archive 73、notify 43、playback 97、旁挂 **207**（tgvenv 下 3 skip 为 ZFS 权限用例）；
  整库 `cargo test --lib` 仍只剩那个**既有** metadata 不稳定用例（trunk 同样失败）。
- 新增单测：实体截断 2 条、api_base 1 条、force 重投 1 条、旁挂 +23 条。
- 仍未做（明确边界）：U11（115 转存客户端）——不是整改项，是下一阶段的新功能。

## 运维工具：演员 credit 审计（第四轮）

`scripts/actor-credits-audit.py` 用来检查「关系快照说已同步、但 `person_credits` 是空的」这种历史残留：

- 只读模式列出 `MISSING_CREDITS` / `COUNT_MISMATCH` / `UNREADABLE`，以及没有关系快照的孤儿 credit；
  有问题退出码 1，干净退出 0，方便接监控。
- `--fix` 只删掉问题条目的 `people.json`（关系快照是缓存，不碰媒体与数据库行）。
- 修复由新接口完成：`POST /api/v1/admin/libraries/{id}/re-enrich`（admin + CSRF，202）。
  它给该库每个根排一次本地 NFO 重读，并**直接**用 `list_scan_local_metadata_sources` + 带 `people` 的 enricher
  重跑一遍（不走 image/backfill 流水线——那些只处理「还需要处理」的条目，正是丢 credit 的条目会被漏掉）。
  后台执行，幂等，跑完日志有 `library re-enrichment started/finished`。
- 普通 scan / reconcile 仍然不会重新富化未变化的条目（实测 touch 或改 NFO 后重扫都无效），所以修复走 re-enrich。

实测（测试实例，电影库 5 个有演员的条目 / 92 条 credit）：模拟 bug（删掉一个条目 20 条 credit）→ 审计报
`MISSING_CREDITS actors=20 credits=0`、exit 1；`--fix` 删关系；`POST …/re-enrich` → 20 条 credit 恢复，
审计回到 `OK / exit 0`；`--prune-stale` 把 4 个已删库遗留的关系目录清掉后，审计显示 `5 checked, 0 stale`。
