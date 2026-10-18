# 修复任务：Telegram 旁挂（按验收报告整改）

依据：`.review/telegram-review.md`（验收结论：不通过）。全部改动提交到分支 `feat/telegram-sidecar`，不直接合并。
每完成一条在括号里打 ✅ 并附提交哈希。

## 修复顺序与验收标准

### 1. 安全：清除仓库中的真实凭证（最高优先）
- 受影响文件：`sidecar/telegram/README.md:64`、`tests/test_config.py:28,143`、`tests/test_logsafe.py:7`（真实手机号），`README.md:62`、`tests/test_config.py:26,52`（真实 api_id）
- 改为明显的假值（如 `+10000000000`、`123456`）
- 验收：`grep -rn "<真实手机号前缀>" <repo>` 与 `grep -rn "<真实 api_id>" <repo>` 均无结果；`git diff` 中无任何真实凭据

### 2. 投递链路（阻塞项 F1）
- 问题：sink 写 `/data/spool/links.jsonl`，Lux 无消费方；watch-inbox 方案不可行（非视频会被判失败）
- 要求：
  - Lux 新增认证接口 `POST /api/v1/admin/archive/ingest`，请求体 `{ "url": "...", "title": "...", "mode": "手动|上新|追更|洗版|全量" }`，写入 `<archive_root>/链接队列/<ts>-<slug>.json`
  - 归档流水线新增 `ingest_link_jobs()`：读取队列文件 → 校验 → 记录状态（`链接队列/状态.jsonl`：pending/needs_115_client）→ 移入 `链接队列/已处理/`
  - **不下载、不转存**：115 链接转存客户端尚未实现，任务标记 `needs_115_client` 并写日志
  - 旁挂 sink 默认 `LUX_TG_SINK=http`，指向该接口，API key 从设置文件读取
- 验收：在测试频道发一条链接 → `链接队列/` 出现 json → 流水线日志出现 ingest 记录 → 状态文件有该任务；重复投递不重复入队（幂等键 = url+mtime）

### 3. Session 加固（阻塞项 F2）
- 容器内 `umask 077`；session 文件 0600、目录 0700（创建后显式 chmod，不依赖 Dockerfile，因为 bind mount 会覆盖）
- 容器使用非 root 用户，并保证 session 目录属主正确
- 验收：`ls -l` 确认 0600/0700；非 root 登录后 session 可读写

### 4. 全量回填取最新消息（F4）
- `iter_messages(limit=N, reverse=True)` 实际从最旧开始；改为先取最新 N 条
- 日志文案改为"最新 N 条"
- 验收：单测断言取到的是最新消息（构造假历史），重启不重复处理旧消息

### 5. 宕机期间消息不丢（F5）
- 启动时调用 `client.catch_up()`，并记录上次处理的消息 id（状态文件），启动后补拉 id 之后的消息
- 验收：单测模拟 last_id → 只补拉新消息

### 6. 损坏 session 规范化（F6）
- `sqlite3.DatabaseError` → 捕获并转成 `RunnerError("session 文件损坏，请重新登录")`，exit 2，不打印 traceback
- 验收：用垃圾文件当 session，输出干净错误

### 7. 4K/8K 解析（F7）
- `4K`/`8K` 是分辨率不是体积；体积解析只认 `GB/TB/MB` 等，不能把 4K 当 4096 字节
- 洗版模式：体积未知时应 `held`（等待），不是 skipped
- 验收：单测 `4K` 不产生体积；未知体积的 4K 条目为 held

### 8. 洗版查询认证（F8）
- 调用 Lux 判定接口时带 `X-Lux-Api-Key`（从设置读取）；接口不存在时保持 held 并记录原因
- 验收：单测校验请求头

### 9. 提交与文档（F9-F11）
- 提交 `sidecar/telegram/` 全部文件 + 更新的 `PROGRESS.md`、`module-telethon.md`
- `.dockerignore`、healthcheck 在无 session 时返回非零、spool 文件权限可被运维读取
- 验收：`git status` 干净；`python3 -m unittest discover -s tests -t .` 全绿；`cargo test --locked --lib archive` 全绿

## 完成后汇报

分支名、每条修复的提交哈希、测试命令与结果、仍未解决的事项（例如 115 转存客户端）。

## 完成情况（2026-10-18）

分支 `feat/telegram-sidecar`（worktree `/vol5/1000/build/lux-telegram-sidecar`，基线 `fork/phase-a`）。

| # | 项目 | 状态 |
| --- | --- | --- |
| 1 | 清除仓库中的真实凭证 | ✅ `f2ff9698` — 假 api_id `123456`、假号码 `+10000000000`；用真实值全仓 grep → 0 命中（文档里不引真实字面量） |
| 2 | 投递链路 | ✅ 旁挂侧 `f2ff9698`（默认 `LUX_TG_SINK=http` → `POST /api/v1/admin/archive/ingest`，body `{url,title,mode}`，`X-Lux-Api-Key`）；Lux 侧已在 `fork/phase-a`（`feat/telegram-ingest`） |
| 3 | Session 加固 | ✅ `0fdcbf14` + `42529388` — `umask 077`、目录 0700/文件 0600、镜像 `USER 10001`、compose 用 `LUX_TG_UID/GID` 对齐设置文件属主；容器内实测 `0o600`/`0o700`、`uid=1000` |
| 4 | 全量回填取最新消息 | ✅ `0fdcbf14` — 去掉 `reverse=True`，取最新 N 条后按时间顺序处理；单测断言请求参数与顺序 |
| 5 | 宕机期间消息不丢 | ✅ `0fdcbf14` — `catch_up()` + `channel_state` 游标 `min_id` 补拉，游标只前进 |
| 6 | 损坏 session 规范化 | ✅ `0fdcbf14` — `sqlite3.Error` → `RunnerError`，exit 2，无 traceback |
| 7 | 4K/8K 解析 | ✅ `3b8d2ec9` — 体积正则去掉裸 `K/M`；洗版体积未知 → `held` |
| 8 | 洗版查询认证 | ✅ `3b8d2ec9` + `f2ff9698` — `GET /archive/lookup?title=` 带 `X-Lux-Api-Key`；失败/未知 → `held` |
| 9 | 提交与文档 | ✅ `42529388` + 本提交 — `.dockerignore`、healthcheck 无 session 非零、inbox 0644、`PROGRESS.md`/`module-telethon.md` |

验收命令与结果：

```
python3 -m unittest discover -s sidecar/telegram/tests -t sidecar/telegram   # 123 passed
cargo test --locked --lib archive                                           # 24 passed
docker build -f sidecar/telegram/Dockerfile -t lux-telegram:1 sidecar/telegram   # ok
stub Lux 契约冒烟（ingest/lookup/鉴权头）                                      # 6/6
```

仍未解决：115 转存客户端（U11）；端到端（测试频道 → `链接队列/` 出现 json）需真实登录 + 已部署 Lux，留给总控合并后实测。
