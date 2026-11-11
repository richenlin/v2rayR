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
  case "$triple" in
    x86_64-*) wintun_arch=amd64 ;;
    aarch64-*) wintun_arch=arm64 ;;
    i686-*) wintun_arch=x86 ;;
    *) echo "Unsupported Wintun architecture: $triple" >&2; exit 1 ;;
  esac
  wintun_zip="$work/wintun.zip"
  curl --fail --location --retry 3 --output "$wintun_zip" https://www.wintun.net/builds/wintun-0.14.1.zip
  printf '%s  %s\n' 07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51 "$wintun_zip" | sha256sum -c -
  mkdir -p "$stage/windows-tun"
  unzip -p "$wintun_zip" "wintun/bin/$wintun_arch/wintun.dll" > "$stage/windows-tun/wintun.dll"
  unzip -p "$wintun_zip" wintun/LICENSE.txt > "$stage/windows-tun/Wintun-LICENSE.txt"
  cargo build --locked --release -p v2rayr-helper
  install -m 0755 target/release/v2rayr-helper.exe "$stage/windows-tun/v2rayr-helper.exe"
  install -m 0755 "$work/sing-box/data/cores/sing-box.exe" "$stage/windows-tun/sing-box.exe"
  helper_version=$(./target/release/v2rayr-helper.exe --version | tr -d '\r')
  (
    cd "$stage/windows-tun"
    printf '%s\n' "$helper_version"
    sha256sum --binary v2rayr-helper.exe sing-box.exe wintun.dll
  ) > "$stage/windows-tun/bundle-v2.sha256"
fi
rm -rf -- apps/desktop/embedded-cores
mv "$stage" apps/desktop/embedded-cores
