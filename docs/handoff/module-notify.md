# 模块：Telegram 通知（module-notify）

目标：归档/入库/失败/洗版/重复事件推送到 TG（bot sendMessage），凭证来自 archive-settings.json 的 `telegram` 段。

## 交付

1. `src/application/archive/notify.rs`
   - `Notifier::from_settings(&ArchiveSettings)`；`async fn send(&self, event: &NotifyEvent) -> Result<(), String>`
   - 事件：`Archived{count, dry_run}`、`Organized{title, year, category, region, target}`、
     `Failed{entry, reason}`、`Washed{title, old_size, new_size}`、`Duplicate{title}`、`Test`
   - 只发送 `telegram.notify` 里勾选的类别；bot_token/chat_id 缺失时静默跳过并 debug 日志
   - HTML/markdown 转义；单条消息 ≤ 4000 字符，超出截断
2. 接入点
   - `run_once` 结束：发一条汇总（archived/organized/failed/duplicates/washed）
   - 单条失败：发失败条目 + 原因（可选，由 notify 勾选控制）
3. 接口
   - `POST /api/v1/admin/notify/test`：发一条测试消息，返回 TG API 结果
   - `GET /api/v1/admin/notify/status`：上次发送时间、成功/失败计数（内存态即可）
4. 测试：事件 → 消息文本（纯函数）；缺失凭证时跳过；截断逻辑

## 约束

- 不在日志里打印 token 或完整消息 URL
- 发送失败不影响归档流程（best effort，记 WARN）
- 使用现有 reqwest client，超时 10s
