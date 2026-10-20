#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "docs(handoff): auto library provisioning contract

媒体目录下有电影/电视剧/动画，但 Lux 只注册了「电影」一个库，web 因此只显示电影。
按用户确认的三条决定写契约：动画独立成库、晚会/演唱会也自动建库、建库后自动全量扫描；
触发点为「归档 run 结束」+「启动对账」，幂等键是库根路径，空目录不建库。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"