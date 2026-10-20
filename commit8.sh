#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "fix(ui): make the light theme readable in the admin and archive settings pages

切到浅色后看不清，原因是几处写死的颜色：后台标题 h2 是 #fff、图标按钮是 #fff、
输入框/文本域是深色透明底 + 白字，状态框也是白色透明底。这些在 #f4f3f1 的浅色
背景上要么消失要么发灰。现在统一回退到 --lux-text / --lux-line-soft /
rgba(28,28,34,.04) 这一套浅色变量，只影响 html[data-lux-theme=\"light\"]。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"