#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "docs(handoff): session handoff for the next conversation

记录本轮完成的 6 块、修掉的 5 个真 bug、4 项未完成（自动入库、TG 频道 v2 Lux 侧、
失败原因 BOM、UI 视觉未验收）以及 6 条新踩坑。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"