#!/bin/bash
echo "--- built bundle contains the new UI strings ---"
docker exec lux-testlib sh -c "grep -rl 测试连接 /usr/local/share/lux/web/assets | head -3" 2>/dev/null || echo "grep unavailable in image"

J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)

echo "--- unreachable branch (dead port) ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":"http://172.17.0.1:8096"}}' -o /dev/null
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo

echo "--- auth_required branch (dead-cookie sidecar on 8097) ---"
docker rm -f lux-transfer115-badc >/dev/null 2>&1 || true
docker run -d --name lux-transfer115-badc -p 8097:8098 -e LUX_115_COOKIE="UID=1; CID=2; SEID=3; KID=4" \
  -v /vol5/1000/build/archive-test-config:/config:ro lux-transfer115:latest >/dev/null
sleep 3
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":"http://172.17.0.1:8097"}}' -o /dev/null
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo

echo "--- restore good sidecar ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":"http://172.17.0.1:8098"}}' -o /dev/null
docker rm -f lux-transfer115-badc >/dev/null 2>&1 || true
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo