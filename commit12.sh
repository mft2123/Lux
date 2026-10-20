#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "fix(telegram): an empty channel list is a normal state, not an unhealthy one

频道监听 v2 要先列出账号频道再勾选，所以「没勾任何频道」是合法状态。把
runtime_problems 里的这一项去掉（保留 runtime_warnings 的提示），否则
/healthz 会在空配置下返回 503，Lux 的「测试连接」会误报。

补 tests/test_channels.py 两条：只返回频道（过滤普通群、过滤无 entity 的）、
字段稳定且断开连接；session 缺失时给明确错误。

实测（chill session 复制体）：/healthz → configured；?deep=1 → logged_in；
/channels → 24 个频道。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"