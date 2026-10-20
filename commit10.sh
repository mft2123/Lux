#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "docs(handoff): channel record shape from daquan, and manual is derived

读 daquan 的 telegram-account.js 后修正三处会写歪的细节：
- manual 不是独立开关，而是其余四个模式全为 false 的派生值；
- 频道 id 形如 telegram:<去掉 -100 的 id>，匹配支持 id/telegramId/username；
- 每频道靠 lastMessageId 游标增量，不是时间戳，重跑不重复处理。
另记：getDialogs 必须包 60s 超时并在失败时断开重连。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"