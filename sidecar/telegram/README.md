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
  state/             # SQLite 状态库（0700 目录 / 0600 文件）
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

### 一条消息多个链接（SC7）

标题与洗版查询都是**逐链接**的：每个链接只用它**自己的锚文本**当标题，洗版模式也按
各自的标题分别查库（同一标题命中缓存，不会重复请求 Lux）。

同一条消息里有多条链接、而某个链接没有独立锚文本时**不猜标题**：该链接记 `held`
（`reason=title_ambiguous`，日志里点名具体 URL），同一条消息里其它有标题的链接照常
投递/判定。整段正文首行只在**单链接**消息里当兜底标题。

逐频道覆盖：`listen_channels` 的字符串沿用全局 `listen_mode`，对象可写
`{"channel": "@b", "mode": "追更", "follow_list": ["某剧", "/S0[12]E/"]}`。

## 收哪些链接

`kind` 由 `links.classify()` 按域名判定：`115` / `aliyun` / `quark` / `baidu` / `magnet` /
`ed2k` / `telegram` / `other`。

- 默认只收**已知网盘与磁力**：`115,magnet,ed2k,aliyun,quark,baidu`；
- `telegram`（`t.me` / `telegram.me` / `telegram.dog` 的频道互引）默认丢弃，这是既有行为；
- `other`（任意 http 网页、新闻链接）默认**丢弃**；要收必须显式 `LUX_TG_ALLOW_OTHER=1`
  （之后 payload 里的 `kind=other` 让 Lux 侧可过滤统计）；
- 自定义白名单用 `LUX_TG_ALLOWED_KINDS=115,quark`；在白名单里直接写 `other` 等价于打开
  `LUX_TG_ALLOW_OTHER`，不需要再给开关。

被丢弃的消息照常入库，`status=skipped`、`reason=no_allowed_link`（提取到链接但类型不收）
或 `no_link`（正文里没有链接），`main.py status` 的 `by_status` 里能看到。

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

`api_key` 用于 `X-Lux-Api-Key`（ingest + lookup 都是管理员接口）。**它是设置文件优先的**：
只要 `telegram.api_key` 有值就忽略 `LUX_TG_API_KEY`，env 只在文件里还没填时兜底（D1）。

环境变量（设置文件缺失/为空时覆盖；`LUX_TG_API_KEY` 见上一条的特殊优先级）：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `LUX_ARCHIVE_SETTINGS` | `/config/archive-settings.json` | 设置文件路径 |
| `LUX_TG_API_ID` / `LUX_TG_API_HASH` / `LUX_TG_PHONE` | 取设置文件 | 覆盖凭证 |
| `TG_2FA_PASSWORD` | 空 | 二步验证密码，一次性 |
| `LUX_TG_API_KEY` | 取设置文件 `telegram.api_key` | **文件优先**，这里只兜底 |
| `LUX_TG_MODE` / `LUX_TG_CHANNELS` / `LUX_TG_FOLLOW` | 取设置文件 | 模式与频道 |
| `LUX_TG_ALLOWED_KINDS` | `115,magnet,ed2k,aliyun,quark,baidu` | 只投递这些类型（`telegram`/`other` 不在默认里） |
| `LUX_TG_ALLOW_OTHER` | `0` | 设为 `1` 才收 `kind=other`（任意 http 网页） |
| `LUX_TG_WASH_TOLERANCE` | 取设置文件 `wash_tolerance_ratio`（0.02） | 洗版容差 |
| `LUX_TG_LUX_BASE` | `http://lux:8097` | Lux 地址，派生 ingest/lookup URL |
| `LUX_TG_INGEST_URL` / `LUX_ARCHIVE_LOOKUP_URL` | 由 base 派生 | 单独覆盖 |
| `LUX_TG_SINK` | `http` | `http` 或 `inbox`（后者仅诊断用） |
| `LUX_TG_INBOX_DIR` | `/watch-inbox` | `sink=inbox` 时的落点 |
| `LUX_TG_SESSION_DIR` / `LUX_TG_STATE_DIR` | `session/` / `state/` | 卷 |
| `LUX_TG_BACKFILL_LIMIT` | `200` | 全量模式回填最新条数，0 = 不回填 |
| `LUX_TG_GAP_LIMIT` | `200` | 启动时按游标补拉的条数上限 |
| `LUX_TG_RETRY_SECONDS` | `300` | failed/pending 重试的**基础**间隔；连续失败时指数退避（封顶 1 小时） |
| `LUX_TG_DRY_RUN` | `0` | 只判定记录，不写下游 |
| `LUX_TG_LOG_LEVEL` | `INFO` | 日志级别 |

## 部署检查清单（设置文件 0640 + 属组）

Lux 保存设置时会把 `/vol5/1000/build/archive-settings.json` 写成 `0640`，属组为
`$LUX_ARCHIVE_SETTINGS_GROUP`（未设置则该组不存在时保持 0600）。旁挂容器以 uid `10001`
运行，只能通过**附加组**读到这个只读挂载的文件；`compose.fragment.yml` 里的
`group_add: ["${LUX_ARCHIVE_SETTINGS_GROUP:-10001}"]` 就是干这个的。上线前逐条核对：

1. **属主/权限**：`ls -l /vol5/1000/build/archive-settings.json`
   应为 `-rw-r----- 1 <lux>:<gid>`；`stat -c '%a %g %U' …` 应为 `640 <gid> <lux>`。
2. **gid 对齐**：`<gid>` 必须等于 compose 里的 `${LUX_ARCHIVE_SETTINGS_GROUP:-10001}`
   （默认 `10001`；Lux 侧改了设置组就必须同步改这里，否则容器里是另一个组）。
3. **compose 生效**：`docker compose config | grep -A2 group_add` 能看到
   `group_add: 10001`（或你的 gid）。
4. **容器内组**：`docker exec lux-telegram id` 的 `groups=` 里包含该 gid。
5. **能读到文件**：`docker exec -u 10001 lux-telegram python main.py check`
   不再报「设置文件不可读」。
6. **失败怎么办**：错误信息是「设置文件不可读：检查属组/权限（见 README 的部署检查清单）：<路径>（<原因>）」。
   先回第 1、2 步；`main.py status` 里的 `settings_path` 能确认挂载路径对不对。
   若 Lux 还没把文件改成 0640（仍是 0600），旁挂必然读不到——要么等 Lux 侧保存一次设置，
   要么临时用 `LUX_TG_API_KEY` / `LUX_TG_API_ID` 等环境变量跑起来（见下一节）。

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
docker exec -u 10001 lux-telegram python main.py status       # 配置（打码）+ session 权限 + inbox
docker exec -u 10001 lux-telegram python main.py check        # 只验配置就绪（不联网）
docker exec -u 10001 lux-telegram python main.py check --deep # 再连一次 Telegram 验登录态
docker exec -u 10001 lux-telegram python main.py held --limit 50
docker exec -u 10001 lux-telegram python main.py replay --all
```

### `check` 与 `check --deep` 的区别（SC9）

| 命令 | 检查什么 | 用在哪 |
| --- | --- | --- |
| `main.py check` | 只看**配置就绪**：设置文件能读、`listen_channels` 非空、session 文件存在、`sink=http` 时 `api_key` 非空。**不联网**。 | 部署检查清单第 5 步、改配置后快速自检 |
| `main.py check --deep` | 在上面基础上真的连一次 Telegram，用 `is_user_authorized()` 确认 session 还有登录态（10s 超时）。无 session、未授权、连不上都返回非零。 | `Dockerfile` 的 `HEALTHCHECK` |

session 过期但文件还在时 `check` 仍然通过——这是它「只验配置就绪」的边界。容器健康状态
由 `check --deep` 决定：掉线即 unhealthy；容器内没有网络时也会失败，这是期望行为（不是误报）。

## 下游契约

### ingest（默认）

`POST /api/v1/admin/archive/ingest`，头 `X-Lux-Api-Key`，body（与 Lux 的 `LinkJob` 逐字段对齐）：

```json
{
  "url": "https://115.com/s/xxxx",
  "title": "某剧 S01",
  "mode": "上新",
  "source": "telegram",
  "kind": "115",
  "channel": "@somechannel",
  "message_id": 4321,
  "size_bytes": 26843545600,
  "captured_at": "2026-10-18T12:00:00+00:00"
}
```

`source` 固定为 `telegram`（Lux 用它区分投递来源）；`kind` 是链接类型；`channel` / `message_id` /
`captured_at` 来自原始频道消息；`size_bytes` 从正文解析，读不出时为 `null`。这样
`链接队列/状态.jsonl` 的每条记录都能回溯到频道与消息。

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
python -m unittest discover -s tests -t .    # 184 个用例，全部离线
python tools/api_smoke.py                    # 升级 telethon 后核对 API（21 项，需要 telethon）
```

权限类用例（session / 设置文件的 `chmod`）在 root 或「不校验 chmod」的文件系统上会自动
`skip`，不会假装通过；正常情况下 184 条全跑。

覆盖：UTF-16 锚文本切片、隐藏链接、类型判定、体积解析（`4K/8K/8M` 不算体积）、五模式、
洗版容差与「未知就挂起」、配置优先级/打码/硬错误、`other` 默认不放行、HTML 实体反转义、
SQLite 去重与频道游标、inbox 与 HTTP 契约（完整 payload）、`X-Lux-Api-Key`、lookup 失败降级、
死信与重试上限/退避、日志脱敏、登录验证码两条路径与首登 phone 校验、回填取最新 / 按游标补拉、
session 权限加固、sqlite 错误分类（损坏 vs 权限/磁盘）、关停前收尾后台任务、部署片段静态检查。

## 已知边界

- **不实现 115 转存**：Lux 侧 ingest 只入队并标 `needs_115_client`，真正的转存在 U11。
- **单实例**：SQLite 状态库按单进程设计，别两个容器挂同一份 `state/`。
- **账号风险**：自铸会话（不复用 chill），封禁风险按计划文档记为接受。
