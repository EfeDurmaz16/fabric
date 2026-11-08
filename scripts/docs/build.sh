#!/bin/sh
# Builds the docs site with mdBook into target/docs-site.
#
# Usage: scripts/docs/build.sh
# mdBook is $MDBOOK when set, else `mdbook` on PATH; CI pins the version
# in .github/workflows/docs.yml. The site's title and repository link come
# from scripts/docs/names.env, so book.toml names neither. The rendered
# pages are then checked for an em dash, which no page may carry.
set -eu

toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"
. scripts/docs/names.env

repo_name=${REPO_URL##*/}
out=target/docs-site
MDBOOK_BOOK__TITLE="$PRODUCT_NAME" \
MDBOOK_OUTPUT__HTML__GIT_REPOSITORY_URL="$REPO_URL" \
MDBOOK_OUTPUT__HTML__SITE_URL="/$repo_name/" \
  "${MDBOOK:-mdbook}" build --dest-dir "$out"

# mdBook copies every file under docs/ that is not a chapter. The evidence
# logs are linked on the repository instead, so they leave the site.
rm -rf "$out/evidence"

em_dash=$(printf '\342\200\224')
if grep -rl -e "$em_dash" -e '&mdash;' "$out" --include='*.html'; then
  echo "error: em dash in a rendered page" >&2
  exit 1
fi
echo "site in $out"
