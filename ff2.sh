#!/bin/bash
set -e
cd /tmp/lux-u11
cat >> docs/handoff/PROGRESS.md <<'EOF'

## 设置页分区保存 + 115 测试连接（2026-10-20）

- 归档设置页从「一个全局保存」改为**每个功能区分区保存**：TMDB / 目录与路径 / 洗版参数 / Telegram 通知 /
  115 转存 / Telegram 频道监听，各自只提交自己的键，别处未保存的编辑不会被连带提交或回滚。
- 115 转存区新增**测试连接**：`POST /api/v1/admin/archive/transfer115/probe` 由服务端调用
  `{sidecar}/healthz?deep=1`，返回 `logged_in` / `auth_required` / `unconfigured` / `unreachable`。
  浏览器到不了 docker 网络里的侧车，所以必须服务端代测。
- 表单只在首次加载时填充；之后的重取不会覆盖未保存的编辑，保存成功只回填本分区的字段。
- 实测：四条分支全部验过（logged_in / auth_required / unconfigured / unreachable），无 CSRF 的调用 403；
  分区 PUT 不会动其它分区（telegram-only 保存后 cookie 与 sidecar_base 不变）。镜像 `lux:phase-a` = `60edbd9084d9`。
EOF
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "docs(handoff): record the per-section settings saves and the sidecar probe"
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a HEAD
git -C "$R" branch -f integration/round2 HEAD
git -C "$R" log --oneline -3 fork/phase-a