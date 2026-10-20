#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "feat(settings): surface the theme, make accent colours land, and probe the Telegram sidecar

- 顶栏加日/月切换：主题原先只在「账户 → 外观」里，得先找到那一页才能改。现在每一页
  右上角都能一键切换，写的是同一份设备偏好（账户页的入口保留）。
- 主按钮跟随强调色：强调色以前只影响勾选框/进度等 111 处细节，主按钮写死 #f2f2f3，
  所以点「莓果/海蓝/琥珀/薄荷」几乎看不出变化。现在 .lux-button-primary 用
  var(--lux-accent)/--lux-accent-foreground；silver（默认）保留原来的浅色按钮。
- TG 频道监听加「测试连接」：旁挂是纯常驻进程、原先没有任何可探测接口，所以给侧车加了
  tgwatch/health.py（GET /healthz，?deep=1 复用 runner.check_session 真验登录态），
  run 子命令用 --health-port / LUX_TG_HEALTH_PORT 开启；Lux 侧新增
  POST /api/v1/admin/archive/telegram/probe 代测（地址取 LUX_TG_SIDECAR_BASE，
  默认 http://telegram:8091），页面按钮与提示跟 115 转存一致。

测试：侧车 tests/test_health.py 5 条通过；cargo test --lib archive 83 通过；镜像构建含 tsc/vite 通过。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"
git -C "$R" log --oneline -3 fork/phase-a