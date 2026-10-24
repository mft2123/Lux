---
artifact_contract: "ce-handoff/v1"
created_at: "2026-10-24T14:55:00Z"
title: "Lux Telegram 频道监听 v2：判定链、离线下载、补集与回执三态"
summary: "2026-10-24 一轮：TG 频道监听从「单模式只投递」升级为每频道模式 + Rust 判定链 + 补集 + 回执三态；magnet/ed2k 走 115 离线下载；8 个提交只在本地，分支未推"
keywords: ["lux", "telegram", "115", "offline-download", "receipts", "补集", "catalog", "nas", "docker"]
cwd: "C:\\Users\\liqiang\\AppData\\Roaming\\dsh-desktop\\harness"
resume_focus: "把 fix/u11-e2e 推上远端（NAS 无 GitHub 凭据，等用户给 PAT），然后补 compose 接线与 docs/archive-pipeline.md 的 TG 段落"
repository: "lux"
repo_root_sha: "5614f65d2378c449644eff45daeef95ebaa1457b"
branch: "fix/u11-e2e"
head: "fb16aed7"
worktree_path: "/tmp/lux-u11"
---

# Lux 交接（2026-10-24）

> 下一会话先读这份。两份契约仍然有效，改算法前先改它们：
> - `docs/archive-pipeline.md` —— 归档行为规范（**但 Telegram 段落还是旧描述，见 §7**）
> - `docs/handoff/AUTO-LIBRARY.md` —— 自动入库契约
>
> 上一轮：`docs/handoff/NEXT-SESSION-2026-10-23.md`（目录下钻 / 板块调整 / 视觉验收）。

## 1. 怎么连、怎么跑

- 仓库：`qiangli@192.168.8.8:/vol5/1000/build/lux`，主 checkout 在 `feat/telegram-ingest`（**该分支未推 origin**）
- 工作区：worktree `/tmp/lux-u11`，分支 `fix/u11-e2e`，本轮结束 worktree 干净，8 个提交
- 从 Windows：`ssh -p 4888 -i ~/.ssh/id_ed25519_feiniu qiangli@192.168.8.8`
- 长脚本别在 `ssh "…"` 里拼 heredoc：本地拼好 → base64 → `echo <b64> | base64 -d | bash`
- 测试容器 `lux-testlib`：宿主 8099 → 容器 8097，`admin` / `Test@12345a`（测试环境专用口令）
- 重建镜像：`cd /tmp/lux-u11 && DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .`（约 4 分钟）
- 重建容器：`bash /tmp/recreate3.sh`（已加 `-e LUX_TG_SIDECAR_BASE=http://172.17.0.1:8091`）
- **`docker build` 成功 ≠ 生效**：核对 `docker inspect lux-testlib --format "{{.Image}}"`
- 测试配置：`/vol5/1000/build/archive-test-config/archive-settings.json`（root 所有，容器内可读）
- 测试库：`/vol5/1000/build/archive-test-config/lux.db`

本轮运行的镜像与容器：

| 容器 | 镜像 | 端口 | 说明 |
| --- | --- | --- | --- |
| `lux-testlib` | `lux:phase-a` = `5bc218de…` | 8099→8097 | 被测 Lux |
| `lux-transfer115` | `lux-transfer115:latest` = `1519d127…` | 8098 | 115 转存 / 离线下载 |
| `lux-telegram-test` | `9fc278eb…`（旧） | — | **还在跑旧代码**，本轮没换 |
| （无） | `lux-telegram:test` = `d992b40e…` | 8091 | 本轮新构建，用 `docker run` 临时验证过 |

## 2. 本轮做了什么（8 个提交，全部在 `fix/u11-e2e`）

| 提交 | 内容 | 为什么 |
| --- | --- | --- |
| `8b13af7f` | magnet/ed2k → 115 云下载（`POST /offline`），新状态 `offline`，设置 `transfer115.offline_dir` | 原先这类链接被记 `unsupported`，等于丢弃 |
| `53537d7f` | `telegram.channels[]`：每频道 5 模式位 + 游标状态，旧配置自动迁移 | 单模式没法「这个频道追更、那个只入新」 |
| `5d8a5157` | Lux 代理 `GET /admin/archive/telegram/channels` | 浏览器不能直连旁挂（跨源、无 CORS） |
| `4d3783f8` | 设置页频道勾选列表（刷新频道 → 勾启用 + 四模式位） | 旧 UI 一改下拉就会按旧模式重建频道 |
| `7800460b` | 判定链 `POST /admin/archive/telegram/plan`：入新/追更/洗版/跳过 + 原因 | 判定需要 catalog，只有 Rust 侧有 |
| `264f73ca` | 旁挂 `planner.py` + 首轮只建游标 + `plan_url`/`missing_url` | 让判定真正跑起来；防历史灌库 |
| `37830f71` | 补集 `GET /admin/archive/telegram/missing` + 旁挂 `complete` 命令 | 缺集要能主动找回 |
| `fb16aed7` | 回执三态 `planned/uncertain/submitted` | 同一条链接不能转存两次 |

关键实现位置（指针，不复制内容）：

- 判定与缺口：`src/application/telegram/mod.rs` —— `decide_one()` 是规则本体，`improvement()` 是洗版尺度，
  `season_gap()`/`series_gaps()` 是补集缺口，`normalize_candidate()` 负责把发布名洗成作品名
- 存储：`src/storage/catalog.rs` 的 `telegram_library_snapshot()` —— 一次批量查作品 + 剧集 + source 清晰度/体积
- 接口：`src/api/admin/admin_jobs.rs` 里的 `admin_archive_telegram_{plan,missing,channels}`（在 `/archive/telegram/*` 路由组里）
- 侧车判定：`sidecar/telegram/tgwatch/planner.py`；接入点在 `tgwatch/runner.py` 的 `process()` / `_plan()` / `complete()`
- 回执：`tgwatch/store.py` 的 `plan_receipt()` / `mark_receipt()` / `trim_receipts()`，投递侧在 `runner.deliver()`
- 115 离线：`sidecar/transfer115/transfer115/service.py` 的 `TransferService.offline()`

## 3. 决定（区分是谁定的）

**用户定的**：

- 判定放 Rust（选项 A）：catalog 和权重逻辑都在 Rust，不把媒体权重搬到 Python
- 「依你的，先做，如果不行再改」—— 允许先落地再迭代
- 磁力/ed2k 走 115 云下载，参考 daquan；TG v2 重参考 daquan，且与补集是一体的
- 本轮收尾顺序：先补集 → 回执三态 → 先推分支 → **先只推分支、不开 PR**

**我（助手）定的，用户未反对，可推翻**：

- 离线目录与转存目录分开（`离线下载` vs `转存目录`）：离线任务在 115 后台下载，未完成的目录不能被归档当「已转存」
- 洗版只在「严格更好」时发生：清晰度更高，或同清晰度体积大 10%；同清晰度同大小一律跳过
- 缺口只算 `1..=已播最大集`：没有排期数据，不猜「下一集」
- 缺口截断保留 `missingTotal` 真值（上限 500 条），不写「缺 500 集」糊弄
- `uncertain` 回执不自动重发，只允许 `replay --force` 人工放行
- 补集「按剧名搜、按集号筛」：Telegram 搜索是整词匹配（见 §7）
- 回执清理只淘汰最老的 `submitted`，`planned`/`uncertain` 永不淘汰

## 4. 当前状态

**完成并在线验证过**：8 个提交对应的全部功能（细节见 §5）。

**完成但只在测试进程里跑过**：`lux-telegram:test`（`d992b40e…`）——生产容器 `lux-telegram-test` 仍是旧镜像 `9fc278eb…`，
本轮没有替换它（避免在验证期间动用户正在用的旁挂）。

**配置现状（已被我清空，不是用户配的）**：`telegram.channels = []`、`listen_channels = []`、
`follow_list = []`。也就是说现在旁挂重启后是空转状态，需要重新勾选频道才会开始工作。

**未开始**：

- 生产 compose 接线：`LUX_TG_LUX_BASE` / `LUX_TG_PLAN_URL` / `LUX_TG_MISSING_URL`，以及换新镜像
- `docs/archive-pipeline.md` 的 Telegram 段落重写（现在仍写「单模式 + 只投递」）
- 把 8 个提交推上远端（**NAS 没有 GitHub 凭据**：无 `gh`、无 SSH key、无 `.netrc`，远端是 HTTPS 只能匿名读）

**已经准备好的推送材料（machine-local）**：

- bundle：`/vol5/1000/build/lux-u11-e2e.bundle`（33MB，`git bundle verify` 通过，含完整历史）
- PR 描述草稿：`C:\Users\liqiang\AppData\Roaming\dsh-desktop\harness\tmp\lux-fix\PR-fix-u11-e2e.md`

**挂起**：用户已选「给我 PAT 我直接推」，但还没贴 token，所以推送没执行。

## 5. 验证记录（可复跑）

| 套件 | 命令 | 结果 |
| --- | --- | --- |
| Rust | `cargo test --locked --lib`（容器见 §1） | 1136 passed / 0 failed / 26 ignored |
| 侧车 transfer115 | `docker run --rm --entrypoint python -v $PWD:/src -w /src chillne/cinechill:latest -m unittest discover -s tests -t .` | 33 passed |
| 侧车 telegram | 同上，目录换 `sidecar/telegram` | 242 passed |

线上证据（都可复跑，脚本在 `tmp/lux-fix/`，machine-local）：

- **离线下载**：投 magnet → 侧车 `/offline` → 115 接受 → `状态.jsonl` 记
  `{"status":"offline","reason":"已提交离线下载任务（目标目录 cid=3546816758461499144）…"}`；
  测试任务已从 115 删除（`clouddownload_task_cnt` 回到 0）
- **频道列表**：`GET /api/v1/admin/archive/telegram/channels` → 24 个真实频道
- **设置页**：浏览器勾选「爱影热更频道 + 入新」保存 → 落盘 24 条 channel，页面显示「已生效」
- **判定链**：`都市古仙医 S01E720 1080p` → `follow`（缺 S01E720）；`梦幻之地 2160p WEB-DL` → `skip`（版本不比它差）；
  关掉四个模式 → 全 skip；频道没配置 → 404
- **补集**：武神主宰缺 670 集（前 500 列出 + notes 说明截断）；dry-run 扫 30 条真实消息、命中 0（频道最近只发到 E70x）
- **回执**：黑洞地址 `10.255.255.1:9` + 1s 超时 → 回执 `uncertain` → 自动重试拒绝重发 → 条目 `held/receipt:uncertain`

期间修掉的两个真问题（下一轮别重犯）：

1. **判定必须在查库之前归一化标题**。侧车送的是原始发布名（`都市古仙医 S01E221 1080p`），
   先查库再归一化会让所有追更误判成入新。现在 `admin_archive_telegram_plan` 先 `normalize_candidate()` 再取 titles。
2. **STRM 行的 `size` 可能是几百字节**。用它做洗版体积比较会算出「体积大 3355704598%」，
   现在比较前要求双方都 ≥ 1 MiB。

## 6. 已知边界（有意为之，不是 bug）

- 缺口只覆盖 `1..=已播最大集`；不猜季/集之后的内容
- 缺口清单最多列 500 条（`MAX_SEASON_GAP`），真实总数在 `missingTotal`
- 回执按 **URL** 去重，跨频道也去重；上限 10000，只淘汰 `submitted`
- 补集每季最多保留 200 个缺集去做匹配（`COMPLETION_PER_SEASON`）
- 首轮只建游标：新加的频道第一次上线不处理任何历史
- 判定接口不可达时退回本地 `filters` 规则（该挂起仍挂起），不会「什么都转」或「什么都不转」

## 7. 坑（下一轮最容易踩的）

- **Telegram 搜索是整词匹配**：搜 `武神主宰 S01E01` 一条都没有，搜 `武神主宰` 才有。
  补集因此按剧名搜、按集号正则筛（`episodes_in_text`）
- **`docker exec … python -` 不读 stdin**：要么 `-i`，要么把脚本 `docker cp` 进容器再跑；
  容器里还要 `PYTHONPATH=/app`，否则 `import tgwatch` 失败
- **旁挂容器要以 `--user 0` 跑**：设置文件是 root 所有，默认 uid 10001 读不到
  （正式 compose 靠 `group_add` 解决，临时容器直接 `--user 0`）
- **Telegram 搜索/频道操作会真的打到用户账号**：验证记得用 dry-run，跑完清理频道
- **115 离线任务会真的进用户账号**：假哈希也会建任务，验证完要 `clouddownload_task_del({"hash[0]": …})`
  （注意是 `hash[0]` 不是 `info_hash`，后者返回 success 但什么都不删）
- **`test_config.py` 里有一条旧断言**（空频道是硬错误）本轮已按当前约定改成警告，别再改回去

## 8. 下一步（一条链，按顺序）

1. **推分支**：等用户给 PAT（fine-grained，`Qoo-330ml/Lux`，Contents: RW），
   写进 `/tmp/.git-token`（600）→ 一次性 `credential.helper` → `git push -u origin fix/u11-e2e` → 立刻删 token
2. **交换机房**：生产 compose 补三个 env、换 `lux-telegram:test`（= `d992b40e…`）为新 tag，再重启旁挂
3. **文档对齐**：`docs/archive-pipeline.md` 的 Telegram 段落按 §2 重写；`docs/handoff/NEXT-SESSION.md` 指向本文件
4. **可选加强**：把 `complete` 做成常驻定时任务（现在只有手动 `complete` 子命令）

## 9. 相关技能

- `ce-handoff`（本文件的来源规范）、`ce-work` / `ce-plan`（接续实现）
- `reverse-engineer-anything` / REA：需要再读 daquan 参考实现时用（源码在 `tmp/daquan-src/`，machine-local）
