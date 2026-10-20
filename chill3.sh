#!/bin/bash
C=/vol5/1000/docker/chillposter/config
echo "=== aiying / strm / 补齐 in logs ==="
grep -aoiE "(aiying|爱影)[^ ]{0,40}" "$C/app.log" 2>/dev/null | sort | uniq -c | sort -rn | head -8
grep -aoiE "strm[^ ]{0,30}" "$C/app.log" 2>/dev/null | sort | uniq -c | sort -rn | head -8
grep -aoiE "(缺集|补齐|缺失剧集)[^ ]{0,30}" "$C/app.log" 2>/dev/null | sort | uniq -c | sort -rn | head -8
echo "=== monitor115 strings: urls/endpoints ==="
docker exec cinechill sh -c "strings /app/core/monitor115/*.so 2>/dev/null" | grep -aoE "https?://[a-zA-Z0-9./_-]+" | sort -u | head -15
echo "=== monitor115 files ==="
docker exec cinechill sh -c "ls -la /app/core/monitor115"