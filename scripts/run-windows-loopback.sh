#!/usr/bin/env bash
# 本机 Windows 实机用例：真实内核 + 本地隔离后端（不访问外网）。
#
#   bash scripts/run-windows-loopback.sh              # 全部 --ignored 用例（含系统代理接管）
#   bash scripts/run-windows-loopback.sh flow         # 只跑两个内核的 CONFIG/FLOW
#   bash scripts/run-windows-loopback.sh takeover     # 只跑系统代理接管/还原（会改写真实系统代理）
#
# 内核默认取安装版 sidecar；也可用 V2RAYR_TEST_SING_BOX / V2RAYR_TEST_XRAY 覆盖。
set -euo pipefail

cd "$(dirname "$0")/.."

core_dir="${LOCALAPPDATA:-$HOME/AppData/Local}/v2rayR"
export V2RAYR_TEST_SING_BOX="${V2RAYR_TEST_SING_BOX:-$core_dir/v2rayr-core-sing-box.exe}"
export V2RAYR_TEST_XRAY="${V2RAYR_TEST_XRAY:-$core_dir/v2rayr-core-xray.exe}"

for core in "$V2RAYR_TEST_SING_BOX" "$V2RAYR_TEST_XRAY"; do
    if [ ! -f "$core" ]; then
        echo "缺少内核文件: $core（先安装或设置 V2RAYR_TEST_* 环境变量）" >&2
        exit 2
    fi
done

case "${1:-all}" in
    flow) filter="windows_loopback_vless_flows_through_both_cores" ;;
    takeover) filter="windows_system_proxy_takeover" ;;
    all) filter="" ;;
    *)
        echo "未知用例: $1（可选 all|flow|takeover）" >&2
        exit 2
        ;;
esac

# `#[tokio::test]` 是单线程运行时，两个用例共用固定入站端口，必须串行。
cargo test -p v2rayr-app --locked --test windows_loopback_flow "$filter" \
    -- --ignored --nocapture --test-threads=1
