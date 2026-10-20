from pathlib import Path

h = Path("/tmp/lux-u11/docs/handoff/HANDOFF.md")
s = h.read_text(encoding="utf-8")

subs = [
 (
  "### 8.2 ★ 真实 cookie 的端到端（最高优先，唯一没验的关键路径）",
  "### 8.2 ✅ 真实 cookie 的端到端（2026-10-20 已通过，见 `U11-ACCEPTANCE-2026-10-20.md`）",
 ),
 (
  "| 测试实例 | 设置读写、侧车被调用 + `failed` 记账、侧车不可达不丢作业，三条路径都验过 |",
  "| 测试实例 | 设置读写、侧车被调用 + `failed` 记账、侧车不可达不丢作业，三条路径都验过 |\n"
  "| 真实 115 cookie | ✅ 转存成功、文件在 CD2 可见、坏 cookie → `auth_required` 且作业留队（`U11-ACCEPTANCE-2026-10-20.md`） |",
 ),
 (
  "11. **NAS 没有外网**：`pip install` / `docker pull` / PyPI 都不通；`docker build` 能成功是因为\n"
  "    `Dockerfile.custom` 里写死了 npmmirror 的 npm 源、cargo 走本地缓存。所以侧车镜像只能在上网环境构建。",
  "11. **NAS 不是「没有外网」，是「到某些域名不通」**（2026-10-20 实测）：`115.com` / `webapi.115.com` /\n"
  "    `115cdn.com` 与清华、阿里 pip 镜像都是 200；只有 `pypi.org` 和 `registry-1.docker.io` 直连不通。\n"
  "    所以侧车镜像**可以在这台 NAS 上构建**：基础镜像走 `docker.m.daocloud.io/library/python:3.12-slim`，\n"
  "    pip 走 `PIP_INDEX_URL`（默认清华源）。不要再按「必须去外网构建」处理。",
 ),
]

for old, new in subs:
    assert s.count(old) == 1, f"anchor count={s.count(old)}: {old[:40]!r}"
    s = s.replace(old, new)
h.write_text(s, encoding="utf-8")

p = Path("/tmp/lux-u11/docs/handoff/PROGRESS.md")
t = p.read_text(encoding="utf-8")
t += (
 "\n## U11 验收（2026-10-20）\n\n"
 "- 真实 115 cookie 端到端通过：转存 → `转存目录` → CD2 可见 → `待整理`；坏 cookie → `auth_required` 且作业留队。\n"
 "- 修掉 3 个挡路 bug（`115cdn` 主机、密码尾部说明、auth 判定），侧车依赖钉 `python-asynctools==0.1.4`。\n"
 "- 证据：`docs/handoff/U11-ACCEPTANCE-2026-10-20.md`；镜像 `lux:phase-a` = `5aaa2f1ef7fa`。\n"
)
p.write_text(t, encoding="utf-8")
print("patched HANDOFF + PROGRESS")