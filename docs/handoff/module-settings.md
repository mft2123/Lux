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
