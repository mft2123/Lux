#!/bin/bash
docker run --rm \
  -e CARGO_HOME=/cargo-home -e RUSTUP_TOOLCHAIN=1.94.1 -e RUSTUP_DIST_SERVER=https://rsproxy.cn \
  -v /vol5/1000/build/cargo-home:/cargo-home \
  -v /tmp/lux-u11:/src \
  -v /vol5/1000/build/target:/src/target \
  -w /src rust:1.94-test \
  bash -c "cargo test --locked --lib archive 2>&1 | tail -40"