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

状态: READY
分支 / 提交: `feat/telegram-sidecar`（worktree `lux-telegram-sidecar`，基线 `fork/phase-a` @ 43f152b1）
  提交：`3b8d2ec9` `f2ff9698` `0fdcbf14` `42529388`
逐条:
- ✅ F0 唯一写入者 — `sidecar/**` 只存在于本分支；本 worktree `git status` 干净；
  主 worktree 里残留的未跟踪副本未再改动（由总控合并时清理）— 证据: 本分支 `git ls-files sidecar | wc -l` = 26
- ✅ F1 投递链路 — `f2ff9698`：默认 `LUX_TG_SINK=http`，POST `/api/v1/admin/archive/ingest`，
  body `{url,title,mode}`，带 `X-Lux-Api-Key`（设置文件 `telegram.api_key`）；`inbox` 降级为诊断
  — 证据: stub Lux 契约冒烟 6/6（路径/body/鉴权头/lookup 查询串与解析）
- ✅ F2 session 0600/0700/非 root — `0fdcbf14` + `42529388`：`umask 077`、登录后 `chmod 0600`、
  目录 `0700`、镜像 `USER 10001`、compose `LUX_TG_UID/GID` 对齐设置文件属主
  — 证据: 容器内 `session 0o600 / dir 0o700 / uid=1000`
- ✅ F3 真实手机号清除 — `f2ff9698`：假 api_id `123456`、假号码 `+10000000000`
  — 证据: 用真实号码与 api_id 做全仓 grep → 0 命中（本文件故意不写这两个字面量，免得文档自己成为命中源）；
    历史提交里也没有，因为首个提交就是假值
- ✅ F4 全量回填最新优先 — `0fdcbf14`：`iter_messages(limit=N)` 取最新 N 条，`reversed()` 后按时间处理
  — 证据: `tests/test_runner.py::BackfillTest`（断言无 `reverse`、按 1,2,3 处理）
- ✅ F5 catch_up + 游标补拉 — `0fdcbf14`：`client.catch_up()` + `channel_state.last_message_id` 的 `min_id` 补拉
  — 证据: `tests/test_runner.py::GapBackfillTest`、`tests/test_store.py` 游标用例
- ✅ F6 损坏 session — `0fdcbf14`：`sqlite3.Error` → `RunnerError("session 文件损坏…")`，exit 2
  — 证据: `tests/test_runner.py::SessionTest::test_corrupt_session_becomes_runner_error`
- ✅ F7 4K/8K — `3b8d2ec9`：体积正则只认 GB/TB/MB/KB；`4K/8K/8M` 不产生体积，洗版未知 → `held`
  — 证据: `tests/test_links.py`、`tests/test_filters.py::WashTest`
- ✅ F8 洗版认证头 — `3b8d2ec9`（判定表）+ `f2ff9698`（`GET …/archive/lookup` 带 `X-Lux-Api-Key`）
  — 证据: `tests/test_sink.py::LookupTest`（断言请求头与查询串）、`WashTest`
- ✅ F9 打包 — `42529388`：`.dockerignore`、healthcheck 改 `check`（无 session → 非零）、inbox 0644
  — 证据: `docker run … check` 无 session → exit 1；缺 api_key → exit 2
测试:
- `python3 -m unittest discover -s sidecar/telegram/tests -t sidecar/telegram` → 123 passed（+1 skip: POSIX-only）
- `cargo test --locked --lib archive` → 24 passed
- `docker build -f sidecar/telegram/Dockerfile -t lux-telegram:1 sidecar/telegram` → ok
- stub Lux 契约冒烟 → 6/6
遗留:
- 115 转存客户端（U11）未实现，链接只入队并标 `needs_115_client`
- 端到端验收（测试频道发链接 → `链接队列/` 出现 json）需真实 Telegram 登录 + 已部署含 ingest 的 Lux，
  由总控合并后实测；旁挂侧已按契约验证
  

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
