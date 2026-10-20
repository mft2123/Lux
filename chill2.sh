#!/bin/bash
C=/vol5/1000/docker/chillposter/config
for f in forward_aiying.json ai_episode_resolver.json drive115_upload_tasks.json 115_life_monitor_state.json; do
  echo "=== $f ==="; cat "$C/$f" 2>/dev/null | head -30; echo
done
echo "=== sqlite tables ==="
for db in 115_life_actions.db media_library_cache.db; do
  echo "--- $db ---"
  python3 - "$C/$db" <<'PY'
import sqlite3,sys
try:
    con=sqlite3.connect(f"file:{sys.argv[1]}?mode=ro",uri=True)
    for (n,) in con.execute("select name from sqlite_master where type='table' order by name"):
        print(" ", n)
except Exception as e:
    print("  ERR", e)
PY
done