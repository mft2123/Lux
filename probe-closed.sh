#!/bin/bash
echo "--- unique new strings in bundle ---"
docker exec lux-testlib sh -c "grep -rl 侧车在线 /usr/local/share/lux/web/assets | head -3" || echo "not found"
docker exec lux-testlib sh -c "grep -rl 未配置侧车地址 /usr/local/share/lux/web/assets | head -3" || echo "not found"

J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
echo "--- truly closed port 8095 ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":"http://172.17.0.1:8095"}}' -o /dev/null
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo
echo "--- restore ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":"http://172.17.0.1:8098"}}' -o /dev/null
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo