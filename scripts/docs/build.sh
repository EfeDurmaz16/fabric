#!/bin/sh
# Builds the docs site with mdBook into target/docs-site.
#
# Usage: scripts/docs/build.sh
# mdBook is $MDBOOK when set, else `mdbook` on PATH; CI pins the version
# in .github/workflows/docs.yml, and this script says how to install that
# version when none is found. The site's title and repository link come
# from scripts/docs/names.env, so book.toml names neither. The rendered
# pages are then checked for an em dash, which no page may carry.
set -eu

toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"
. scripts/docs/names.env

# The version CI pins is the one to use; it is written once, in docs.yml.
pinned=$(sed -n 's/^ *MDBOOK_VERSION: *//p' .github/workflows/docs.yml)
mdbook=${MDBOOK:-mdbook}
if ! command -v "$mdbook" > /dev/null 2>&1; then
  echo "error: mdBook not found ($mdbook). Install the version CI pins:" >&2
  echo "  cargo install mdbook --version $pinned --locked" >&2
  echo "or set MDBOOK to its path." >&2
  exit 1
fi
have=$("$mdbook" --version | sed 's/^mdbook v//')
if [ "$have" != "$pinned" ]; then
  echo "warning: mdBook $have here, CI pins $pinned; the site may differ" >&2
fi

repo_name=${REPO_URL##*/}
out=target/docs-site
MDBOOK_BOOK__TITLE="$PRODUCT_NAME" \
MDBOOK_OUTPUT__HTML__GIT_REPOSITORY_URL="$REPO_URL" \
MDBOOK_OUTPUT__HTML__SITE_URL="/$repo_name/" \
  "$mdbook" build --dest-dir "$out"

# mdBook copies every file under docs/ that is not a chapter. The evidence
# logs are linked on the repository instead, so they leave the site.
rm -rf "$out/evidence"

em_dash=$(printf '\342\200\224')
if grep -rl -e "$em_dash" -e '&mdash;' "$out" --include='*.html'; then
  echo "error: em dash in a rendered page" >&2
  exit 1
fi
echo "site in $out"
