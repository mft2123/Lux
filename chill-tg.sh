#!/bin/bash
echo "=== lux settings template: telegram keys ==="
python3 - <<'PY'
import json
p="/vol5/1000/build/archive-settings.json"
try:
    d=json.load(open(p,encoding="utf-8"))
    t=d.get("telegram",{})
    for k,v in t.items():
        if isinstance(v,str):
            print(f"  {k}: len={len(v)} {'(masked)' if '…' in v else ''}")
        else:
            print(f"  {k}: {type(v).__name__}={v if not isinstance(v,(list,dict)) else '...'}")
except Exception as e:
    print("ERR", e)
PY
echo "=== session files in chill ==="
docker exec cinechill sh -c "find / -maxdepth 5 -name '*.session*' 2>/dev/null" | head -10
echo "=== chill config tg keys (names only) ==="
python3 - <<'PY'
import json,glob
for p in glob.glob("/vol5/1000/docker/chillposter/config/*.json"):
    try:
        d=json.load(open(p,encoding="utf-8"))
    except Exception:
        continue
    if isinstance(d,dict) and any(k in d for k in ("telegram","tg","api_id","bot_token")):
        print(" ", p.split("/")[-1], "->", [k for k in d if k in ("telegram","tg","api_id","api_hash","phone","bot_token","session")])
PY