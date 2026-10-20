#!/bin/bash
echo "=== mode keys used in decisions ==="
docker exec daquanclaw sh -c "grep -rhoE 'modes\.[a-zA-Z]+' /app --include=*.js | sort -u"
echo "=== channel config files ==="
ls -la /vol5/1000/docker/daquanclaw/config 2>/dev/null | head -20
echo "=== config keys mentioning channel/mode ==="
grep -rhoE '"(channels|modes|newRelease|remux|follow|full|manual)"' /vol5/1000/docker/daquanclaw/config/*.json 2>/dev/null | sort | uniq -c | head -10