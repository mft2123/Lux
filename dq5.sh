#!/bin/bash
echo "=== mode keys ==="
docker exec daquanclaw sh -c "grep -rhoE 'modes\.[a-zA-Z]+' /app/*.js | sort -u"
echo "=== data dir ==="
ls -la /vol5/1000/docker/daquanclaw/data 2>/dev/null | head -15
echo "=== channel list source (getDialogs equivalent) ==="
docker exec daquanclaw sh -c "grep -rhoE 'getDialogs|iterDialogs|getChats|folders|GetDialogs' /app/*.js | sort | uniq -c | head"