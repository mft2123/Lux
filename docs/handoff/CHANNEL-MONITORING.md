# TG 频道监听 v2：频道清单 + 每频道多模式（daquan 对齐）

> 目标：旁挂登录后能**看到账号里有哪些频道**，勾选监控哪个，并为每个频道单独勾选模式；
> 模式语义与 daquan（`chendaquan/daquanclaw`，源码可读的 Node 版）保持一致。
> 依据：daquan 的 `telegram-channel-policy.js` + `telegram-channel-account.js`（2026-10-20 实读）。

## 1. 模式（5 个可叠加布尔）

| key | 界面名 | 含义 |
| --- | --- | --- |
| `newRelease` | 入新 | 库里没有这部作品时投递 |
| `follow` | 追更 | 剧集、已订阅且命中订阅列表时，补入缺季缺集 |
| `full` | 全量 | **允许入新+追更+洗版全部动作**（不是「无条件全投」） |
| `remux` | 洗版 | 库里已有，且新版本质量确实更好时才替换 |
| `manual` | 手动 | **派生值，不是独立开关**：其余四个都为 false 时才为 true（daquan `normalizeTelegramChannelModes`） |

## 2. 数据模型

```jsonc
telegram: {
  "channels": [
    { "id": "-1001234567890", "title": "爱影", "username": "aiying",
      "enabled": true,
      "modes": { "newRelease": true, "follow": true, "full": false, "remux": false, "manual": false } }
  ]
}
```

- `id` 用 Telethon 的 `dialog.id`（负数），不用标题做键（标题会改）。
- 旧配置 `listen_channels: ["@x"]` + `listen_mode: "全量"` 迁移为 `channels[{id:"@x", modes:{full:true}}]`；
  `listen_channels` 与 `listen_mode` 保留读取（兼容），写回后以 `channels` 为准。

## 2.1 频道记录（照抄 daquan `channelFromDialog`）

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

## 3. 判定矩阵（逐个文件决策，不是整条消息）

```
媒体库状态不可用        → 报错，绝不按空库处理
有 pending 回执          → 跳过（避免重复转存）
库里没有这部作品        → full || newRelease → 入新
                         否则 follow && tv && 命中订阅 → 追更
                         否则 跳过
库里有作品但缺这一集/季  → 上次 planned && newRelease → 入新
                         tv && (full || follow) → 追更
                         否则 跳过
库里已有这一集           → 没开 full/remux → 跳过
                         无权重方案/未就绪 → 跳过
                         质量信息不全 → 跳过（不降级）
                         权重比较确实更好 → 洗版
```

`manual` 在以上所有分支之前生效：只登记到状态库，不投递。

## 4. 接口

| 方向 | 接口 | 说明 |
| --- | --- | --- |
| 页面 → Lux | `POST /api/v1/admin/archive/telegram/channels` | 让服务端代取频道清单（浏览器到不了侧车） |
| Lux → 侧车 | `GET {LUX_TG_SIDECAR_BASE}/channels` | 侧车用 `iter_dialogs` 取全量，返回 `{id,title,username,kind,last_message_at}`；分页大小 500，结果缓存 5 分钟 |
| 侧车 → Lux | `GET /api/v1/admin/archive/lookup?title=` | 判定时查库内状态（已有，60s 缓存） |

频道数量大时：侧车缓存全量清单到 `state/dialogs.json`，刷新按钮才真正重拉。

## 5. 验收标准

1. 侧车登录后，设置页点「刷新频道列表」能在 3 秒内显示账号全部频道（标题 + @用户名 + id）。
2. 勾选两个频道、分别设「入新」与「全量」，保存后 `archive-settings.json` 结构如 §2。
3. 侧车按频道过滤：未勾选频道的新消息不产生任何 ingest。
4. 同一条消息含多集时，按 §3 逐文件决策：已入库的集跳过、缺的集投递。
5. 关闭 `manual` 的频道不自动投；`manual` 频道可用 `replay` 手动放行。
6. 旧配置升级后行为不变（全量频道仍全量）。

## 6. 待定 / 风险

- Telethon `iter_dialogs` 拉数千个会话较慢（约 1–2 分钟），必须先做缓存 + 手动刷新。
- `follow` 的「订阅来源」目前是设置里的 `follow_list`；daquan 用的是媒体库订阅状态，后续可对接。
- ~~判定发生在**侧车**，需要把 Lux 的库内状态通过 lookup 暴露给它~~ → **v2 已改**：判定链在 Lux
  （`src/application/telegram/`，`POST /admin/archive/telegram/plan`），侧车只负责抽取与执行。本文件 §3
  的矩阵是当时的 daquan 对齐稿，实现以 `docs/archive-pipeline.md` 2.2.2 为准——差别在于代码里没有
  「上次 `planned` 且开入新 → 入新」那条分支，缺集一律按 `follow`/`full` 判。