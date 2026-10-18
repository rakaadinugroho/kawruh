#!/usr/bin/env bash
# Sets the version in every manifest, commits and tags the release. Pushing the
# tag runs .github/workflows/release.yml.
#
#   scripts/release.sh 0.2.0           bump, commit "chore: release v0.2.0", tag v0.2.0
#   scripts/release.sh --check v0.2.0  fail unless every manifest says 0.2.0 (CI uses this)
set -euo pipefail
cd "$(dirname "$0")/.."

JSON=(app/package.json app/src-tauri/tauri.conf.json)

# Top-level "version" of a JSON manifest (2-space indent, as these files are).
json_version() { sed -n 's/^  "version": "\(.*\)",$/\1/p' "$1"; }
cargo_version() { sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml; }

check() {
  local want="$1" bad=0 f v
  for f in Cargo.toml "${JSON[@]}"; do
    if [[ "$f" == Cargo.toml ]]; then v="$(cargo_version)"; else v="$(json_version "$f")"; fi
    if [[ "$v" != "$want" ]]; then
      echo "$f has version '$v', expected '$want'" >&2
      bad=1
    fi
  done
  return "$bad"
}

if [[ "${1:-}" == "--check" ]]; then
  want="${2:?usage: release.sh --check vX.Y.Z}"
  check "${want#v}"
  echo "all manifests at ${want#v}"
  exit 0
fi

VERSION="${1:?usage: release.sh X.Y.Z}"
VERSION="${VERSION#v}"
TAG="v$VERSION"
# Windows installers need numeric versions; a pre-release suffix is fine (0.2.0-beta.1).
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || { echo "not a version: $VERSION" >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo "commit or stash your changes first" >&2; exit 1; }
! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || { echo "tag $TAG already exists" >&2; exit 1; }

sed -i.bak "/^\[workspace\.package\]/,/^\[/s/^version = \".*\"$/version = \"$VERSION\"/" Cargo.toml
for f in "${JSON[@]}"; do
  sed -i.bak "s/^  \"version\": \".*\",$/  \"version\": \"$VERSION\",/" "$f"
done
rm -f Cargo.toml.bak "${JSON[@]/%/.bak}"
check "$VERSION"
cargo metadata --quiet --format-version 1 >/dev/null # refreshes the workspace entries in Cargo.lock

git add Cargo.toml Cargo.lock "${JSON[@]}"
git commit -q -m "chore: release $TAG"
git tag -a "$TAG" -m "Kawruh $TAG"
echo "tagged $TAG. Publish it with:"
echo "  git push --atomic origin $(git branch --show-current) $TAG"
