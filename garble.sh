#!/bin/bash
R="/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/失败目录"
LATEST=$(ls -1 "$R" | tail -1)
echo "latest=$LATEST"
ls -la "$R/$LATEST" 2>/dev/null | head -6
F=$(find "$R/$LATEST" -maxdepth 1 -type f | head -1)
echo "file=$F"
echo "--- first bytes ---"
head -c 200 "$F" | xxd | head -6
echo "--- as utf8 ---"
head -c 200 "$F"