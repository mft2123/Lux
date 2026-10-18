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

待做（按顺序）：

1. **module-settings**：设置文件 + `GET/PUT /api/v1/admin/archive/settings` + 前端设置页（已完成，`feat/settings`）
2. **module-notify**：Telegram bot 通知（入库/归档/失败/洗版/播放/重复…）——已实现，验收整改在 `feat/notify-settings`
3. **module-telethon**：Python 旁挂，Userbot 登录 + 频道监听 5 模式（手动/上新/追更/洗版/全量）
4. 存量迁移 + chill 退役（用户要求先跑几天测试，暂缓）

## 模块分工

| 模块 | 文档 | 负责文件 | 状态 |
| --- | --- | --- | --- |
| 设置与 UI | module-settings.md | `src/application/archive/settings.rs`、`src/api/admin/admin_jobs.rs`、`web/src/features/settings/…` | 已完成（`feat/settings`） |
| 通知 | module-notify.md | `src/application/notify/`、admin 路由 | 验收整改（`feat/notify-settings`） |
| Telethon 旁挂 | module-telethon.md | 新目录 `sidecar/telegram/`（Python） | 待开工 |

## 审核规则（总控对话框用）

- 每个模块合并前：`cargo test --locked --lib archive` 全绿、前端 `pnpm build` 通过、在测试库跑一次 dry-run 验证
- 模块之间不互相改文件；跨模块需求由总控对话框转达
- 每次合并后更新本文件「当前进度」与提交哈希
