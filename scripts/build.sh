#!/usr/bin/env bash
# 交叉编译脚本：输出 aarch64-musl 静态二进制
set -euo pipefail
TARGET="aarch64-unknown-linux-musl"
rustup target add "$TARGET" >/dev/null 2>&1 || true
cargo zigbuild --release --target "$TARGET"
echo "产物: target/$TARGET/release/koakuma"
