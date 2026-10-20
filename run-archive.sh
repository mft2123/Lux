#!/bin/bash
J=/tmp/c.jar
rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
echo "--- run ---"
curl -s -m 200 -b $J -X POST "http://127.0.0.1:8099/api/v1/admin/archive/run?dry_run=false" \
  -H "X-CSRF-Token: $csrf" -w "\nHTTP %{http_code}\n"
echo "--- state tail ---"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/status \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps({k:d[k] for k in d if k in ('pending_link_jobs','running')}, ensure_ascii=False))"