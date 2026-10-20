#!/bin/bash
CHUNK=$(docker exec lux-testlib sh -c "grep -l 侧车在线 /usr/local/share/lux/web/assets/*.js" | head -1 | sed 's|.*/||')
echo "chunk=$CHUNK"
for s in lux-settings-saved lux-error-copy lux-login-background-warning lux-network-proxy-status lux-button-secondary; do
  printf "%-32s %s\n" "$s" "$(docker exec lux-testlib sh -c "grep -o '$s' /usr/local/share/lux/web/assets/$CHUNK | wc -l")"
done

J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
echo "--- probe ---"
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo

cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "style(settings): use the product's status palette for the sidecar probe

The probe verdict was rendered with its own hard-coded green/red chip, which did
not match the admin UI (and ignored the light theme, where the same states are
darkened). It now reuses the classes the rest of the settings page already uses:
lux-settings-saved for 连接正常, lux-error-copy for 失效/连不上,
lux-login-background-warning for 未配置, and lux-network-proxy-status for the
in-flight line. Only the row layout is local.

The button keeps lux-button-secondary, the same variant as 检测延迟与出口."
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"