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
