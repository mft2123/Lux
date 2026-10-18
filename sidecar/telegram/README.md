# Telethon 旁挂（module-telethon）

Python 旁挂服务：用**自己的** Telegram 用户会话监听频道，把资源链接按五种模式过滤后交给 Lux。
不碰 Lux 的 Rust 代码，也不复用 chill 的 `telegram_user.session`（会把 chill 踢下线）。

```
频道消息 ──► 提取链接（含隐藏链接）──► 五模式判定 ──► POST /api/v1/admin/archive/ingest
                                        │                （Lux 写 <archive_root>/链接队列/）
                                        └──► SQLite（去重、游标、挂起、重试）
```

## 目录

```
sidecar/telegram/
  main.py            # 入口：run / status / check / held / replay
  tgwatch/
    config.py        # 设置文件 telegram 段 + LUX_TG_* 覆盖；读 api_key / wash_tolerance_ratio
    links.py         # 链接提取（entity 隐藏链接 + 明文）、体积解析、标题猜测
    filters.py       # 五种模式判定（纯函数）
    store.py         # SQLite：消息去重、频道游标、条目状态机、重试队列
    sink.py          # 投递：http（默认，Lux ingest）/ inbox（诊断）；含 lookup 查询
    runner.py        # telethon 接线：登录、权限加固、监听、回填/补拉、投递、重试
    logsafe.py       # 日志脱敏（api_hash / phone / 2FA / api_key）
  tests/             # 离线单测，不装 telethon 也能跑
  tools/api_smoke.py # 升级 telethon 后核对 API
  session/           # telethon session（0700 目录 / 0600 文件，不进仓库）
  state/             # SQLite 状态库（800/600）
  Dockerfile         # 非 root（10001），healthcheck 会真的失败
  compose.fragment.yml
```

## 五种模式

| 模式 | 别名 | 行为 |
| --- | --- | --- |
| 手动 | `manual` | 只记录不投递；管理员 `main.py replay --all` 时才处理 |
| 上新 | `new` | 新消息含链接即投递 |
| 追更 | `follow` | 仅当**链接标题**命中 `follow_list` 时投递 |
| 洗版 | `wash` | 仅当链接体积大于库中同名版本（超出 `wash_tolerance_ratio`）时投递 |
| 全量 | `all` | 所有链接投递，启动时回填**最新** N 条 |

判定只有三种结果：`deliver` / `hold`（留库等放行）/ `skip`（记原因）。
洗版只要有一项不确定就 `hold`：体积读不出、Lux 查询失败，都不投。

逐频道覆盖：`listen_channels` 的字符串沿用全局 `listen_mode`，对象可写
`{"channel": "@b", "mode": "追更", "follow_list": ["某剧", "/S0[12]E/"]}`。

## 配置

读 Lux 的设置文件（**只读**），默认 `/config/archive-settings.json`：

```json
{
  "wash_tolerance_ratio": 0.02,
  "telegram": {
    "api_id": 123456,
    "api_hash": "…",
    "phone": "+10000000000",
    "api_key": "<Lux 管理 API key>",
    "listen_mode": "上新",
    "listen_channels": ["@somechannel"],
    "follow_list": []
  }
}
```

`api_key` 用于 `X-Lux-Api-Key`（ingest + lookup 都是管理员接口）。

环境变量（优先于设置文件）：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `LUX_ARCHIVE_SETTINGS` | `/config/archive-settings.json` | 设置文件路径 |
| `LUX_TG_API_ID` / `LUX_TG_API_HASH` / `LUX_TG_PHONE` | 取设置文件 | 覆盖凭证 |
| `TG_2FA_PASSWORD` | 空 | 二步验证密码，一次性 |
| `LUX_TG_API_KEY` | 取设置文件 `telegram.api_key` | Lux 管理 API key |
| `LUX_TG_MODE` / `LUX_TG_CHANNELS` / `LUX_TG_FOLLOW` | 取设置文件 | 模式与频道 |
| `LUX_TG_ALLOWED_KINDS` | `115,magnet,ed2k,aliyun,quark,baidu,other` | 只投递这些类型 |
| `LUX_TG_WASH_TOLERANCE` | 取设置文件 `wash_tolerance_ratio`（0.02） | 洗版容差 |
| `LUX_TG_LUX_BASE` | `http://lux:8097` | Lux 地址，派生 ingest/lookup URL |
| `LUX_TG_INGEST_URL` / `LUX_ARCHIVE_LOOKUP_URL` | 由 base 派生 | 单独覆盖 |
| `LUX_TG_SINK` | `http` | `http` 或 `inbox`（后者仅诊断用） |
| `LUX_TG_INBOX_DIR` | `/watch-inbox` | `sink=inbox` 时的落点 |
| `LUX_TG_SESSION_DIR` / `LUX_TG_STATE_DIR` | `session/` / `state/` | 卷 |
| `LUX_TG_BACKFILL_LIMIT` | `200` | 全量模式回填最新条数，0 = 不回填 |
| `LUX_TG_GAP_LIMIT` | `200` | 启动时按游标补拉的条数上限 |
| `LUX_TG_RETRY_SECONDS` | `300` | failed/pending 重试间隔 |
| `LUX_TG_DRY_RUN` | `0` | 只判定记录，不写下游 |
| `LUX_TG_LOG_LEVEL` | `INFO` | 日志级别 |

## 首次登录

首次 `run` 自动走登录流程：

1. 容器日志打印验证码提示；
2. 把 Telegram 收到的验证码写进 `session/code.txt`（宿主机
   `/vol5/1000/build/lux-telegram/session/code.txt`），进程读走即删；
   `docker attach` / `docker run -it` 时也可以直接在终端敲；
3. 有二步验证时用 `TG_2FA_PASSWORD` 一次性传入；
4. 成功后 session 落盘并 `chmod 0600`，目录 `0700`；之后重启不掉线。

```bash
mkdir -p /vol5/1000/build/lux-telegram/{session,state}
chown -R 10001:10001 /vol5/1000/build/lux-telegram
chmod 700 /vol5/1000/build/lux-telegram/session
docker build -f sidecar/telegram/Dockerfile -t lux-telegram:1 sidecar/telegram
docker compose up -d telegram && docker logs -f lux-telegram
```

## 运维命令

```bash
docker exec -u 10001 lux-telegram python main.py status      # 配置（打码）+ session 权限 + inbox
docker exec -u 10001 lux-telegram python main.py check       # 校验（无 session 时返回 1，healthcheck 用）
docker exec -u 10001 lux-telegram python main.py held --limit 50
docker exec -u 10001 lux-telegram python main.py replay --all
```

## 下游契约

### ingest（默认）

`POST /api/v1/admin/archive/ingest`，头 `X-Lux-Api-Key`，body：

```json
{"url": "https://115.com/s/xxxx", "title": "某剧 S01", "mode": "上新"}
```

Lux 校验后返回 202 `{ok, jobId, file, status, duplicate, note}`，并把任务写到
`<archive_root>/链接队列/<ts>-<slug>.json`；流水线下一轮把它记进 `链接队列/状态.jsonl`
（当前 115 转存客户端未实现，状态为 `needs_115_client`）。幂等在 Lux 侧按 URL 做。

### wash 查询

`GET /api/v1/admin/archive/lookup?title=…`（同样带 `X-Lux-Api-Key`）→
`{"title","matched","size_bytes"}`。`size_bytes=null` 表示库中无同名版本（按新增投递）；
请求失败则条目 `held`。

### 死信与重试

投递失败的条目按错误性质分流（`tgwatch/runner.py`）：

| 上游返回 | 处理 |
| --- | --- |
| 400（链接被 Lux 判非法）、403（管理员接口无权限） | **永久失败**：只尝试一次，立刻置 `dead`，不再自动重试 |
| 401（API key 无效） | 可重试：等运维修 `telegram.api_key` 后下一轮继续 |
| 5xx / 网络错误 / 磁盘错误 | 可重试：最多 `20` 次，重试间隔从 `LUX_TG_RETRY_SECONDS` 起指数退避（封顶 1 小时） |
| 试满 20 次 | 置 `dead`，原因写成 `max_attempts(20): …` |

死信不参与自动重试，`main.py status` 能看到计数与原因：

```bash
docker exec -u 10001 lux-telegram python main.py status
# 状态库: {"messages":…,"by_status":{…,"dead":2},"dead":2,"dead_reasons":{"Lux 永久拒绝（400，不再重试）…":2}}
# 死信: 2 条（原因 {…}；`main.py replay --id <id>` 可人工再试）

docker exec -u 10001 lux-telegram python main.py replay --id 42   # 看清原因后人工放行一条死信
```

### inbox（可选，诊断用）

`LUX_TG_SINK=inbox` 时写 `/watch-inbox/<ts>-<slug>.txt`，首行链接、其余 `#` 注释、`0644`。
注意：Lux 的 `ingest_watch_dirs` 只接受视频，`.txt` 进去会被判「不是视频文件」，**不要**拿它当正式通道。

## 测试

```bash
cd sidecar/telegram
python -m unittest discover -s tests -t .    # 123 个用例，全部离线
python tools/api_smoke.py                    # 升级 telethon 后核对 API（21 项）
```

覆盖：UTF-16 锚文本切片、隐藏链接、类型判定、体积解析（`4K/8K/8M` 不算体积）、五模式、
洗版容差与「未知就挂起」、配置优先级/打码/硬错误、SQLite 去重与频道游标、inbox 与 HTTP 契约、
`X-Lux-Api-Key`、lookup 失败降级、日志脱敏、登录验证码两条路径、回填取最新 / 按游标补拉、
session 权限加固、损坏 session 的干净报错。

## 已知边界

- **不实现 115 转存**：Lux 侧 ingest 只入队并标 `needs_115_client`，真正的转存在 U11。
- **单实例**：SQLite 状态库按单进程设计，别两个容器挂同一份 `state/`。
- **账号风险**：自铸会话（不复用 chill），封禁风险按计划文档记为接受。
