#!/bin/bash
R="/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库"
echo "--- where is Whistle ---"
find "$R" -maxdepth 4 -iname "*Whistle*" 2>/dev/null
echo "--- 待整理 ---"
ls -la "$R/待整理" | head -8
echo "--- 失败目录 ---"
ls -la "$R/失败目录" | head -8
echo "--- lux log (tail) ---"
docker logs lux-testlib 2>&1 | tail -15