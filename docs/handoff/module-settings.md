# 模块：设置与 UI（module-settings）

目标：所有可调参数与凭证**手动输入、随时可改**，不写死；改完立即生效（无需重启）。

## 交付

1. `src/application/archive/settings.rs`
   - `ArchiveSettings`（serde）：tmdb_api_key、watch_dirs、mirror_root、cd2_prefix、cd2_mount、
     poll_seconds、settle_seconds、wash_enabled、wash_tolerance_ratio、telegram{bot_token, chat_id,
     api_id, api_hash, phone, notify, listen_mode, listen_channels, follow_list}
   - 位置：`LUX_ARCHIVE_SETTINGS` 环境变量，默认 `/config/archive-settings.json`；文件不存在时用 env 默认值，
     首次保存时创建（0600）
   - `load()` / `save()`；`masked()` 用于 API 回显（token 只显示前 6 位 + 长度）
2. 管理接口（`src/api/admin/admin_jobs.rs`）
   - `GET /api/v1/admin/archive/settings`：返回打码后的设置
   - `PUT /api/v1/admin/archive/settings`：整体覆盖写入（校验：路径存在性、容差 0–1、poll 0–3600）
   - `ArchiveConfig::from_env()` 改为「先读设置文件，缺失项回退 env，再缺回退默认」
3. 前端 `web/src/features/settings/`（或现有设置页内加一节）
   - 分组：TMDB、目录与路径、洗版参数、Telegram（bot + userbot + 通知项 + 监听模式）
   - 密钥输入框 type=password，留空表示"不修改"
   - 保存后提示"已生效"，并调用 `GET status` 刷新
4. 测试：settings 解析/打码/保存往返；PUT 校验失败用例

## 约束

- 不把凭证写进日志；API 响应永远打码
- 设置文件不在 git 仓库内（/config 是容器卷）
- 改 `ArchiveConfig::from_env` 时保持现有 22 个 archive 测试全绿

## 交付状态（2026-10-18，分支 `feat/settings`）

已完成：

- `src/application/archive/settings.rs` — 设置文件模型与读写
  - 路径 `LUX_ARCHIVE_SETTINGS`，默认 `/config/archive-settings.json`；父目录自动创建；临时文件 + rename 写入，权限 0600
  - `load()`：**文件 > 环境变量（`LUX_ARCHIVE_*`）> 默认值**，逐字段回退；文件缺失或损坏只告警，回退 env，不影响启动
  - `masked()`：tmdb_api_key / bot_token / chat_id / api_hash / phone 只返回「前 6 位 + 长度」，空值保持空
  - 更新时空凭证（或原样回传的打码值）保留原值；`notify` 保留数组 / 对象 / null 三种形态
  - 保留通知模块的 telegram 调优键（enabled / api_base / message_thread_id / silent / min_interval_ms /
    max_per_minute / dedupe_seconds / timezone_offset_hours），本模块保存不会丢
- `ArchiveConfig::from_env()` 改为读设置文件，缺失项回退 env；`LUX_ARCHIVE_ROOT` 与 `LUX_TMDB_LANGUAGE`
  仍只由环境变量控制（它们描述部署挂载，而不是流水线行为）
- 轮询器每轮重读设置：间隔、监听目录、TMDB key、洗版规则都在下一轮生效；`poll_seconds=0` 时任务 idle 等待，
  改成非 0 后无需重启
- 管理接口 `GET/PUT /api/v1/admin/archive/settings`（PUT 需要 CSRF，admin 权限）
  - PUT 校验：容差 0–1、poll / settle 0–3600、镜像库与监听目录必须存在且为绝对路径、CD2 挂载点必须绝对路径
  - 返回体是同一份设置文档（打码），前端因此可以原样回传
- 前端 `web/src/features/settings/ArchiveSettingsSection.tsx` + `archive-settings.css`，挂在
  `AdminSettingsPage` 末尾；分组：TMDB / 目录与路径 / 洗版参数 / Telegram 通知 / Telegram 频道监听（Userbot）；
  密码框留空表示不修改；保存后提示「已生效，无需重启」并刷新 `GET status`
- 文档 `docs/archive-pipeline.md` 第 7/8/9 节：设置文件、字段结构、两个新接口

测试：`cargo test --locked --lib archive` → 34 通过（新增 12 个 settings 测试，原有 22 个全绿）；
`pnpm build`（`tsc --noEmit && vite build`）通过。

边界与风险：

- 设置文件归本模块所有；通知模块与 Telethon 旁挂只读
- 环境变量优先级不一致：本模块是「文件 > env」，通知模块对 `LUX_TG_*` 是「env > 文件」；
  当前部署没有设置 `LUX_TG_*`，暂不影响，但对齐时应统一
- 通知类别清单在前端硬编码了一份（播放/入库/整理/洗版/转存/115妙传/媒体整理/签到/其他任务/失败），
  通知模块以后新增类别需要同步
- 未在浏览器里实际点过页面（只过了 tsc + vite build），也没有在真实容器里调过接口；
  合并前建议在测试库用 admin API Key 各调一次 GET/PUT 验证

