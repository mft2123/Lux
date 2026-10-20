#!/bin/bash
echo "=== telegram-related files ==="
docker exec daquanclaw sh -c "ls -1 /app | grep -iE 'telegram|tg|channel|monitor'" 2>&1 | head -20
echo "=== files mentioning getDialogs/iterDialogs ==="
docker exec daquanclaw sh -c "grep -rl 'getDialogs\|iterDialogs\|getChats' /app --include=*.js 2>/dev/null" | head -10
echo "=== files mentioning 入新/追更/全量 ==="
docker exec daquanclaw sh -c "grep -rl '入新\|追更\|全量' /app --include=*.js 2>/dev/null" | head -10