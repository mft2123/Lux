#!/bin/bash
J=/tmp/c.jar
rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
echo "--- transfer115 before ---"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/settings \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d.get('transfer115'), ensure_ascii=False, indent=2))"
echo "--- PUT non-secret fields ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"target_dir":"转存目录","sidecar_base":"http://172.17.0.1:8098","timeout_seconds":120}}' \
  -w "\nHTTP %{http_code}\n" | tail -20
echo "--- transfer115 after ---"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/settings \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d.get('transfer115'), ensure_ascii=False, indent=2))"