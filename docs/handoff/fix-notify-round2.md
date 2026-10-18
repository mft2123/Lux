# 通知组整改单（第二轮 · 2026-10-18）

审核对象：`feat/notify-settings` @1b90afb4 的 `src/application/notify/`（config/event/format/mod/send）、
`src/api/playback.rs` 接入点、`src/application/archive.rs` 的 `run_once` 接入点。
决策见 `REVIEW-BUS.md` D1–D10。

结论：**CHANGES**。A/C/E 三条整改代码属实且实现正确，但被设置页抵消，另有 4 条真问题。

## 一、必须修（合并前）

### N1 [严重] 设置页类别清单缺「重复」，保存一次就永久关掉 duplicate
- 位置：`web/src/features/settings/ArchiveSettingsSection.tsx:16-27`（10 项硬编码）、`:533`（payload 回写数组）、
  `:575-595`（渲染）；`notify/config.rs:31,79,115`（11 个类别）
- 现象：`telegram.notify` 缺省 `null` = 全部开启；数组 = 精确选择。管理员在 UI 里保存任意设置，
  前端把 10 项数组写回，`duplicate` 从此被静默关闭，UI 里也没有开关能再打开它。
  C 条验收在 UI 路径上不成立。
- 改法（按 D5）：settings 响应带 `known_categories`（key + 中文标签），前端只渲染后端给的列表；
  不再硬编码。
- 验收：`notify: null` → UI 保存后文件里仍是 `null`（或 11 项全开），duplicate 事件仍逐条发出。

### N2 [一般] 对象形态归一成数组，丢掉「未列出=开启」
- 位置：`ArchiveSettingsSection.tsx:585-592`
- 现象：`{"playback":true}` 保存后变成 `["播放"]` = 只开播放，其余类别被关掉。
- 改法：前端保持原形态（对象→对象，数组→数组，null→null）；`known_categories` 里用三态（开/关/未设置）。
- 验收：对象形态往返后逐键不变；null 往返后仍是 null。

### N3 [一般] 播放通知的「IP」恒为空
- 位置：`playback.rs:1676-1680`、`:3966-3970`、`:4198-4202`（只有 `event_type_is_stopped` 才传 `remote_ip`）；
  `playback.rs:1942` 只发 `PlaybackStarted` 通知
- 改法：started 事件也带上 `remote_ip`（来自请求上下文），或把这个字段从 started 卡片里去掉。
- 验收：真实播放一次，通知里 IP 非空；没有 IP 时不渲染空字段。

### N4 [一般] 失败事件无去重、无跨轮预算 → 通知风暴
- 位置：`archive.rs:591-597`（转存移动失败条目留在 transfer 目录）、`notify/event.rs:225-231`（失败事件无 dedupe_key）
- 现象：转存目录里一个坏条目每个 poll 周期重发一次失败通知，无跨轮总量上限
  （单轮上限只是 `MAX_ITEM_EVENTS_PER_RUN=10` × 5 类）。
- 改法：失败/重复逐条事件带 `dedupe_key = category:条目名:原因`，并在 `run_once` 汇总里限总量；
  同一 key 在 `dedupe_seconds` 内只发一次。
- 验收：同一失败条目连续 3 轮 → 只收到 1 条；`GET /admin/notify/status` 的 skipped 计数可见。

### N5 [一般] 去重键在发送成功前写入
- 位置：`notify/mod.rs:175-189`、`:197`、`:251-257`
- 现象：先登记 dedupe 再发送；发送失败后同键仍被抑制一个窗口 → 通知丢失。
- 改法：发送成功后再登记，或失败时回滚该键。
- 验收：单测模拟发送失败 → 同键立刻重试不被抑制。

## 二、小项（合并后可排期）

- N6 [小项] 行内截断可能切断 HTML 实体（`format.rs:98-107` 未调用 `repair`）。
- N7 [小项] 字段先按 400 字符截断再转义，实体密集值可占约 2000 码元，挤掉后续字段。
- N8 [小项] `telegram.notify` 为字符串时 fail-open 全开（应视为非法配置，按 D6 记 WARN 并当 null）。
- N9 [小项] 每个事件都同步读盘（async 里阻塞 IO）；读失败每个调用点都 WARN（按 D9 每进程一次）。
- N10 [小项] `api_base` 未校验 scheme、允许 http、含 `?#` 时 token 落进 query（按 D7 打码 + 只允许 https）。
- N11 [小项] `run_once` 中途 `?` 早退时完全不发通知（`archive.rs:544-567`）——至少发一条失败汇总。
- N12 [小项] `NotifyConfig` / `NotifyEnv` derive `Debug` 带明文 token（加 `#[derive(Debug)]` 时手写 redacted）。
- N13 [小项] 文档写「全局串行」，实现只串行「准入」不串行发送 → 改文档或改实现。

## 三、已核实无问题（不用重做）

- A 条：`config.rs:282-337` 只消费 `ArchiveSettings::load()`，逐键 `file_value(...).or_else(env)`；
  单测 `config.rs:448-511` 覆盖「文件覆盖 env」「env 补缺」。
- `from_parts` 语义：缺省=全开、空数组=全关、对象=从全开逐项覆盖（`config.rs:137-141,170-205,288-296`）。
- C 条：`duplicate` 的 enum/key/label/parse/事件/逐条 `take(10)`/汇总计数齐全（只被 N1 抵消）。
- E 条：api_base 打码（`config.rs:395-415`）、chat_id 复用 `mask_secret`（`:348-353`）、
  3900 UTF-16 总长不变式成立、截断不破坏实体（标题与 hard limit 路径）、播放上下文短路（`playback.rs:1930-1933`）。
- 安全：token 不进日志/错误串；发送失败不影响归档/播放；`spawn_event` 有 runtime 守卫；
  HTML 转义覆盖所有动态字段；429 `retry_after` clamp 后只重试一次；限流用单调时钟，无死锁。
- 三个 admin 路由均有 `require_admin` + POST CSRF；请求体有界；dry-run 不发。

## 四、汇报格式

逐条一个提交（`fix(notify): N1 categories come from the server`）。
完成后更新 `REVIEW-BUS.md` 自己那节（`状态: READY` + 每条证据 + 测试结果 + 遗留）。
测试要求（D10）：`cargo test --locked --lib notify`、`--lib archive`、`--lib playback` 全绿 + `pnpm build` 通过。
端到端实测按 D8 执行（一次、前缀「【Lux 验收测试】」）。
