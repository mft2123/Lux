#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "feat(telegram): list the account's channels and stop refusing an empty channel list

频道监听 v2 的前置：设置页要先把账号里的频道列出来，用户才有得勾。两处改动：

- runner.run 在 listen_channels 为空时不再 exit 2，改为「空转 + WARNING」，
  /healthz 与 /channels 照常可用；勾选频道并重启后开始投递。
- 新增 runner.list_channels()（连接→get_dialogs→只留 is_channel，60s 超时，
  失败断开重连），并由侧车的 GET /channels 暴露。

实测（用 chill 的 session 复制体，未动原件）：/channels 返回账号真实频道
（爱影热更频道、爱影VIP群组、多多资源库、115网盘热更频道 等），空频道启动也只
打 WARNING 不再退出。单测尚未补（下一步与频道勾选一起写）。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"