#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "feat(ui): generated covers for entries that have no artwork

媒体库里的电影/剧集/动画常常没有海报：目录里没有 poster.jpg，TMDB 也可能
返回空图。这里提供一个纯本地绘制的封面组件（GeneratedCover）：

- 按类型给主色：电影酒红 / 剧集藏蓝 / 动画紫；
- 片名首字（中文取前两字、英文取前两词首字母）+ 片名 + 年份 + 类型角标；
- 种子哈希让同一作品每次长得一样，不同作品之间又有细微差别；
- 不发网络请求，跟随主题，aspect-ratio 2:3 适配卡片。

本轮只落组件与样式，尚未接入卡片（下一步接首页/媒体库/详情三处）。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"