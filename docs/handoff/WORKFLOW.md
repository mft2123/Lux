# 协作流程（文件总线）

所有模块共用一份**评审总线**：`docs/handoff/REVIEW-BUS.md`。这是唯一的状态来源，任何一方改动它都要提交 git，
另一方在**每轮开工前**先看 `git log -1 --stat -- docs/handoff/REVIEW-BUS.md` 和文件内容；有变动就按最新指示处理。

## 角色

- **总控（审核方）**：管总线、审核、合并、重建镜像、更新 PROGRESS。不直接改模块代码。
- **模块方（每个对话框一个）**：只改自己模块的文件，按总线状态流转。

## 分支与隔离（强制）

1. 每个模块一个分支 + 一个独立 worktree：
   `git worktree add /vol5/1000/build/lux-<模块> -b feat/<模块> fork/phase-a`
2. 各自用独立 target 目录：`/vol5/1000/build/target-<模块>`（从共享 target 复制种子）。
3. **一个目录只能有一个写入者**；发现冲突立即停止并写总线「冲突」条目，不要覆盖对方。
4. 模块方**不得**构建/覆盖共享镜像标签 `lux:phase-a`；镜像由总控统一构建。

## 模块方每轮必须做的

1. 开工前：读 `REVIEW-BUS.md` 自己那节，确认状态是 `READY` 被退回（`CHANGES`）还是已 `PASS`。
2. 只做总线里列出的条目（编号 F1、F2……），每条一个提交，提交信息带编号：`fix(sidecar): F3 scrub credentials`。
3. 自测：模块自带测试 + 总控要求的测试命令，结果写进总线。
4. 完成后更新总线自己那节：
   - `状态: READY`
   - `分支 / 提交`：分支名 + 本次所有提交哈希
   - `逐条:` 每个编号后写 `✅ <提交哈希> 验收证据`
   - `测试:` 命令与结果
   - `遗留:` 未解决项（没有就写"无"）
5. 提交总线文件：`git commit -m "bus(<模块>): ready for review" -- docs/handoff/REVIEW-BUS.md`

## 审核方（总控）每轮必须做的

1. 看总线里 `状态: READY` 的节点，逐条核对（跑测试、必要时实测）。
2. 在总线里写结论：`状态: PASS` 或 `状态: CHANGES`，并追加 `新意见:` 列表（编号、严重级别、file:line、验收标准）。
3. `CHANGES` 时把模块分支**退回**（不改代码），等模块方修完再 `READY`。
4. `PASS` 后由总控合并到 `fork/phase-a`（只 fast-forward 或合并无冲突时），更新 `PROGRESS.md`，并在总线写 `MERGED <合并提交>`。

## 状态机

```
DRAFT → READY → PASS → MERGED
            ↑      │
            └ CHANGES ←（审核方写意见）
```
- 任何一方改动文件后必须提交，提交信息以 `bus(<模块>):` 开头，便于对方 `git log` 发现。
- 冲突/阻塞写 `状态: BLOCKED` + 原因；解除后回到 `READY`。

## 审核意见格式（统一）

```
- F7 [严重/一般/小项] <一句话问题> — <file:line>
  验收: <可执行的检查>
```

## 合并门禁

- 模块测试全绿（`cargo test --locked --lib archive` / 模块自带 Python 测试）
- 前端改动需 `pnpm build` 通过
- 无真实凭据出现在仓库（每次合并前 grep）
- 总控构建镜像并实测一次关键路径
