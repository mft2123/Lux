# Lux 自建媒体库 — 项目交接与规划

> 这份文档写给「下一个对话」。读完它 + `PROGRESS.md` + `REVIEW-BUS.md` 就能接手。
> 最后更新：2026-10-18 21:05（CST），trunk `f0b05e2e`（文档提交；代码状态 = `7c4dffca`），镜像 `lux:phase-a` = `4813a0cb967b`。

---

## 1. 一句话现状

归档流水线、设置页、Telegram 通知、Telegram 旁挂（代码层）、链接队列、运维修复工具都已完成并合并；
**唯一的功能缺口是「把一条 115 分享链接真正转存进 115 网盘」这一步（脚本里叫 U11）**——其余剩下的是联调、部署和存量迁移。

---

## 2. 项目目标

用 Lux（Rust + React 的自建媒体库）替掉现在的 chill（cinechill），实现：

1. 资源自动入库：Telegram 频道收到资源链接 → 转存进 115 → 落到本地镜像库（strm + NFO + 海报）→ Emby 扫描可见；
2. 归档自动化：转存目录 → 待整理 → TMDB 识别 → `媒体目录/<类型>/<国内|国外>/<年份>/<片名 (年份)>`；
   重复/洗版按体积判定；被替换的旧版进洗版目录，重复的进重复目录；
3. 可运维：设置页可改参数与凭证、TG 通知（入库/洗版/失败/播放…）、失败可查可重试；
4. 最后：存量迁移 + chill 退役（用户要求先观察一段时间）。

---

## 3. 架构与数据流

```
Telegram 频道 ──(Telethon 旁挂)──> Lux ingest API ──> <archive_root>/链接队列/*.json
                                                        │
                                          （每轮消费，写 状态.jsonl）
                                                        ▼
                                          ┌─────────────────────────────┐
                                          │ 115 转存（★ 未实现，U11）    │
                                          └─────────────┬───────────────┘
                                                        ▼
                                           115 网盘「转存目录」（CD2 挂载可见）
                                                        ▼
                          归档流水线：转存目录 → 待整理 → 识别/TMDB → 判定
                                                        │
                        ┌───────────────┬───────────────┼──────────────┐
                        ▼               ▼               ▼              ▼
                    媒体目录        重复目录        洗版目录        失败目录
                    （115 侧）      （115 侧）      （115 侧）      （115 侧）
                        │
                        ▼
                本地镜像库（strm + NFO + poster/fanart）──> Emby 扫描
```

- **CD2（CloudDrive2）** 把 115/百度挂成本地目录；Lux 对「网盘」的全部操作都是普通文件系统操作
  （移动、rename、读体积），不直接调网盘 API。**唯一例外就是上面的 115 转存（U11）**，它必须登录 115。
- 通知模块挂在归档 run 结束、播放开始、以及 admin API 上。

---

## 4. 运行环境（NAS）

| 项 | 值 |
| --- | --- |
| 仓库 | `qiangli@192.168.8.8:/vol5/1000/build/lux`（主 checkout，当前在 `feat/telegram-ingest`，**不是主干**） |
| 主干分支 | `fork/phase-a`（当前 `7c4dffca`） |
| 集成分支 | `integration/round2`（= 主干，工作流用干净克隆 `/tmp/lux-merge`） |
| SSH | `ssh -p 4888 -i "$HOME\.ssh\id_ed25519_feiniu" -o BatchMode=yes qiangli@192.168.8.8 "<cmd>"`；大于几 KB 的文件先本地写再 `scp` |
| 镜像 | `lux:phase-a` = `4813a0cb967b`（只能由总控/交接者构建，模块方不得覆盖） |
| 测试容器 | `lux-testlib`（8099:8097，admin/`Test@12345a`，healthy） |
| 测试配置 | `/vol5/1000/build/archive-test-config`（`lux.db`、`metadata/`，属 root） |
| 测试库 | `/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库`（唯一允许写的网盘位置） |
| 本地镜像库 | `/vol5/1000/docker/lux-library`（Emby 扫这个） |
| CD2 | 容器 `clouddrive2`（`cloudnas/clouddrive2`），web/API 在 **19798**，挂载点在 `/vol5/1000/docker/clouddrive2/CloudNAS` |
| chill | 容器 `cinechill`，配置 `/vol5/1000/docker/chillposter/config`；**闭源**（`.so`/`.pyc`），用 `p115client 0.0.9.4` |
| 设置模板 | `/vol5/1000/build/archive-settings.json`（含 chill 的 TG 凭证，权限 600，**不在 git 里**） |

硬约束：生产媒体库只读；chill/Emby/CD2 不做任何修改；不启用 Lux 的 STRM 远程探测。

---

## 5. 仓库、分支与提交流程

- 主干 `fork/phase-a`；每个模块历史上有独立分支/worktree（`lux-notify-settings`、`lux-telegram-sidecar`、`lux-settings`），
  现在都并入了主干。
- 这一轮的实现分支是 `integration/round2`，等于主干（fast-forward 合并）。
- 提交规范：`type(scope): 描述`，带编号的整改用 `fix(settings): S1 …` 这种形式；
  总线/文档提交用 `bus(...)`、`docs(...)`。
- 合并门禁：`cargo test --locked --lib` 全绿（当前 **1080 passed / 0 failed / 26 ignored**）、
  `pnpm build` 通过（前端改动时）、旁挂 `python -m unittest discover -s tests -t .` 全绿（**207 OK**，ZFS 上 3 skip）、
  全仓 grep 真实手机号/api_id/token 0 命中。
- 总控规则见 `WORKFLOW.md`；评审状态机见 `REVIEW-BUS.md`。

---

## 6. 当前状态（已完成，含证据）

### 6.1 归档流水线（核心，已合并）

- 目录约定：`转存目录 → 待整理 → 媒体目录/<类型>/<国内|国外>/<年份>/<片名 (年份)>`；
  晚会不分地区（`媒体目录/晚会/<年份>/…`），演唱会分国内/国外。
- TMDB：双语搜索、发行地判国内/国外、年份多级回退、晚会/演唱会关键词识别。
- 判定：容差内重复、明显更大洗版（旧版进 `洗版目录/<时间戳>-<原名>`）、更小重复。
- **剧集按季判定**（F1）：剧集包按每个 `Season NN` 独立判定，缺季入库、重复季进 `重复目录/<原包名>/Season NN`、
  更大季洗版；全季重复才退回整包判定；合并后重生成镜像。规范见 `archive-pipeline.md` §5.1。
  实测：`聊斋 (1996)` 丢 Season 02 的现场已修复（40 个文件回库、80 个镜像 sidecar、strm 逐条校验）。
- 本地镜像：strm（CD2 路径）+ NFO + poster/fanart，0755/0644；`POST /archive/metadata` 回填历史条目。
- 相关接口：`GET /archive/status`、`POST /archive/run?dry_run=`、`POST /archive/metadata`、
  `POST /archive/ingest`、`GET /archive/lookup?title=`、`POST|GET /archive/settings`。

### 6.2 设置与 UI（已合并）

- 文件 `/config/archive-settings.json`，优先级 **文件 > 环境变量 > 默认**；`LUX_ARCHIVE_ROOT`/`LUX_TMDB_LANGUAGE` 只由 env 控制。
- PUT 只落盘「本次改动 + 文件已有」的键（env 派生值不会被固化），未知键原样保留；
  越界值 400（不静默钳制）；进程内串行写；单文件 bind mount 时退化为原地写（带 `.bak` 回滚）。
- `telegram.api_key` 字段（旁挂用，打码、留空不改）；`known_categories` 由后端下发（前端不再硬编码）。
- 设置页分组：TMDB / 目录与路径 / 洗版参数 / Telegram 通知 / Telegram 频道监听。

### 6.3 Telegram 通知（已合并）

- 11 个类别（播放/入库/整理/洗版/重复/转存/115妙传/媒体整理/签到/其他任务/失败），
  缺省全开，数组=精确选择，对象=逐项布尔。
- 逐条事件带去重键（失败/重复/转存/整理/洗版），去重键在**发送成功后**才记账；3900 UTF-16 码元上限；
  截断不破坏 HTML 实体；`api_base` 只允许裸 http(s) origin；`NotifyConfig` 的 Debug 已打码。
- 接口：`GET /admin/notify/status`、`POST /admin/notify/test`、`POST /admin/notify/send`。

### 6.4 Telegram 旁挂（代码完成，**未真机验证**）

- 位置 `sidecar/telegram/`，Telethon 1.45.0，依赖已钉死（telethon/pyaes/rsa/pyasn1/python-socks）。
- 投递：默认 `LUX_TG_SINK=http` → `POST /api/v1/admin/archive/ingest`（带 `X-Lux-Api-Key`）；
  带 `source/kind/channel/message_id/size_bytes/captured_at`；`inbox` 仅诊断用。
- 五模式（手动/上新/追更/洗版/全量）；多链接逐链接取标题（缺标题 → held/title_ambiguous）；
  400/403 落死信（`main.py status` 可查、`replay --id` 放行），其余 20 次退避重试；401 可重试。
- `main.py check --deep` 深检登录态，Dockerfile healthcheck 用它。
- 未做：真机登录/真频道投递（没有测试账号凭据）。

### 6.5 链接队列（Lux 侧，已合并）

- 生产方只写、Lux 只读：`<root>/链接队列/<毫秒>-<slug>.json`；`状态.jsonl`（0600）；`已处理/`；`无效/`。
- 幂等键=URL（不设 TTL）；`"force": true` 可强制重投；202 返回 `status: "queued"`。
- 消费器逐条 best effort（单条失败不拖垮整轮）；作业文件 0600；日志只打 `scheme://host`；
  `mode` 只认五种；URL 必须有 host/target。
- `GET /archive/lookup?title=`：只匹配标题层、年份一致、短标题精确、续集不误配；60s 缓存；
  `spawn_blocking` 执行（72k 条目 bench：冷 137ms / 命中 5.9µs）。
- **消费后只记 `needs_115_client` 就停住 —— 这正是 U11 的缺口。**

### 6.6 运维与修复工具（本轮新增）

- `scripts/actor-credits-audit.py`：检查「关系快照说已同步、`person_credits` 却为空」的历史残留；
  `--fix` 删问题条目的 `people.json`；`--prune-stale` 清理条目已不存在的遗留关系目录；有问题 exit 1。
- `POST /api/v1/admin/libraries/{id}/re-enrich`：直接对库的所有条目重跑本地 NFO 富化（带 people），
  把 `--fix` 之后缺失的 credit 补回来；后台执行、幂等。
- 背景：修掉了一个真 bug——credit 分块失败回滚后，关系快照仍「当前」，之后任何扫描都不会补 credit。
  现在失败分块会失效关系快照，并且 re-enrich 可以手动触发修复。

---

## 7. 关键决策与约定（不要推翻，除非有理由）

| 编号 | 决定 |
| --- | --- |
| D1 | 配置优先级 **文件 > env > 默认**，env 只补缺。 |
| D2 | 禁止静默钳制：PUT 越界值一律 400。 |
| D3 | `telegram.api_key` 存在设置文件里（打码、留空不改）；旁挂优先读文件、`LUX_TG_API_KEY` 兜底。 |
| D4 | 设置文件 `0640` + 属组（旁挂 uid 10001 用 `group_add` 读）；写文件后 `chmod/chgrp` 由设置侧负责。 |
| D5 | 通知类别清单由后端下发，前端只渲染。 |
| D6 | `duplicate` 通知默认开启；PUT 不拒未知类别（前向兼容）。 |
| D7 | 分享链接在日志/状态里只允许出现 `scheme://host`。 |
| D8 | 允许一次端到端 TG 实测（前缀「【Lux 验收测试】」），凭据取 `archive-settings.json`。 |
| D9 | 设置读失败每进程只 WARN 一次。 |
| D10 | 合并门禁：三套 cargo 测试 + `pnpm build` + 凭据 grep 0 命中。 |

---

## 8. 待办（按优先级）

### 8.1 ★ U11：115 转存（唯一的功能缺口）

**目标**：把链接队列里的 115 分享链接真正转存进 115 网盘的目标目录，文件经 CD2 挂载出现在
`<archive_root>/转存目录`，之后现有的 `watch_dirs → 待整理 → …` 流水线原样接管。

**已确认的技术路线**（来自 chill 实现调研）：

- chill 用 **`p115client`**（版本 0.0.9.4）+ 网页 cookie 登录。chill 的 115 模块在容器内
  `/app/core/monitor115/`（`client.so`/`models.so`/`monitor.pyc`，**编译过，读不到源码**），
  配置目录里有 `115_file_path_map.json`、`115_life_actions.db`、`drive115_upload_state.json`。
- `p115client` 关键 API：
  - `P115Client(cookies="UID=…; CID=…; SEID=…; KID=…")`，`login_status()` / `user_my()` 验证登录态，
    失效抛 `P115AuthenticationError`；
  - `share_snap({share_code, receive_code})` → `GET https://webapi.115.com/share/snap` 拿 file_id；
  - `share_receive({share_code, receive_code, file_id, cid})` → `POST https://webapi.115.com/share/receive` 转存；
  - `fs_mkdir(name, pid)` 建目标目录；`fs_move` / `fs_copy` 备用；
  - 磁力/ed2k：`clouddownload_task_add_urls(...)`（115 云下载）。
- 建议实现形态：**新 Python 侧车 `sidecar/transfer115/`**（与 Telegram 旁挂同模式，用 p115client），
  Lux 链接队列消费到 `needs_115_client` 时 HTTP 调它；侧车返回 `transferred`/`failed`/`auth_required`，
  Lux 写回 `状态.jsonl`。目标目录按配置的 115 路径解析 cid（如 `测试库/转存目录`），不存在就创建。
  幂等：URL 幂等 + 转存前查目标目录同名。
- 待用户确认的参数（开工前问一次）：cookie 的提供方式（F12 四个值 vs 整串）、目标目录规则（固定 vs 按来源分）、
  是否把 magnet/ed2k 一起做。
- 侧车写好后：离线单测（mock 115 响应）→ 测试库真链接验收 → 更新 `archive-pipeline.md` 队列状态机
  （加 `transferred`/`failed`/`auth_required`）→ 更新总线/PROGRESS。

### 8.2 端到端联调（依赖 8.1）

旁挂真机登录、真频道发链接、看到 `链接队列 → 转存目录 → 待整理 → 媒体目录 → 镜像` 全通。
旁挂 207 条离线用例已过，但**从未用真实凭据跑过**。

### 8.3 生产部署

compose 编排（Lux + 旁挂 + transfer115）、设置文件权限/属组、备份与 WAL checkpoint、监控告警（通知已可用）、
日志脱敏复查。

### 8.4 存量迁移 + chill 退役（用户要求先观察几天）

迁移脚本/流程、Emby 库切换、回滚方案、观察期。

### 8.5 收尾小项

- I7：链接队列幂等没有 TTL/轮转（现在只有 `force` 重投），`状态.jsonl` 只增不减。
- `lux-bug-reports.md` 里三份上游 issue（首页/latest 超时、NFO 毒批次、has_available_source 不传播）
  还没提交给上游作者。
- 大库性能复核：lookup 冷启动 180k 外推约 340ms；首页/latest 的修复需要 18 万条目环境复测。

---

## 9. 踩过的坑（新对话必读）

1. **普通 scan / reconcile 不会重新富化未变化的条目。** 实测：touch 或改 NFO 内容后重扫都没用；
   要补历史数据只能 `POST /libraries/{id}/re-enrich`（本轮新增），或删库重建。
2. **SQLite `SQLITE_BUSY_SNAPSHOT` 不会被 busy_timeout 重试。** 并发写 people index 会瞬时失败；
   已在 `persist_nfo_item_actors_deferred` 里加 3 次重试。
3. **credit 分块失败会留下「关系快照当前、credit 为空」的坏状态。** 已修（失败时失效快照），
   历史数据用审计脚本 + re-enrich 处理。
4. **单文件 bind mount 上 `rename` 会 EBUSY**，设置写入必须退化原地写（有 `.bak`）。
5. **ZFS 上 `chmod` 对属主不生效**，旁挂的权限类测试在本机会 skip；真机权限要在容器里验。
6. **`cargo test --lib archive` 的过滤词会命中 `notify::event` 和 `observability::logs` 的用例**，
   数量不是「archive 模块」的真实数；看整库 1080 那个数字。
7. **CD2 只作为路径来源**（`cd2_prefix`/`cd2_mount` 生成 strm 内容，`watch_dirs` 是挂载点），
   代码里没有任何 CD2 API 调用；不要误以为转存是 CD2 做的。
8. **chill 闭源**：只有 `.so`/`.pyc`，115 逻辑靠 `p115client` 的行为反推。
9. **容器日志可能不滚动到 stdout**（`/config/logs` 有文件）；排查时看 `docker logs` 也看配置文件目录。
10. **测试容器的 DB/元数据属 root**：读写要 `docker run --user 0 -v …:/config`，不要在宿主机直接读。

---

## 10. 常用命令

```bash
# 测试（整库）
docker run --rm -e CARGO_HOME=/cargo-home -e RUSTUP_TOOLCHAIN=1.94.1 -e RUSTUP_DIST_SERVER=https://rsproxy.cn \
  -v /vol5/1000/build/cargo-home:/cargo-home -v <src>:/src \
  -v /vol5/1000/build/target:/src/target -w /src rust:1.94-test \
  bash -c "cargo test --locked --lib"

# 旁挂测试
cd sidecar/telegram && python3 -m unittest discover -s tests -t .

# 构建镜像（只有总控做）
cd <src> && DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .

# 重建测试容器（新镜像后）
docker rm -f lux-testlib && docker run -d --name lux-testlib -p 8099:8097 \
  -e LUX_ARCHIVE_ROOT=/testlib -e LUX_ARCHIVE_MIRROR_ROOT=/library -e LUX_ARCHIVE_CD2_PREFIX=测试库 \
  -e LUX_ARCHIVE_CD2_MOUNT=/media/cloud -e LUX_ARCHIVE_WATCH_DIRS=/testlib/外部上传,/baidu \
  -e LUX_ARCHIVE_POLL_SECONDS=10 -e LUX_ARCHIVE_SETTLE_SECONDS=3 -e LUX_TMDB_LANGUAGE=zh-CN \
  -e LUX_HTTP_ADDR=0.0.0.0:8097 -e LUX_CONFIG_DIR=/config -e RUST_LOG=luxd=info \
  -v /vol5/1000/docker/lux-library:/library \
  -v fc7b30cdcd17ddb92b4473c2bcf840dfa306a20aeebac7f0874867d98ca0d4ea:/media \
  -v /tmp/lux-baidu-sim:/baidu \
  -v /vol5/1000/build/archive-test-config:/config \
  -v /vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库:/testlib \
  lux:phase-a

# 演员 credit 审计 / 修复
docker run --rm --user 0 -v <config>:/config:ro -v <repo>/scripts:/scripts --entrypoint python python:3.12-slim \
  /scripts/actor-credits-audit.py --config-dir /config            # 只读检查
... --fix --prune-stale                                           # 失效问题关系 + 清垃圾
curl -X POST -b cookies -H "X-CSRF-Token: …" http://<host>:8097/api/v1/admin/libraries/<id>/re-enrich

# 登录测试容器（admin / Test@12345a）
curl -s -c /tmp/c.jar -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H 'content-type: application/json' -d '{"username":"admin","password":"Test@12345a"}'
csrf=$(awk '$6=="lux_csrf" {print $7}' /tmp/c.jar)
```

---

## 11. 未验证 / 风险

- 旁挂从未真机登录、从未真正投递过（只有 stub 契约 + 207 条离线用例）。
- 链接队列从未和真实 115 转存串起来（U11 未实现）。
- 生产环境从未跑过（只有测试容器）；大库（18 万条目）性能只有外推，没有实测。
- 上游三份 issue 未提交。
- `re-enrich` 会把整库 NFO 重读一遍，大库上要挑空闲时段；以后可以改成只处理指定 item ids。

---

## 12. 新对话怎么开场

把下面这段直接粘给新对话即可：

```
接手 Lux 项目。先读 /vol5/1000/build/lux（NAS，ssh -p 4888 -i ~/.ssh/id_ed25519_feiniu qiangli@192.168.8.8）
里的 docs/handoff/HANDOFF.md，再读 PROGRESS.md 与 REVIEW-BUS.md。
主干分支 fork/phase-a，镜像 lux:phase-a，测试容器 lux-testlib。
下一步优先做 HANDOFF 第 8.1 节的 U11（115 转存侧车，p115client + cookie），
开工前先问我要 cookie 提供方式和目标目录规则。
```

必读文件顺序：`HANDOFF.md` → `PROGRESS.md` → `REVIEW-BUS.md` → `archive-pipeline.md`
→（做到哪块再看）`module-*.md` / `fix-*-round2.md`。

---

## 13. 用户偏好（接手者注意）

- 中文交流；结论先行，过程简短。
- 不接受「大概能用」：每个改动要有测试或实测证据，没验证的要明说。
- 不接受伪造/放宽测试来「修绿」；宁可指出是既有问题。
- 生产媒体库只读；chill/Emby/CD2 不动；不启用 STRM 远程探测。
- 镜像只由总控/交接者构建；模块方不覆盖 `lux:phase-a`。
