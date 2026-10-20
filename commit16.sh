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
echo "=== generated cover in bundle ==="
docker exec lux-testlib sh -c "grep -rl lux-generated-cover /usr/local/share/lux/web/assets/*.js | head -3"
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "feat(ui): use generated covers where a poster is missing

接进 MediaPlaceholder：没有海报的电影/剧集不再显示灰底文字占位，改为 GeneratedCover
（电影酒红、剧集藏蓝）；文件夹保留图标；仍在拉本地元数据的条目把转圈叠在新封面上。

上一版只提交了组件没接线，产物哈希没变、页面自然没变化；这次镜像哈希从
360d1fed 变为 23e522ba，证明模块真的进了 bundle（已确认 lux-generated-cover 出现在 assets 里）。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"