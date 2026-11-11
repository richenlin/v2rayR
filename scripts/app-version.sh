#!/usr/bin/env bash
# Print the product version from the root workspace manifest.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
manifest=${1:-$root/Cargo.toml}
version=$(awk '
  $0 == "[workspace.package]" { section = 1; next }
  section && /^\[/ { exit }
  section && $1 == "version" {
    value = $3
    gsub(/"/, "", value)
    print value
    exit
  }
' "$manifest")
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "Cargo.toml 中没有可用的 workspace.package.version：${version:-<missing>}" >&2
  exit 1
fi
printf '%s\n' "$version"
