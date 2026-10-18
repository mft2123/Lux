# 修复任务：通知 + 设置（验收整改）

依据：通知/设置验收报告（结论 PASS with issues）。提交到分支 `feat/notify-settings`（基于 `feat/settings` 的 87a77c48），不直接合并。

## A. 通知模块 env 优先级（必须改）
- 问题：`notify/config.rs:304-343` 的 `apply_env` 无条件覆盖文件值；`module-notify.md:59` 也写"环境变量优先于文件"，与 `archive-pipeline.md` §8（文件优先）和 `settings.rs` 冲突
- 改法：env **只补文件缺失的键**；或直接消费 `ArchiveSettings::load()`，删除 notify 自己的 loader
- 文档：`module-notify.md:59` 改为"配置文件为准，env 仅补缺失项"
- 验收：同时设置文件与环境变量，发送使用文件值；单测覆盖

## B. 单文件挂载 PUT 失败（必须改）
- 问题：`settings.rs:492-519` 用"临时文件+rename"覆盖 `/config/archive-settings.json`，当该路径是单文件 bind mount 时返回 `EBUSY`（500）
- 改法：`rename` 失败且 `kind() == CrossesDevices | ResourceBusy` 时退化为**原地写入**（write + 保留 0600），日志 WARN
- 文档：`archive-pipeline.md` 注明"推荐挂载 /config 目录；单文件挂载也可用（原地写）"
- 验收：单文件挂载下 PUT 返回 200，内容生效，权限仍 600

## C. 重复事件（需要决定）
- 二选一：增加 `duplicate` 通知类别 + 逐条事件；或明确不做，只在汇总里计数
- 若不做，在 `module-notify.md` 写清理由，避免下次验收再提
- 默认建议：**加** `duplicate` 类别（逐条，受通知项开关控制），因为洗版/重复是用户关心的信息

## D. PUT 不落盘 env 派生值
- 问题：`ArchiveSettings::load()` 先合并 `from_env()`，`save()` 又把整结构写回；首次 UI 保存后 LUX_ARCHIVE_* 被固化进文件，之后改 env 失效
- 改法：`save()` 只写"文件自有字段"（记录哪些键来自文件），env 值不写入
- 验收：只在 env 设置 watch_dirs、UI 保存后文件中不出现该值；env 仍生效

## E. 小项（能改则改）
- `api_base` 在 status 里打码（可能含凭据）
- 通知模块读配置失败时至少 WARN 一次（不要静默）
- 统一 chat_id 打码格式（head6+len，与 settings.rs 一致）
- 消息长度按 UTF-16 码元计算（4096 限制），3900 预算保留
- `playback.rs:1928` 的通知上下文查询移到 `state.webhooks` 早退之后，或按 `webhooks.is_some() || notify 已配置` 短路

## 交付
每条修复一个提交；跑 `cargo test --locked --lib notify`、`--lib archive`（都要全绿）；更新 `docs/handoff/module-notify.md` 与 `PROGRESS.md`。汇报：分支、每条提交哈希、测试结果。
