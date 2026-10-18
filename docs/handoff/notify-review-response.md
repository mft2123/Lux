# 通知模块验收整改反馈（提交给审核组）

- 分支：`feat/notify-settings`（基于 `feat/settings` 的 `87a77c48`）
- 依据：`docs/handoff/fix-notify-settings.md`
- 范围：本对话框只负责 TG 通知模块；设置模块（`settings.rs`）不在此分支改动

## 逐项结论

| 项 | 结论 | 提交 | 说明 |
| --- | --- | --- | --- |
| A 配置优先级 | 已改 | `80ef8046` | 删除 notify 自己的设置文件解析，改为消费 `ArchiveSettings::load()`；`LUX_TG_*` 只补文件缺失的键（文件优先）。新增纯函数 `from_parts(settings, env)`，单测覆盖「文件值覆盖 env」「env 只补缺」 |
| B 单文件挂载 PUT | **未改，属设置模块** | — | 见下方「移交给设置侧」 |
| C 重复事件 | 已改（选择「加」） | `1401b09d` | 新增 `duplicate`/`重复` 类别，逐条事件，受 `telegram.notify` 开关控制；汇总保留重复计数。按整改单默认建议执行 |
| D save 不落 env 派生值 | **未改，属设置模块** | — | 见下方「移交给设置侧」 |
| E api_base 打码 | 已改 | `191dd852` | status 只回 `scheme://host[:port]`，有凭据显示 `***`，非根路径显示 `/…` |
| E 配置读失败 WARN | 已满足（间接） | `80ef8046` | notify 不再自己读文件；`ArchiveSettings::load()` 在读取失败时已有 WARN。注：每个调用点都会 WARN 一次，如需「每进程仅一次」请审核组确认 |
| E chat_id 打码统一 | 已改 | `191dd852` | 直接复用 settings 的 `mask_secret`，格式 `head6…(len)` |
| E UTF-16 长度 | 已改 | `191dd852` | 预算改为 3900 **UTF-16 码元**，emoji 计 2；截断不破坏 HTML 实体。新增 emoji 密集消息的单测 |
| E 播放上下文短路 | 已改 | `191dd852` | `state.webhooks` 与通知都未配置时直接返回，不做条目查询；通知已配置时仍正常取上下文 |

## 验证结果

在 `feat/notify-settings` HEAD（`814fe434` 之后）实跑：

- `cargo test --locked --lib notify` → **40 passed**
- `cargo test --locked --lib archive` → **36 passed**
- `cargo test --locked --lib playback` → **97 passed**

未做真实 TG 发送验证：测试库没有挂载 `archive-settings.json`，且不应向用户 TG 主动发消息。需要端到端验证时，请审核组指定凭证来源与是否允许发一条测试消息。

## 移交给设置侧（B / D）

这两项都在 `src/application/archive/settings.rs`，本对话框按模块边界未改。给设置侧的具体建议：

**B（单文件 bind mount 的 PUT）**
- 现状：`write_private` 先写临时文件再 `rename`，单文件 bind mount 上 `rename` 返回 `EBUSY`（也可能 `EXDEV`）。
- 建议：`rename` 失败且 `error.kind()` 为 `CrossesDevices`/`ResourceBusy`（或 raw_os_error 为 EBUSY/EXDEV）时退化为**原地写入**：`OpenOptions::new().write(true).truncate(true).open(path)`，写入后 `set_permissions(0600)`，并记 WARN。
- 注意：原地写不是原子操作，写入期间读到半截 JSON 的进程会走「解析失败→回退 env」，需要在文档里注明这个窗口。

**D（save 不固化 env 派生值）**
- 难点：`apply_update` 收到的是 UI 回传的整份设置（含从 env 读到的值），仅靠「update 里出现的键」判断会把 env 值也写进文件。
- 建议方案：`load_from_with_base` 记录文件里**实际出现**的键集合（provenance），`apply_update` 只把「值与该键 env 基值不同」的键标记为 file-owned；`save_to` 只序列化 file-owned 键。这样验收场景（仅 env 设 watch_dirs，UI 保存）不会落盘。
- 该改动会触及 `ArchiveSettings` 的相等性/测试构造，需要设置侧评估；本对话框不代改。

## 需要审核组拍板的点

1. **B/D 的归属**：是否由设置侧另开整改，还是要求本分支一并完成？若要求本分支完成，请确认可以改动 `settings.rs` 并接受上面的 provenance 方案。
2. **E「WARN 一次」的粒度**：当前是「每次 load 失败都 WARN」（由 settings.rs 负责）。是否要改成每进程只 WARN 一次？
3. **duplicate 默认行为**：`telegram.notify` 缺省（null）时所有类别默认开启，包括 `duplicate`。若希望重复默认关闭，请明确，我改成缺省白名单。
4. **通知项校验**：`telegram.notify` 目前允许任意 JSON（notify 读取时忽略未知项/类型）。是否需要在 PUT 时校验并拒绝未知类别？这会影响设置 API 的契约，由审核组决定。
5. **端到端验证**：是否允许使用 `archive-settings.json` 里的 bot 凭据向用户 TG 发一条测试消息？测试库当前没有该设置文件，需要先挂载/配置。

## 备注

- 模块内文档已同步：`docs/handoff/module-notify.md`（文件优先、duplicate、UTF-16、打码、整改记录）。
- `PROGRESS.md` 已把通知行标记为「验收整改（feat/notify-settings）」。
- 本分支未改动设置模块、未动 telethon 相关工作区。
