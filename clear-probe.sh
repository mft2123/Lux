#!/bin/bash
J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"tmdb_api_key":""}' -o /dev/null -w "clear -> %{http_code}\n"
docker run --rm -v /vol5/1000/build/archive-test-config:/config:ro -v /tmp/lux-u11/inspect-settings.py:/i.py --entrypoint python lux-transfer115:latest /i.py