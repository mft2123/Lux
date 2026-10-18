# 旁挂组整改单（第二轮 · 2026-10-18）

审核对象：`feat/telegram-sidecar` @4ac4e1fa 的 `sidecar/telegram/**`（main.py、tgwatch/*、tests/*、
Dockerfile、compose.fragment.yml、README）。
决策见 `REVIEW-BUS.md` D1–D10。

结论：**CHANGES**。上一轮 F1–F9 在代码层全部落地（并有可离线跑通的断言），
但剩 2 条部署级阻塞和 4 条一般项。F0 并发冲突已解除：`sidecar/` 已提交、只有一个写入者。

## 一、必须修（合并前）

### SC1 [严重] Lux 管理 key 没有归属，保存设置即丢失
- 位置：`tgwatch/config.py:265,309-313`（读 `telegram.api_key`，缺失即 `ConfigError`）、
  `compose.fragment.yml:22-33`（没设 `LUX_TG_API_KEY` 兜底）
- 对端：Lux `TelegramSettings` 无 `api_key` 字段（设置组 S3 修，按 D3）。
- 本组要做的：`LUX_TG_API_KEY` 作为**等价来源**（settings 里的值优先，env 兜底），
  启动日志里只打印打码值；compose 注释写明两种来源。
- 验收：只给 env、只给 settings、两者都给（settings 优先）三种情况都能启动；
  `main.py status` 不打印明文 key。

### SC2 [严重] 设置文件 0600 与 uid 10001 冲突
- 位置：`compose.fragment.yml:35`（`:ro` 挂载 `/vol5/1000/build/archive-settings.json`）、`Dockerfile:24`（`USER 10001`）
- 对端：设置组按 D4 在保存后 `chmod 0640` + `chgrp $LUX_ARCHIVE_SETTINGS_GROUP`；本组 compose 加
  `group_add: ["${LUX_ARCHIVE_SETTINGS_GROUP:-10001}"]`。
- 本组要做的：`config.py` 读到权限错误时，报「设置文件不可读：检查属组/权限（README 第 X 节）」而不是笼统 ConfigError；
  README 增一节部署检查清单。
- 验收：`ls -l` 为 `640 <lux>:<gid>`；旁挂以 10001 启动成功；把权限改回 0600 时错误信息给出可操作提示。

### SC3 [一般] `session_errors` 把一切 sqlite 错误都报成"session 损坏"
- 位置：`tgwatch/runner.py:191-204`
- 改法：只有 `DatabaseError` 且消息含 `not a database` / `malformed` 才认定损坏；`OperationalError`
  原样带 errno 报出（无权限、磁盘满、被锁）。
- 验收：新增三条用例（垃圾文件 / 无写权限 / 只读目录），错误文案各不相同。

### SC4 [一般] 永久失败无死信，无限重试
- 位置：`tgwatch/runner.py:366-400`（`retry_loop` 无计数）、`sink.py:93-97`
- 改法：400/403 直接置 `dead` 并记录原因（不再重试）；其余可重试错误设上限（如 20 次）+ 指数退避；
  `held` 保持人工。状态库里能看到 `dead` 计数与原因。
- 验收：构造 400 与 403 各一条 → 只尝试一次、落 `dead`、`status` 可查；网络错误重试到上限后落 `dead`。

### SC5 [一般] 默认放行 `other`，任意 http 链接进队列
- 位置：`tgwatch/links.py:47-55,137`
- 改法（按产品口径，二选一并在 README 写明）：默认只收已知网盘/磁力；
  若保留 `other`，payload 必须带 `kind`，让 Lux 侧可过滤统计。
- 验收：一条新闻链接默认不投递（或 `kind=other` 明确可见）。

### SC6 [一般] payload 缺溯源字段
- 位置：`tgwatch/sink.py:36-42`；Lux 端支持 `source/kind/channel/message_id/size_bytes/captured_at`
- 改法：把 item 里已有的字段补齐；`test_sink` 断言完整 body。
- 验收：`状态.jsonl` 每条记录的 `source/kind/channel/message_id` 非空。

## 二、小项（合并后可排期）

- SC7 多链接共享一个 title，洗版只查一次库（`runner.py:267,286-299,320-334`）→ 逐链接取标题，或降级 `held`。
- SC8 `status` 打印原始 `ingest_url`（`main.py:90-93`、`sink.py:117-118`）→ 用 `mask_url`（D7）。
- SC9 healthcheck 只验配置就绪（`config.py:378-387`）→ 增加 `is_user_authorized()` 深检或心跳文件。
- SC10 只钉 `telethon==1.45.0`（`requirements.txt:2`）→ 生成 lock 或 `--require-hashes`；基础镜像钉 digest。
- SC11 关停顺序：`store.close()` 后后台任务仍可能写库（`runner.py:417-419,449,576-584`）→ 先 cancel+gather。
- SC12 去重键 `chat_id:msg_id`，backfill 缺 chat_id 时退化成 `0:<id>`（`runner.py:61-63,88-97`）→ 用 `channel:msg_id`。
- SC13 测试保真度：F6 手工 raise；权限用例 Windows skip；`api_smoke` 未覆盖 `catch_up`/`min_id`。
- SC14 注释/文档残留：`links.py:250`「spool 文件名」、`README.md:28`「800/600」、`Dockerfile:7` 无效 `ENV UMASK`。
- SC15 不做 HTML 实体反转义，`&amp;` 原样进 URL（`links.py:65-68`）。
- SC16 `phone` 不校验，首次登录缺 phone 抛非 RunnerError（`config.py:281`、`runner.py:176-180`）。
- SC17 `tests/test_logsafe.py:32` 仍有一个形如大陆手机号的常量（假号）→ 换 `+10000000000`，让 grep 验收可机械通过。
- SC18 `deliver` 只捕获 `SinkError`，序列化异常不落 `failed`（`runner.py:365-374`）→ 兜底 `except Exception`。

## 三、已核实无问题（不用重做）

- 投递链路：默认 `LUX_TG_SINK=http` → `/api/v1/admin/archive/ingest`；spool 已从全树删除；inbox 仅显式诊断用。
- F2：`umask(0o077)` 先于建库；登录后 `chmod 0600/0700`；容器 `USER 10001`。
- F3：全树 grep 无真实手机号 / api_id；README 与测试都用假值（除 SC17 的假号形态）。
- F4：`iter_messages(limit=N)` 不带 reverse，取最新 N 条，有断言。
- F5：`catch_up()` + 每频道游标 `MAX()` 单调 + `min_id` 补拉。
- F6：`RunnerError` → `exit 2`，不打印 traceback。
- F7：`4K/8K/8M` 不算体积；体积未知 → `held`。
- F8：lookup 带 `X-Lux-Api-Key`，响应 `{title,matched,size_bytes}` 与 Lux 逐字段对齐，旧的 `verdict` 期望已不存在。
- 契约对照（路径/方法/202/body/认证/400·401·500/幂等/lookup）全部一致；URL 协议两侧相同。
- 状态库：单连接 + 锁 + WAL + `UNIQUE(msg_key,url)`；SIGTERM 取消主任务后断开，配合 Lux 幂等不会重复入队。
- 离线测试：`python -m unittest discover -s tests -t .` → 123 passed / 1 skipped（审核组实跑）。

## 四、汇报格式

逐条一个提交（`fix(sidecar): SC3 classify sqlite errors`）。完成后更新 `REVIEW-BUS.md` 自己那节。
测试要求：`python -m unittest discover -s tests -t .` 全绿；一次真实登录用 `LUX_TG_DRY_RUN=1` 跑通
（F4/F5 的 `iter_messages`/`catch_up`/`min_id` 无法离线验证，必须在 NAS 测试容器核对一次）。
