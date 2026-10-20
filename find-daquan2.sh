#!/bin/bash
echo "=== containers (all) ==="
docker ps -a --format "{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}" | head -40
echo "=== images with daquan ==="
docker images --format "{{.Repository}}:{{.Tag}}" | grep -i daquan