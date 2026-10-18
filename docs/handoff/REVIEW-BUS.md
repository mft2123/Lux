# 评审总线（REVIEW-BUS）

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

## 模块：通知（feat/notify-settings）

状态: READY
分支 / 提交: `feat/notify-settings` @ `10cb5e47`（基于 `feat/settings` 87a77c48，已合并 `fork/phase-a`）
逐条:
- ✅ F1 env 覆盖文件 → 改为消费 `ArchiveSettings::load()`，文件优先、`LUX_TG_*` 只补缺失键 — `80ef8046`
  证据: 单测 `file_values_win_over_environment`、`environment_only_fills_missing_keys`、`environment_notify_list_is_used_only_when_the_file_has_none`
- ✅ F2 单文件 bind mount PUT → `rename` 遇 EBUSY/EXDEV 退化为原地写（截断重写 + 0600 + WARN），其他错误照常透传 — `24090385`
  证据: 单测 3 个（fallback 内容/权限、非 bind 错误透传、EBUSY/EXDEV 分类）+ 单文件挂载 E2E `10cb5e47`
  E2E: `-v /vol5/1000/build/bind-settings.json:/tmp/archive-settings.json` + `LUX_BIND_MOUNT_TEST_PATH=/tmp/archive-settings.json`，`cargo test --lib bind_mount_save_end_to_end -- --ignored` → 1 passed，宿主文件内容更新且仍 600
  备注: 挂载文件必须对容器运行用户可写（生产为 root:root 600）；首次尝试用 qiangli 600 的宿主文件会 EACCES，属预期权限错误
- ✅ F3 重复事件 → 新增 `duplicate`/`重复` 类别逐条通知（受开关控制），汇总保留计数 — `1401b09d`
  证据: 单测 `archive_report_produces_one_event_per_item_and_a_summary`、`duplicate_event_carries_the_existing_path`
- ✅ F5 api_base 打码、chat_id 统一 `head6…(len)`、3900 上限按 UTF-16 码元 — `191dd852`
  证据: 单测 `api_base_drops_credentials_and_paths`、`chat_id_uses_the_settings_mask_format`、`emoji_count_as_two_utf16_units_and_stay_within_the_limit`
测试:
- `cargo test --locked --lib notify` → 40 通过
- `cargo test --locked --lib archive` → 46 通过（另 1 ignored 即 bind-mount E2E）
- `cargo test --locked --lib playback` → 97 通过
遗留:
- F4（save 不落 env 派生值）属设置模块，本分支未改
- 未做真实 TG 发送（测试库无 settings 文件；需审核方许可后执行）
新意见（审核方）: 无（F1/F2/F3/F5 已按上表处理，等待复审）

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
