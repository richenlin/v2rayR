#!/usr/bin/env bash
# Publish the verified installers from one successful CI run.
set -euo pipefail
artifacts=$1
commit=$2
version=$3
run_url=$4
tag="v$version-ci.${commit:0:7}"
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
python3 - "$artifacts" "$work" <<'PY'
import hashlib
import pathlib
import shutil
import sys

source, target = map(pathlib.Path, sys.argv[1:])
records = []
for platform in ("macos-15", "windows-2022", "ubuntu-22.04"):
    folder = source / ("v2rayR-" + platform)
    manifest = folder / "SHA256SUMS"
    names = set()
    for line in manifest.read_text().splitlines():
        digest, name = line.split(maxsplit=1)
        name = name.removeprefix("*")
        if pathlib.Path(name).name != name or name in names:
            raise ValueError("Invalid checksum filename " + name)
        names.add(name)
        payload = folder / name
        if hashlib.sha256(payload.read_bytes()).hexdigest() != digest.lower():
            raise ValueError("Checksum mismatch " + name)
        destination = target / name
        if destination.exists():
            raise ValueError("Duplicate release asset " + name)
        shutil.copyfile(payload, destination)
        records.append(digest.lower() + "  " + name)
    if names != {p.name for p in folder.iterdir() if p.is_file() and p.name != "SHA256SUMS"}:
        raise ValueError("Unverified or missing installers for " + platform)
    if not names:
        raise ValueError("Missing installers for " + platform)
(target / "SHA256SUMS").write_text("\n".join(sorted(records)) + "\n")
PY
notes="$work/notes.md"
cat > "$notes" <<NOTES
Installers built from commit $commit.

- macOS ARM64 DMG
- Windows x64 MSI and NSIS setup
- Linux x64 DEB, RPM and AppImage

These CI packages are unsigned. Windows installation, integrity rejection and MSI repair checks passed. Native TUN traffic and crash recovery acceptance remains pending.

Build and test results: $run_url
Verify downloads against the attached SHA256SUMS.
NOTES
if draft=$(gh release view "$tag" --json isDraft --jq .isDraft 2>/dev/null); then
  if [[ "$draft" != true ]]; then
    gh release view "$tag" --json url --jq .url
    exit 0
  fi
else
  # Pin the tag independently. Omitting target_commitish when creating the
  # release also lets GITHUB_TOKEN publish an existing historical build.
  if tagged=$(gh api "repos/{owner}/{repo}/git/ref/tags/$tag" --jq .object.sha 2>/dev/null); then
    if [[ "$tagged" != "$commit" ]]; then
      echo "Release tag points to a different commit: $tag" >&2
      exit 1
    fi
  else
    gh api repos/{owner}/{repo}/git/refs -f "ref=refs/tags/$tag" -f "sha=$commit" > /dev/null
  fi
  gh release create "$tag" --draft --prerelease     --title "v$version CI (${commit:0:7})" --notes-file "$notes"
fi
assets=("$work"/*)
upload=()
for asset in "${assets[@]}"; do
  if [[ "$asset" != "$notes" ]]; then upload+=("$asset"); fi
done
gh release upload "$tag" "${upload[@]}" --clobber
gh release edit "$tag" --draft=false --prerelease --notes-file "$notes"
gh release view "$tag" --json url --jq .url
