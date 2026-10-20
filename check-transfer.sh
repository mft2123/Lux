#!/bin/bash
echo "=== sidecar log ==="
docker logs lux-transfer115 2>&1 | grep "transfer job" | tail -4
echo "=== 状态.jsonl (last) ==="
tail -1 "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/链接队列/状态.jsonl"
echo
echo "=== 转存目录 ==="
ls -la "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/转存目录/" | head -12
echo "=== 待整理 ==="
ls -la "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/待整理/" | head -12