from pathlib import Path
p = Path("/tmp/lux-u11/docs/handoff/CHANNEL-MONITORING.md")
s = p.read_text(encoding="utf-8")
old = "| `manual` | 手动 | 只登记，不自动投递；由管理员 `replay` 触发 |"
new = ("| `manual` | 手动 | **派生值，不是独立开关**：其余四个都为 false 时才为 true（daquan `normalizeTelegramChannelModes`） |")
assert s.count(old) == 1
s = s.replace(old, new)

anchor = "## 3. 判定矩阵（逐个文件决策，不是整条消息）"
add = """## 2.1 频道记录（照抄 daquan `channelFromDialog`）

```js
id:         "telegram:<去掉 -100 前缀的 id>"   // 稳定键，标题可改
telegramId: entity.id || dialog.id            // 原始负 id
username:   去掉 @ 的用户名
title:      dialog.title || name || entity.title || username || "未命名频道"
enabled:    true                              // 新发现默认监控
modes:      normalizeTelegramChannelModes()   // 全 false → manual=true
```

- 只取 `dialog.isChannel && dialog.entity`（**不含群组**）。
- `getDialogs({})` 必须包 60s 超时；失败即断开重连，否则一次卡死会拖住整个旁挂。
- 匹配用 `id | telegramId | username` 三者任一（daquan `channelMatches`）。
- 每频道另有游标状态：`{ initialized, lastMessageId, lastRunAt, lastStatus, lastError, transferredCount }`
  —— 靠 `lastMessageId` 增量，**不是靠时间戳**，重跑不会重复处理（daquan `normalizeChannelAutomation`）。

"""
assert s.count(anchor) == 1
s = s.replace(anchor, add + anchor)
p.write_text(s, encoding="utf-8")
print("doc patched")