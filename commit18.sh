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
docker exec lux-testlib sh -c "grep -rl 'onError' /usr/local/share/lux/web/assets/*.js | head -2"
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "fix(ui): fall back to a generated cover when the poster request fails

现象：同一页里剧集有自动封面、电影没有。原因是电影条目带 imageTags（于是
imageUrl() 返回非空、走 <img>），但图片接口 404，失败后既不出图也不回退占位——
卡片上就是空白。现在 <img> 加 onError，失败即回退 GeneratedCover；剧集本来
就没有 tags，路径不变。

镜像 0c20b652（较上一版 23e522ba 有变化，说明改动进了 bundle）。"
TIP=$(git rev-parse HEAD)
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "tip: $TIP"