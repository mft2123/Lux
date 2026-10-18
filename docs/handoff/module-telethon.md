# 模块：Telegram 频道监听旁挂（module-telethon）

目标：Python 旁挂服务（Telethon Userbot），监听指定频道，把资源链接交给 Lux 转存；模式：手动/上新/追更/洗版/全量。

## 目录

```
sidecar/telegram/
  main.py          # 入口：读配置 → 登录 → 监听 → 投递
  requirements.txt # telethon
  Dockerfile
  session/         # telethon session 文件（卷）
```

## 配置（读同一份 archive-settings.json 的 telegram 段）

- api_id / api_hash / phone / listen_mode / listen_channels（频道 @username 或 id 列表）/ follow_list（追更目标）
- 登录：首次运行在容器日志打印验证码提示，用户在 `session/` 目录下用交互终端录入；2FA 密码通过环境变量 `TG_2FA_PASSWORD` 一次性提供
- 投递：POST 到 Lux `POST /api/v1/admin/archive/ingest`（本模块需在 Lux 侧加一个接口：接收 `{url, mode}` 并写入待处理队列）
  或更简单：写入共享目录 `/watch-inbox/<ts>-<slug>.txt`（Lux 的 watch_dirs 已在监控，天然复用）

## 模式语义

| 模式 | 行为 |
| --- | --- |
| 手动 | 只记录不投递；管理员触发时才处理 |
| 上新 | 新消息含链接即投递 |
| 追更 | 仅当链接标题命中 follow_list（剧集名）时投递 |
| 洗版 | 仅当链接体积大于库中同名的现有版本时投递（调用 Lux 的 dry-run 判定） |
| 全量 | 所有链接投递 |

## 验收

- 手动登录一次，session 持久化；重启不掉线
- 测试频道发一条链接，`/watch-inbox/` 出现对应文件，Lux 随后自动整理
- 五种模式各有单元测试（消息过滤逻辑），不依赖真实 TG
- 日志不含 api_hash/phone 明文

## 交付状态（2026-10-18，review 整改后）

分支 `feat/telegram-sidecar`（worktree `/vol5/1000/build/lux-telegram-sidecar`，基线 `fork/phase-a`）。
状态以 `docs/handoff/REVIEW-BUS.md` 的「旁挂」一节为准。

| 提交 | 内容 |
| --- | --- |
| `3b8d2ec9` | F7 体积单位（4K/8K/8M 不算体积）+ F8 洗版判定表（三要素缺一即 held） |
| `f2ff9698` | F1 默认投递到 Lux ingest（`{url,title,mode}` + `X-Lux-Api-Key`）+ F8 lookup 鉴权 + F3 假凭证 |
| `0fdcbf14` | F2 权限加固、F4 回填取最新、F5 catch_up/游标补拉、F6 损坏 session 干净报错 |
| `42529388` | F2/F9 非 root 镜像、`.dockerignore`、healthcheck 无 session 时非零 |

Lux 侧的 ingest/lookup 由 `feat/telegram-ingest` 实现并已合并到 `fork/phase-a`
（`POST /api/v1/admin/archive/ingest`、`GET /api/v1/admin/archive/lookup`、
`<archive_root>/链接队列/` 与状态文件）。本模块只消费这两个接口，不改 Rust 文件。

逐条对照本 brief：

| 文档要求 | 实现 |
| --- | --- |
| `main.py`：读配置 → 登录 → 监听 → 投递 | `run`（首次运行即登录）/ `status` / `check` / `held` / `replay` |
| `requirements.txt` = telethon | `telethon==1.45.0`，其余全用标准库 |
| `session/` 是卷 | 默认 `sidecar/telegram/session/`，容器 `/app/session`；**目录 0700、文件 0600** |
| 读 `archive-settings.json` 的 telegram 段 | `tgwatch/config.py` 只读；另读 `telegram.api_key` 与顶层 `wash_tolerance_ratio` |
| 登录：日志提示 + 在 `session/` 下录入 | `runner.code_provider()`：提示进日志，读 `session/code.txt`（读走即删）或 tty 直接敲 |
| 2FA：`TG_2FA_PASSWORD` | 同名环境变量，只进 `client.start(password=)`，不进日志 |
| 投递 A：`POST …/archive/ingest` | **默认通道** `{url,title,mode}` + `X-Lux-Api-Key`（Lux `LinkJob` 就这三个字段起） |
| 投递 B：`/watch-inbox/<ts>-<slug>.txt` | `LUX_TG_SINK=inbox` 时可用（诊断用；`ingest_watch_dirs` 只收视频，不作正式通道） |
| 手动 | `held`，`main.py replay --all` 才投递 |
| 上新 | 新消息含链接即投递；启动时 `catch_up()` + 游标补拉宕机窗口 |
| 追更 | 只匹配**链接标题**（锚文本/正文首行），支持 `/正则/` |
| 洗版 | `GET /archive/lookup?title=` 查库中体积，按 `wash_tolerance_ratio` 比较；体积未知/查询失败一律 `held` |
| 全量 | 所有链接投递 + 启动时回填**最新** N 条（按时间顺序处理） |
| 验收：session 持久化 | 0600 落卷；重启走已授权分支不再要码 |
| 验收：五模式单测、不依赖真实 TG | `tests/test_filters.py` + `tests/test_runner.py`，全库 123 个离线用例 |
| 验收：日志不含 api_hash/phone | `tgwatch/logsafe.py`（api_key 一并脱敏）+ `tests/test_logsafe.py` |

## 验证记录（NAS）

```
python3 -m unittest discover -s sidecar/telegram/tests -t sidecar/telegram   # 123 passed
cargo test --locked --lib archive                                           # 24 passed
docker build -f sidecar/telegram/Dockerfile -t lux-telegram:1 sidecar/telegram
docker run --rm --entrypoint id lux-telegram:1                              # uid=10001(lux)
# 容器内 harden_permissions 实测：session 0o600、目录 0o700
# check 无 session → exit 1；缺 api_key → exit 2 并给出可操作提示
# stub Lux 契约冒烟 6/6：路径、body、X-Lux-Api-Key、lookup 查询串与解析
```

## 遗留

- **115 转存客户端（计划 U11）未实现**：Lux ingest 只入队并把状态记成 `needs_115_client`，
  链接还不会真的转存。端到端验收（测试频道发链接 → `链接队列/` 出现 json）需要真实登录 +
  已部署含 ingest 的 Lux，留给总控合并后实测。
- `listen_channels` 需要运维填（设置页 UI 归 module-settings）；当前线上设置是手动模式且频道为空。

## 第二轮整改（2026-10-18）

- **管理 key 的来源**：优先读设置文件 `telegram.api_key`，缺失时才用 `LUX_TG_API_KEY` 兜底；
  Lux 设置页新增了该字段（打码、留空不改），所以从 UI 保存设置不会再把它抹掉。
- **设置文件可读性**：Lux 以 0600 写、旁挂以 uid 10001 只读挂载。部署时要把文件改成 `0640` 并把属组
  设成旁挂的 gid（compose 里用 `LUX_ARCHIVE_SETTINGS_GROUP` + `group_add`），README 的"部署检查清单"有步骤；
  旁挂读到权限错误会直接提示这一条，而不是笼统报配置错误。
- **投递语义**：400/403 视为永久失败落死信（`main.py status` 可查、`replay --id` 可人工放行），
  其余错误最多重试 20 次并指数退避；401 保留可重试（运维换 key 后能自愈）。
- **默认只收已知网盘/磁力**，`LUX_TG_ALLOW_OTHER=1` 才收其它 http 链接。
- 状态回执仍是聚合的 `pending_link_jobs`；单作业状态查询（`GET …/ingest/{jobId}`）留待 115 转存客户端接入时一并做。
