#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "feat(settings): give every server-settings region its own save

播放行为 bundled three unrelated concerns (继续观看阈值 / 待确认标记 / 媒体库排序)
behind one 保存设置 button, so changing the threshold also committed whatever was
half-edited about the library. Split it into 播放行为 and 媒体库显示, each with its
own button and its own PATCH: the server merges the fields it receives, so one
region never carries another region's pending edits.

The form is also filled once now: a refetch after one region saves no longer
resets the fields still being edited in another region."
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"
git -C "$R" log --oneline -3 fork/phase-a