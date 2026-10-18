# 评审总线（REVIEW-BUS）

## 总控交接（2026-10-18）

原总控对话框上下文用尽，审核职责移交**审核组对话框**。接手方开工先读：
`WORKFLOW.md`（规则）、本文件（状态）、`PROGRESS.md`（进度）、`fix-*.md`（各模块整改清单）。

---

## 待办 F1【严重】剧集包必须按季判定（新发现，最高优先）

- 现象：`重复目录/聊斋 (1996) {tmdb-68678}/` 同时含 Season 01、Season 02，而库中只有 Season 01；
  整包被当作"重复"搬走，导致**库里缺 Season 02**
- 根因：`src/application/archive.rs` 的入库判定以**整个目录包**为单位（`decide_existing` 比整包体积），
  没有按子目录（Season NN）逐个比对
- 要求：
  1. 剧集包（`matched.kind == Series` 或目录含 Season 子目录）按**每个 Season 子目录**分别判定：
     库中无该季 → 合并入库；库中有 → 体积比（等效=重复，明显更大=洗版该季）
  2. 只有真正重复的季进 `重复目录/`，其余入库；镜像库同步补该季的 strm/NFO
  3. 电影包保持整包判定不变
  4. 单测：两季包、库中只有一季 → 一季入库、一季重复；两季都在库且等效 → 全重复
- 验收：测试库中用聊斋两季包重跑，库中出现 Season 02、重复目录只留 Season 01；镜像同步
- 现状修复：把 `重复目录/聊斋 (1996) {tmdb-68678}/Season 02` 合并回
  `媒体目录/电视剧/国内/1996/聊斋 (1996)/Season 02`，并重新生成该季镜像

---

## 模块：设置（feat/settings，worktree lux-settings）

> 规则见 `WORKFLOW.md`。任何状态变化都要提交，提交信息 `bus(<模块>): ...`。
> 模块方开工前先看自己那节；审核方每轮更新结论。

---

## 模块：设置（feat/settings，worktree lux-settings）

状态: CHANGES
分支 / 提交: `feat/settings` @ 87a77c48（4 提交，可直接快进合并）
逐条:
- ✅ 配置读写、打码、PUT 校验 — 单测 12 个
- ⏳ F2 单文件 bind mount 下 PUT 500（EBUSY）
- ⏳ F1 env 优先级（见通知模块 A 条，需一起改）
- ⏳ F4 PUT 不落盘 env 派生值
测试: `cargo test --locked --lib archive` 36 通过
遗留: 见 `docs/handoff/fix-notify-settings.md` A–E
新意见（审核方）:
- F2 [严重] `settings.rs:492-519` rename 覆盖单文件挂载点 → EBUSY；改为原地写回退 — 验收: 单文件挂载下 PUT 返回 200 且文件仍 600
- F4 [严重] `save()` 把 env 派生值写进文件；只写文件自有字段 — 验收: 仅 env 设置 watch_dirs 时，UI 保存后文件中不出现该值
- F8 [小项] 与通知模块统一 chat_id 打码格式

---

## 模块：通知（fork/phase-a 上已合并，ad8502b2）

状态: CHANGES
分支 / 提交: 直接在 `fork/phase-a`（`ad8502b2`）
逐条:
- ✅ 实测发出消息（messageId 48311）、类别过滤、长度截断、非阻塞
- ⏳ F1 env 覆盖文件（违反约定）
- ⏳ F2 单文件挂载（同设置）
- ⏳ F3 重复事件缺失（需决定）
新意见（审核方）:
- F1 [严重] `notify/config.rs:304-343` `apply_env` 无条件覆盖；env 只补缺失，并改 `module-notify.md:59` — 验收: 文件与 env 同时设置时用文件值
- F3 [一般] 增加 `duplicate` 类别逐条通知，或文档写明不做
- F5 [小项] `api_base` 未打码；两处 chat_id 打码格式不一致；3900 上限按 UTF-16 计

---

## 模块：旁挂（feat/telegram-sidecar，sidecar/telegram）

状态: BLOCKED（并发写入冲突未清）
分支 / 提交: 无（sidecar/ 仍是未跟踪状态；两个版本互相覆盖过）
逐条:
- ⏳ F1 投递链路（Lux 侧 ingest 已就绪：feat/telegram-ingest）
- ⏳ F2 session 0600/0700/非 root
- ⏳ F3 **真实手机号仍残留**：`sidecar/telegram/tests/test_config.py:162`（提交前必须清）
- ⏳ F4 全量回填最新优先；F5 catch_up；F6 损坏 session；F7 4K/8K；F8 洗版认证头
新意见（审核方）:
- F0 [阻塞] 目录只有一个写入者：请旁挂对话框全权负责 `sidecar/**`，先读 `fix-telegram-sidecar.md`，不要回退已评审版本 — 验收: `git status` 中 sidecar 版本唯一、F1–F8 均有提交
- F3 [严重] 清除真实手机号后再提交 — 验收: 全仓 grep 真实号码/ api_id 为 0 命中

---

## 模块：Rust 侧 ingest（feat/telegram-ingest）

状态: READY（审核方已初查，待合并）
分支 / 提交: `feat/telegram-ingest` @ d0271c43（f45a9ecf + d0271c43）
逐条:
- ✅ `POST /api/v1/admin/archive/ingest`（202/400/重复幂等）、`GET .../lookup`、`pending_link_jobs`
- ✅ 消费器 `ingest_link_queue`：状态 0600、无效任务隔离、不下载
- ✅ `cargo test --locked --lib archive` 31 通过（连续 3 轮）
新意见（审核方）:
- F9 [一般] 合并前先修 `docs/handoff/module-telethon.md` 的"未改动 Rust"表述（与事实冲突）
- F10 [小项] job 文件 0644 与状态文件 0600 不一致；URL 幂等无过期；并发 POST 无锁；`mode` 未校验 5 种取值
