#!/usr/bin/env bash
# Build the desktop app for this machine and copy its installers to dist/.
#
# CI runs this on macOS, Windows, and Linux after a merge to main. A pull
# request only compiles the app: scripts/package-desktop.sh --no-bundle
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

bundle=1
extra=()
while [[ $# -gt 0 ]]; do
  case $1 in
    --no-bundle)
      bundle=0
      shift
      ;;
    *)
      extra+=("$1")
      shift
      ;;
  esac
done

if ! command -v pnpm >/dev/null 2>&1; then
  echo "pnpm is required to package the desktop app" >&2
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to package the desktop app" >&2
  exit 1
fi

pnpm --dir ui install --frozen-lockfile
pnpm --dir ui build
bash scripts/stage-desktop-cores.sh

tauri="$root/ui/node_modules/.bin/tauri"
if [[ ! -e "$tauri" ]]; then
  echo "missing $tauri; ui dependencies did not install the Tauri CLI" >&2
  exit 1
fi

# macOS system Bash treats an empty array as unbound under set -u.
# The bundle version is the root workspace version, not tauri.conf.json.
version=$(bash "$root/scripts/app-version.sh")
args=(build --config "{\"version\":\"$version\"}")
if (( bundle == 0 )); then
  args+=(--no-bundle)
fi
if (( ${#extra[@]} > 0 )); then
  args+=("${extra[@]}")
fi
(
  cd apps/desktop
  "$tauri" "${args[@]}"
)

if (( bundle == 0 )); then
  exit 0
fi

shopt -s nullglob
installers=(
  apps/desktop/target/release/bundle/dmg/*.dmg
  apps/desktop/target/release/bundle/msi/*.msi
  apps/desktop/target/release/bundle/nsis/*.exe
  apps/desktop/target/release/bundle/deb/*.deb
  apps/desktop/target/release/bundle/rpm/*.rpm
  apps/desktop/target/release/bundle/appimage/*.AppImage
)
shopt -u nullglob
if (( ${#installers[@]} == 0 )); then
  echo "tauri build produced no installers" >&2
  exit 1
fi

mkdir -p dist
seen_ext=" "
for installer in "${installers[@]}"; do
  ext=${installer##*.}
  case $seen_ext in
    *" $ext "*) ;;
    *)
      for stale in dist/*."$ext"; do
        [[ -e $stale ]] || continue
        rm -f "$stale"
      done
      seen_ext="$seen_ext$ext "
      ;;
  esac
  cp -f "$installer" "dist/$(basename "$installer")"
done

list=()
for candidate in dist/*; do
  [[ -f $candidate ]] || continue
  case $candidate in
    *.dmg | *.msi | *.exe | *.deb | *.rpm | *.AppImage)
      list+=("$(basename "$candidate")")
      ;;
  esac
done
(
  cd dist
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${list[@]}"
  else
    shasum -a 256 "${list[@]}"
  fi
) > dist/SHA256SUMS
echo "installers written to dist/"
printf '  %s\n' "${list[@]}"
