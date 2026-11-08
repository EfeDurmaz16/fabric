#!/bin/sh
# Installs a tool the release workflow runs, at a pinned version, from its
# GitHub release, after checking the archive against a sha256 pinned here.
# The release builds and attests the archives users install, so its tools
# are not fetched by an installer script piped to a shell: a replaced
# release asset fails the checksum instead of running.
#
# Usage: scripts/install-release-tool.sh dist|cargo-cyclonedx
# Installs into $CARGO_HOME/bin (default ~/.cargo/bin) and, under GitHub
# Actions, puts that directory on PATH for later steps.
#
# To move to a new version, change the version and every sha256 below; take
# each sha256 from the release's own .sha256 asset or `digest` field and
# check it against a download. dist's version must equal cargo-dist-version
# in dist-workspace.toml.
set -eu

dist_version=0.34.0
cyclonedx_version=0.5.5

fail() {
  echo "error: $*" >&2
  exit 1
}

[ "$#" -eq 1 ] || fail "usage: $0 dist|cargo-cyclonedx"
tool=$1

case "$(uname -s)-$(uname -m)" in
Darwin-arm64) target=aarch64-apple-darwin ;;
Darwin-x86_64) target=x86_64-apple-darwin ;;
Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
*) fail "no pinned build for $(uname -s) $(uname -m)" ;;
esac

case $tool in
dist)
  toplevel=$(git rev-parse --show-toplevel)
  configured=$(sed -n 's/^cargo-dist-version = "\(.*\)"$/\1/p' "$toplevel/dist-workspace.toml")
  [ "$configured" = "$dist_version" ] ||
    fail "dist-workspace.toml wants dist $configured, this script pins $dist_version"
  name=cargo-dist
  bin=dist
  url=https://github.com/axodotdev/cargo-dist/releases/download/v$dist_version/$name-$target.tar.xz
  case $target in
  aarch64-apple-darwin) sha256=d1eec30e63eed23e5a5bd48fe96eae314799811bffca254ec1d2264638ba1890 ;;
  x86_64-apple-darwin) sha256=6aa89bb1e7c18f51ae8b148ce2fc6ab00ac46ed8173d4f2ed9afa12101218773 ;;
  x86_64-unknown-linux-gnu) sha256=7fee44ff3ff71ed0064b32d4ec62fee6b58bd37c5887b12a86aedddc5af0c7b5 ;;
  esac
  ;;
cargo-cyclonedx)
  name=cargo-cyclonedx
  bin=cargo-cyclonedx
  url=https://github.com/CycloneDX/cyclonedx-rust-cargo/releases/download/$name-$cyclonedx_version/$name-$target.tar.xz
  case $target in
  x86_64-unknown-linux-gnu) sha256=9136714bd6e1ac703f5c2c889af578d9783852886d6ebfe18d471555637ad2b3 ;;
  *) fail "cargo-cyclonedx is pinned only for x86_64-unknown-linux-gnu, where the release runs it" ;;
  esac
  ;;
*) fail "unknown tool $tool" ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
archive=$work/$name-$target.tar.xz
curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$archive" "$url"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$archive" | cut -d ' ' -f 1)
else
  actual=$(shasum -a 256 "$archive" | cut -d ' ' -f 1)
fi
[ "$actual" = "$sha256" ] || fail "$url has sha256 $actual, pinned $sha256"

tar -xJf "$archive" -C "$work"
dest=${CARGO_HOME:-$HOME/.cargo}/bin
mkdir -p "$dest"
install -m 0755 "$work/$name-$target/$bin" "$dest/$bin"
if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$dest" >>"$GITHUB_PATH"
fi
echo "installed $bin from $url (sha256 $sha256) into $dest" >&2
