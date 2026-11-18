#!/usr/bin/env bash
# 部署脚本：目标信息全部通过环境变量传入，仓库中不硬编码任何具体网络信息。
set -euo pipefail
: "${KOAKUMA_HOST:?请设置 KOAKUMA_HOST}"
: "${KOAKUMA_USER:?请设置 KOAKUMA_USER}"
REMOTE_PATH="${KOAKUMA_REMOTE_PATH:-/data/other_vol/koakuma/koakuma}"
TARGET="aarch64-unknown-linux-musl"
BIN="target/$TARGET/release/koakuma"
[ -f "$BIN" ] || { echo "未找到二进制，请先执行 build.sh"; exit 1; }
echo "上传 $BIN 到 ${KOAKUMA_USER}@${KOAKUMA_HOST}:$REMOTE_PATH"
# 实际传输方式按环境而定，此处仅示意
echo "完成（示例脚本，具体传输命令按部署环境补充）"
