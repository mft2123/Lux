#!/bin/bash
J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)

echo "--- probe (good sidecar) ---"
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe \
  -H "X-CSRF-Token: $csrf" -w "\nHTTP %{http_code}\n"

echo "--- probe without CSRF (must be rejected) ---"
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -o /dev/null -w "HTTP %{http_code}\n"

echo "--- per-section PUT: telegram only, must not touch transfer115 ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"telegram":{"enabled":true}}' -o /dev/null -w "PUT %{http_code}\n"
docker run --rm -v /vol5/1000/build/archive-test-config:/config:ro -v /tmp/lux-u11/inspect-settings.py:/i.py --entrypoint python lux-transfer115:latest /i.py