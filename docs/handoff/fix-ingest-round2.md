# 归档 / ingest 组整改单（第二轮 · 2026-10-18）

审核对象：`feat/telegram-ingest` @43f152b1（含 f45a9ecf 的链接队列 ingest/lookup）的
`src/application/archive.rs`、`src/api/admin/admin_jobs.rs`、`src/api/admin/admin_common.rs`、
`docs/archive-pipeline.md` §2.1/§9。
决策见 `REVIEW-BUS.md` D1–D10。

结论：**CHANGES**。契约与鉴权主干正确、路径穿越防护完整、消费器语义符合规范；
剩下 1 条严重（失败隔离）和 5 条一般项。

## 一、必须修（合并前）

### I1 [严重] 队列消费失败会让整轮归档不执行
- 位置：`archive.rs:1610-1616`（`append_link_state(...)?` / `move_entry(...)?`）、`archive.rs:544`
  （`ingest_link_queue` 在 `run_once` 最前，失败即整轮返回）
- 现象：单条作业的 IO 问题（磁盘满、目录权限、`已处理/` 里重名）→ `run_once` 直接返回 Err，
  watch 目录→转存→待整理→镜像 全部不执行；轮询下每轮重复失败，等于永久卡死。
  另外「状态已追加、移动失败」的作业下一轮会重复追加一条 `needs_115_client`。
- 改法：消费器逐条 best effort——单条失败记入 `skipped`/`failed` 并继续；
  状态追加与移动做成可重入（先移动再记账，或记账前检查 `已处理/` 里是否已有该作业）。
- 验收：构造一条不可移动的作业（目标已存在同名目录）→ 本轮其余条目照常归档，
  该作业只记一次账；`cargo test --locked --lib archive` 全绿。

### I2 [一般] ingest 的 check-then-write 无锁；jobId 与文件名可能不一致
- 位置：`archive.rs:1440`（查重）→ `1454-1465`（写临时文件 + `unique_target` + rename）
- 现象：并发两个同 URL POST 都写一份；第二个被命名成 `… (2).json`，而 202 返回的 `jobId` 仍是内部 id，
  与文档「job_id 就是文件名」冲突；两份都会被消费。
- 改法：进程内 `Mutex`（ingest 与 run 已在同一 service 内）或 `O_EXCL` 抢占；
  落盘后把最终文件名回写进 202 响应。
- 验收：并发 10 个同 URL POST → 队列里只有 1 个作业文件，10 个响应 `duplicate` 语义正确。

### I3 [一般] 作业文件 0644 vs 状态文件 0600
- 位置：`archive.rs:1460`（`std::fs::write`）对照 `2657-2667`（`OpenOptionsExt.mode(0o600)`）
- 现象：同一条含 `?password=` 的链接，两处权限不同；移入 `已处理/` 后仍是 0644。
- 改法：作业文件同样 0600（临时文件打开即设 mode），rename 保留权限。
- 验收：`ls -l 链接队列/*.json` 与 `已处理/*` 都是 0600。

### I4 [一般] 日志明文打印完整分享链接（含 password）
- 位置：`archive.rs:1441-1446`、`1466-1472`、`1617-1623`
- 改法（按 D7）：新增 `mask_link_for_log(url)`，只留 `scheme://host`（magnet 只留 `magnet:?xt=urn:btih:<前8位>…`）；
  所有 `url = %...` 日志点改用它；补一条断言脱敏的单测。
- 验收：ingest/consume/duplicate 三条路径的日志都不含 `password=` 与完整链接。

### I5 [一般] lookup 名称启发式误匹配
- 位置：`archive.rs:2687-2701`（包含/前缀规则）、`2717-2733`（每一层都匹配）
- 现象：`流浪地球2` ↔ `流浪地球`、`繁花` ↔ `繁花似锦`、跨年份同名都命中；
  匹配还发生在 `类型/地区/年份` 目录层，标题恰好是年份时会返回整年体积。错误体积会让旁挂把真升级判成重复。
- 改法：只在标题层（深度 3/4）匹配；要求年份段一致或词边界一致；短标题（≤3 字）只做精确匹配。
- 验收：新增三组单测（系列第 1/2 部、同剧不同年、短标题），错误的匹配不再命中。

### I6 [一般] lookup 每请求全树遍历 + 递归求和，无缓存，跟随符号链接，阻塞 async worker
- 位置：`archive.rs:2705-2736`、`2019-2033`（`path_size` 无深度上限）、`admin_jobs.rs:1489-1492`
- 改法：至少（a）限制匹配层数、（b）用 `symlink_metadata` 不跟随符号链接、
  （c）`spawn_blocking` 执行、（d）命中结果缓存 N 秒。18 万条目下给出耗时预算（目标 <200ms）。
- 验收：18 万条目规模的 dry 测试（可用合成目录）下 P95 < 200ms；自指符号链接不会无限递归。

## 二、小项（合并后可排期）

- I7 幂等键=URL 永久有效，无 TTL/无重投接口，`状态.jsonl` 只追加不轮转 → 文档写明手工解除方式 + 轮转策略。
- I8 `mode` 不校验（`archive.rs:308`）→ 与 `settings.rs` 的 `LISTEN_MODES` 共用校验，非法 400。
- I9 「什么算作业」消费者与 `count_link_jobs` 两套判定（大小写/符号链接）→ 抽一个共用函数。
- I10 URL 校验过浅：`https://` 空目标也算合法；http(s) 未限制主机（U11 接入后是 SSRF 面）→ 要求非空 host + 域名白名单。
- I11 `normalize_title_key` 未做全半角归一（`０-９`/`Ａ`）→ NFKC 后再比对。
- I12 读失败被当成"没有重复"、隔离失败只 WARN、坏行静默跳过 → 读失败跳过本轮并记 WARN；坏行有修复手段。
- I13 202 返回的 `status: needs_115_client` 是目标状态；`poll_seconds` 默认 0 时队列不会被消费 →
  202 返回 `queued`，文档写明默认关闭轮询时需手动 run。
- I14 duplicate 响应里的 `file` 可能指向不存在的重命名文件 → 状态里存实际落盘相对路径。
- I15 `Json(job)` 提取器在鉴权前执行，类型错误返回 axum 422 而非 `api_error` 400 → 先鉴权再解析。
- I16 [小项] 文档写 `dry_run=1|0`，接口只接受 bool（`?dry_run=1` → 400 `provided string was not true or false`）。
  `docs/archive-pipeline.md` §9 与 `POST …/run`、`…/metadata` 三处对齐：要么文档改成 `true|false`，
  要么 query 解析兼容 `1/0/true/false`（审核组实测踩到，建议兼容，脚本更顺手）。

## 三、已核实无问题（不用重做）

- 鉴权分支顺序：共享 API key 先 `Ok` 且不走 CSRF；客户端令牌 403；Web 会话 + CSRF。旁挂只带 `X-Lux-Api-Key`，配对成立。
- 错误码：`Invalid`→400、`Storage`→500、成功 202，错误体统一 `api_error`。
- 路径穿越不可能：`job_id = <ms>-<slug>`，slug 只保留字母数字/CJK/`-`，上限 40；生产方 jobId 被覆盖（有测试）。
- 原子写：`.` 前缀临时文件 + rename，消费者跳过 `.` 开头文件；失败清理 temp。
- 消费器：只吃第一层 `*.json`；未稳定留到下一轮；解析失败且已稳定 → `无效/` + `invalid`；合法作业不下载、
  不写 `转存目录/`/`媒体目录/`（测试断言）。
- `run_once` 顺序：全局锁 → 链接队列 → watch dirs → 转存 → 待整理；dry-run 只统计。
- `status` 新增 `link_queue_dir`/`link_state_file`/`pending_link_jobs` 且 `#[serde(default)]` 向后兼容。
- lookup 空 title → 400；无命中 200 + null；只读元数据。

## 四、汇报格式

逐条一个提交（`fix(archive): I1 isolate per-job failures in the link queue`）。
完成后更新 `REVIEW-BUS.md` 自己那节（`状态: READY` + 证据 + 测试结果 + 遗留）。
测试要求（D10）：`cargo test --locked --lib archive` 全绿；`--lib notify`、`--lib playback` 不回归。
注意：`feat/telegram-ingest` 落后 `fork/phase-a` 一个 F1 提交（`42134498` 剧集按季判定），
开工前先 `git merge fork/phase-a`，并保持 `organize_series_pack` 的行为不被你的改动破坏。
