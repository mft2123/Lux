#!/bin/bash
set -e
J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
echo "--- endpoint still works ---"
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo

cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "fix(settings): make the sidecar test button visible and its verdict unmissable

The 测试连接 button shipped with the bare .lux-button class, which is
transparent with #111 text — on the dark settings page it was a black smudge
nobody could see. It now uses lux-button-secondary (border + translucent fill +
white text), the same treatment as the network-proxy test button.

The verdict was a line of .74rem grey-ish text under the whole form, and only
after a successful call; a failure was even quieter. It is now a coloured chip on
the same row as the buttons: green 连接正常 with a check, red with an alert icon
for 失效/未配置/连不上, plus an explicit 正在连接侧车并校验 115 登录… state while
the probe runs. Editing the address or the cookie clears a stale verdict."
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"