#!/bin/bash
T="/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/转存目录"
echo "--- t0 ---"
ls -la "$T" | head -6
sleep 20
echo "--- t+20s ---"
ls -la "$T" | head -6
echo "--- 媒体目录 Whistle? ---"
find "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/媒体目录" -maxdepth 3 -iname "*Whistle*" 2>/dev/null | head -5