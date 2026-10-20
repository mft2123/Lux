#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "fix(ui): make dropdown items readable in both themes

深色主题下很多「选择项」文字看不见，两个原因叠在一起：

- 自定义下拉 .lux-select-option 用的是 --lux-muted（64% 通透的白），在
  rgba(21,21,25,.98) 的菜单底上偏灰；改成 --lux-text。
- 原生 <select>（监听模式、登录页背景来源等）的弹层由系统绘制，不继承页面
  背景，而 option 没有显式颜色 → 深色下白字白底、浅色下也可能对比不足。
  现在按主题显式给 option/optgroup 定色（dark: #f5f5f5 on #151519，
  light: #18181b on #fff）。

镜像构建含 tsc/vite 通过；产物 index-kO3gmWjz.css 已确认包含两条规则。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"