#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "docs(handoff): channel monitoring contract, aligned with daquan's policy

从 daquanclaw（未编译的 Node 源码）实读出来的语义写成契约，供下一步实现对照：
频道清单、每频道 5 个可叠加模式（newRelease/follow/full/remux/manual）、
逐文件判定矩阵、配置结构与迁移、验收标准。full 的含义按 daquan 与用户确认：
允许入新+追更+洗版全部动作，而不是无条件全投。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"