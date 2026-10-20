#!/bin/bash
set -e
docker rm -f lux-testlib >/dev/null 2>&1 || true
docker run -d --name lux-testlib -p 8099:8097 \
  -e LUX_ARCHIVE_ROOT=/testlib -e LUX_ARCHIVE_MIRROR_ROOT=/library -e LUX_ARCHIVE_CD2_PREFIX=测试库 \
  -e LUX_ARCHIVE_CD2_MOUNT=/media/cloud -e LUX_ARCHIVE_WATCH_DIRS=/testlib/外部上传,/baidu \
  -e LUX_ARCHIVE_POLL_SECONDS=10 -e LUX_ARCHIVE_SETTLE_SECONDS=3 -e LUX_TMDB_LANGUAGE=zh-CN \
  -e LUX_HTTP_ADDR=0.0.0.0:8097 -e LUX_CONFIG_DIR=/config -e RUST_LOG=luxd=info \
  -v /vol5/1000/docker/lux-library:/library \
  -v fc7b30cdcd17ddb92b4473c2bcf840dfa306a20aeebac7f0874867d98ca0d4ea:/media \
  -v /tmp/lux-baidu-sim:/baidu \
  -v /vol5/1000/build/archive-test-config:/config \
  -v /vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库:/testlib \
  lux:phase-a >/dev/null
sleep 12
docker ps --filter name=lux-testlib --format "{{.Status}}"
docker exec lux-testlib sh -c "grep -o 'isolation:isolate' /usr/local/share/lux/web/assets/*.css | head -2"
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "fix(home): stop neighbouring rows bleeding into the horizontal library rail

overflow-x:auto 会把未声明的 overflow-y 一并算成 auto，首页「我的媒体库」这类
横向轨道因此成了纵向也可滚动的容器；页面纵向滚动时，相邻区块的文字会渗进这一行。
显式 overflow-y:hidden 裁掉纵向溢出，并给轨道加 isolation:isolate 独立层叠，
避免子元素的 z-index 与相邻区块互相穿透。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"