#!/bin/bash
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
echo "=== light overrides shipped ==="
docker exec lux-testlib sh -c "grep -o 'data-lux-theme=light].lux-admin-panel-heading h2' /usr/local/share/lux/web/assets/*.css" | head -2
docker exec lux-testlib sh -c "grep -o 'data-lux-theme=light].lux-icon-button' /usr/local/share/lux/web/assets/*.css" | head -2