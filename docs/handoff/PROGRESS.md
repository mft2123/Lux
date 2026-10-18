# Lux 自建项目 — 会话分工与进度（总控文档）

> 用法：每个新对话框开场说「读 docs/handoff/PROGRESS.md 和对应的 module-*.md，然后继续」。
> 总控对话框负责审核、合并、更新本文件；模块对话框只改动自己模块的文件。

## 环境速查

- NAS 仓库：`qiangli@192.168.8.8:/vol5/1000/build/lux`（分支 `fork/phase-a`）
  - SSH：`ssh -p 4888 -i "$HOME\.ssh\id_ed25519_feiniu" -o BatchMode=yes qiangli@192.168.8.8 "echo <b64> | base64 -d | bash"`
  - 大于几 KB 的文件先本地写再 `scp`（避免命令过长）
- 构建：`DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .`
- 测试：`docker run --rm -e CARGO_HOME=/cargo-home -e RUSTUP_TOOLCHAIN=1.94.1 -e RUSTUP_DIST_SERVER=https://rsproxy.cn -v /vol5/1000/build/cargo-home:/cargo-home -v /vol5/1000/build/lux:/src -v /vol5/1000/build/target:/src/target -w /src rust:1.94-test bash -c "cargo test --locked --lib <filter>"`
- 测试容器：`lux-testlib`，端口 8099，admin `admin` / `Test@12345a`，配置 `/vol5/1000/build/archive-test-config`
  - 环境：`LUX_ARCHIVE_ROOT=/testlib  LUX_ARCHIVE_MIRROR_ROOT=/library  LUX_ARCHIVE_CD2_PREFIX=测试库  LUX_ARCHIVE_CD2_MOUNT=/media/cloud  LUX_ARCHIVE_WATCH_DIRS=/testlib/外部上传  LUX_ARCHIVE_POLL_SECONDS=10  LUX_ARCHIVE_SETTLE_SECONDS=3`
  - 挂载：`测试库` → `/testlib`，`/vol5/1000/docker/lux-library` → `/library`
- 测试库（唯一允许写的位置）：`/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库`
- 本地镜像库：`/vol5/1000/docker/lux-library`（strm + NFO + 海报，媒体服务器扫描这个）
- 设置模板：`/vol5/1000/build/archive-settings.json`（chill 的 TG 凭证已预填，权限 600）
- 硬约束：生产媒体库只读、绝不读取；chill/Emby/CD2 不做任何修改；不启用 Lux STRM 远程探测

## 当前进度（2026-10-18）

已完成并提交（`git log --oneline origin/test..HEAD`）：

- 阶段 A 稳定性 7 个单元（首页缓存、latest SQL、可用性 rollup、毒批次熔断、WAL 回收、目录筛选）
- 归档流水线：转存监控（多来源）→ 待整理 → 媒体目录 → 失败/重复/洗版
  - 结构：`媒体目录/<类型>/<国内|国外>/<年份>/<片名 (年份)>`；`晚会` 不分地区；`演唱会` 分国内/国外
  - TMDB：双语搜索、发行地判国内/国外、年份多级回退、晚会/演唱会关键词识别
  - 体积判定：容差内重复、明显更大洗版、更小重复
  - 本地镜像：strm（CD2 路径）+ NFO + poster/fanart，权限 755/644
  - 管理接口：`GET /api/v1/admin/archive/status`、`POST .../archive/run?dry_run=`、`POST .../archive/metadata?dry_run=&overwrite=`
  - 规范文档：`docs/archive-pipeline.md`（改行为先改它）
- 测试：`cargo test --locked --lib archive` 22 个通过
- **Telegram 旁挂 · Lux 侧交接**（分支 `feat/telegram-ingest`）：
  `POST /api/v1/admin/archive/ingest` 把链接写成 `<archive_root>/链接队列/<ts>-<slug>.json`（按 URL 幂等）；
  流水线每轮消费队列，登记 `<archive_root>/链接队列/状态.jsonl`（`needs_115_client`）并移入 `已处理/`，
  **不下载、不转存**（115 转存客户端 U11 未实现）；`GET …/archive/lookup?title=` 供洗版查库中体积；
  `GET …/archive/status` 新增 `link_queue_dir` / `link_state_file` / `pending_link_jobs`

待做（按顺序）：

1. **module-settings**：设置文件 + `GET/PUT /api/v1/admin/archive/settings` + 前端设置页
2. **module-notify**：Telegram bot 通知（入库/归档/失败/洗版）
3. **module-telethon**：Python 旁挂，Userbot 登录 + 频道监听 5 模式（手动/上新/追更/洗版/全量）
4. 存量迁移 + chill 退役（用户要求先跑几天测试，暂缓）

## 第二轮审核（2026-10-18 · 审核组）

- **F1 剧集包按季判定已修并合入 `fork/phase-a`**（`42134498`）：剧集包按每个 `Season NN` 独立判定，
  缺季入库、同季重复进 `重复目录/<包名>/Season NN`、更大季洗版；全季重复才退回整包判定。
  - 单测 `cargo test --locked --lib archive` **34 通过**（新增 3 个季判定用例）
  - 测试库 `聊斋 (1996)` 数据已修：库里有 Season 01+02，重复目录只剩 Season 01，镜像 80 个 sidecar 就位（strm 目标逐条校验通过）
  - 规范：`docs/archive-pipeline.md` §5.1
- 四个在飞模块本轮**全部 CHANGES**，整改单：`fix-settings-round2.md` / `fix-notify-round2.md` /
  `fix-sidecar-round2.md` / `fix-ingest-round2.md`；决策见 `REVIEW-BUS.md` D1–D10。
- 合并门禁与状态机见 `WORKFLOW.md`；镜像仍只由总控构建（本轮 `lux:phase-a` 由审核组重建）。

## 模块分工

| 模块 | 文档 | 负责文件 | 状态 |
| --- | --- | --- | --- |
| 设置与 UI | module-settings.md | `src/application/archive/settings.rs`、`src/api/admin/admin_jobs.rs`、`web/src/features/settings/…` | CHANGES（`feat/notify-settings`），见 fix-settings-round2 |
| 通知 | module-notify.md | `src/application/notify/`、admin 路由 | CHANGES（`feat/notify-settings`），见 fix-notify-round2 |
| Telethon 旁挂 | module-telethon.md | Lux 侧 `src/application/archive.rs` + admin 路由；Python 侧车在 `sidecar/telegram/` | CHANGES（`feat/telegram-sidecar`），见 fix-sidecar-round2 |
| 归档 / ingest | archive-pipeline.md | `src/application/archive.rs` + admin 路由 | CHANGES（`feat/telegram-ingest`），见 fix-ingest-round2；F1 已合并 |


## 审核规则（总控对话框用）

- 每个模块合并前：`cargo test --locked --lib archive` 全绿、前端 `pnpm build` 通过、在测试库跑一次 dry-run 验证
- 模块之间不互相改文件；跨模块需求由总控对话框转达
- 每次合并后更新本文件「当前进度」与提交哈希

## 第三轮：审核组接手实现（2026-10-18）

用户决定「以后所有模块都由审核组做」，于是从本轮起审核与实现合并到同一个对话框。
工作分支 `integration/round2`（`fork/phase-a` + settings/notify/sidecar 三个模块分支 + 本轮的整改提交）。

已完成：
- 设置：S1/S2(F4)/S3/S4/S5/S7/S10/S11/S12（越界 400、provenance 保存、`telegram.api_key`、
  原地写备份、对象型 channels 保留、PUT 串行、打码、类型校验、按改动校验路径）
- 通知：N1/N2（类别由后端下发、保存不改形态）、N3（播放通知 IP）、N4（逐条去重键）、N5（发送成功后再记账）、
  N12（Debug 打码）
- 旁挂：SC1–SC6、SC8、SC11、SC14–SC18（模块方实现）+ 测试清理与 inbox 0644（审核组补）
- 归档：F1 剧集按季判定（前一轮已合入 `fork/phase-a`）；ingest I1–I6/I8–I12（队列失败隔离、并发锁、
  0600、日志脱敏、lookup 标题层/缓存/spawn_blocking、mode 校验、全角归一、读失败不再当无重复）

测试：archive 71 / notify 41 / playback 97 / sidecar 184 全绿；
整库 `cargo test --lib` 仅剩一个**既有** metadata 用例不稳定（trunk 上同样失败，见总线「已知的 trunk 问题」）。

待办：
- 重建 `lux:phase-a` 镜像，`lux-testlib` 重建，端到端验收（设置保存语义、通知类别、旁挂投递）
- 第三轮收尾已完成：S8、N6–N13、SC7/SC9/SC10/SC12/SC13、I7/I13/I14
- U11（115 转存客户端）仍是功能链最后缺口（新功能，不是整改项）
