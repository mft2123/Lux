#!/bin/bash
J=/tmp/c.jar
rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
echo "--- transfer115 (masked) ---"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/settings \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d.get('transfer115'), ensure_ascii=False, indent=2))"
echo "--- archive status ---"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/status \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps({k:d[k] for k in d if k in ('pending_link_jobs','link_queue_dir','link_state_file','transfer115_base','running')}, ensure_ascii=False, indent=2))"
echo "--- containers ---"
docker ps --filter name=lux- --format "{{.Names}} {{.Status}}"