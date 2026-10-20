#!/bin/bash
J=/tmp/c.jar
LIB=01a14eca-30b5-7e72-adde-62a5e0fdb222
echo "=== library roots ==="
curl -s -b $J "http://127.0.0.1:8099/api/v1/admin/libraries/$LIB/roots" | head -c 600; echo
echo "=== 媒体目录 on 115 ==="
ls -1 "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/媒体目录" 2>/dev/null | head
echo "=== 本地镜像库 ==="
ls -1 /vol5/1000/docker/lux-library 2>/dev/null | head