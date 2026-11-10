#!/usr/bin/env bash
# Stage native Tauri sidecars from checksum-verified upstream release archives.
set -euo pipefail
cd "$(dirname "$0")/.."
triple=$(rustc -vV | awk '/^host:/ {print $2}')
case "$triple" in
  *-apple-darwin|*-unknown-linux-gnu) ext= ;;
  *-pc-windows-msvc) ext=.exe ;;
  *) echo "Unsupported desktop target: $triple" >&2; exit 1 ;;
esac
cargo build --locked -p v2rayr-cli
work=$(mktemp -d)
stage=$(mktemp -d apps/desktop/.embedded-cores.XXXXXX)
trap 'rm -rf -- "$work" "$stage"' EXIT
mkdir -p "$stage/xray-assets" "$stage/licenses"
download() {
  local kind=$1 attempt
  for attempt in 1 2 3; do
    if "./target/debug/v2rayr$ext" --home "$work/$kind" install "$kind"; then
      return 0
    fi
    if (( attempt < 3 )); then sleep "$((attempt * 2))"; fi
  done
  return 1
}
for kind in sing-box xray; do
  download "$kind" &
  if [[ $kind == sing-box ]]; then sing_pid=$!; else xray_pid=$!; fi
done
sing_status=0
xray_status=0
wait "$sing_pid" || sing_status=$?
wait "$xray_pid" || xray_status=$?
if (( sing_status != 0 || xray_status != 0 )); then
  echo "Unable to prepare both verified desktop cores" >&2
  exit 1
fi
for kind in sing-box xray; do
  install -m 0755 "$work/$kind/data/cores/$kind$ext" "$stage/v2rayr-core-$kind-$triple$ext"
  cp "$work/$kind/data/cores/$kind-LICENSE" "$stage/licenses/"
done
for file in geoip.dat geosite.dat; do
  cp "$work/xray/data/cores/$file" "$stage/xray-assets/"
done
if [[ "$triple" == *-pc-windows-msvc ]]; then
  cp "$work/sing-box/data/cores/wintun.dll" "$stage/wintun.dll"
fi
rm -rf -- apps/desktop/embedded-cores
mv "$stage" apps/desktop/embedded-cores
