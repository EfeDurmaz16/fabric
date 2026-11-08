#!/bin/sh
# Proves that the docs checks fire. Each case copies the working tree into
# a scratch repository, breaks one thing, and expects the check that guards
# it to fail; the unbroken copy must pass. The CLI reference and the site
# cases run when the binary ($DOCS_CLI or target/debug/<CLI_BIN>) and
# mdBook ($MDBOOK or on PATH) are there, and say so when they are not.
#
# Usage: scripts/test-check-docs.sh
set -eu

toplevel=$(git rev-parse --show-toplevel)
. "$toplevel/scripts/docs/names.env"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0
bin=${DOCS_CLI:-$toplevel/target/debug/$CLI_BIN}
mdbook=${MDBOOK:-mdbook}

# A scratch repository holding the working tree's tracked and new files,
# made once; each case starts from a copy of it.
mkdir -p "$work/pristine"
(cd "$toplevel" && git ls-files -z -co --exclude-standard | xargs -0 tar -cf - 2>/dev/null) \
  | tar -xf - -C "$work/pristine"
git -C "$work/pristine" init -q -b main
git -C "$work/pristine" add -A
fresh() {
  rm -rf "$work/repo"
  cp -R "$work/pristine" "$work/repo"
}

expect() {
  want=$1
  name=$2
  shift 2
  if (cd "$work/repo" && "$@") > /dev/null 2>&1; then got=pass; else got=fail; fi
  if [ "$got" = "$want" ]; then
    echo "ok   $name"
  else
    echo "FAIL $name: expected $want, got $got" >&2
    failures=$((failures + 1))
  fi
}

fresh
expect pass 'generated pages up to date' python3 scripts/docs/gen.py --check
expect pass 'relative links resolve' python3 scripts/docs/check-links.py

printf '\nSee [the page](no-such-page.md).\n' >> "$work/repo/docs/start/install.md"
expect fail 'a broken relative link' python3 scripts/docs/check-links.py

fresh
printf '# ADR 9999: A test\n\nStatus: proposed, 2026-11-08\n' > "$work/repo/docs/adr/9999-a-test.md"
expect fail 'a new ADR missing from the index' python3 scripts/docs/gen.py --check

fresh
printf '# ADR 9999: A test\n\nNo status line.\n' > "$work/repo/docs/adr/9999-a-test.md"
expect fail 'an ADR without a status line' python3 scripts/docs/gen.py

fresh
sed 's/| Stay on the source |/| Moves |/' "$work/repo/docs/start/what-moves.md" > "$work/repo/x" \
  && mv "$work/repo/x" "$work/repo/docs/start/what-moves.md"
expect fail 'the matrix changed but not copied' python3 scripts/docs/gen.py --check

fresh
sed 's/| Stay on the source |/| Moves |/' "$work/repo/README.md" > "$work/repo/x" && mv "$work/repo/x" "$work/repo/README.md"
expect fail 'the README matrix edited by hand' python3 scripts/docs/gen.py --check

fresh
sed '/^- `while_stopping`/d' "$work/repo/docs/concepts/ledger.md" > "$work/repo/x" \
  && mv "$work/repo/x" "$work/repo/docs/concepts/ledger.md"
expect fail 'a fault point dropped from the ledger page' python3 scripts/docs/gen.py --check

fresh
mkdir -p "$work/repo/docs/evidence/zz-new"
printf '# A new run\n' > "$work/repo/docs/evidence/zz-new/README.md"
expect fail 'new evidence missing from its index' python3 scripts/docs/gen.py --check

fresh
expect pass 'claims carry their qualifiers' python3 scripts/docs/check-claims.py
printf '\nProved on two real Macs.\n' >> "$work/repo/docs/start/install.md"
expect fail 'a two-Mac claim without its qualifier' python3 scripts/docs/check-claims.py

fresh
sed 's/cargo install mdbook --version [0-9.]*/cargo install mdbook --version 0.0.1/' "$work/repo/CONTRIBUTING.md" > "$work/repo/x" \
  && mv "$work/repo/x" "$work/repo/CONTRIBUTING.md"
expect fail 'an mdBook install command off the pin' python3 scripts/docs/check-claims.py

fresh
if [ -x "$bin" ]; then
  expect pass 'CLI reference matches the binary' env DOCS_CLI="$bin" scripts/docs/cli-reference.sh --check
  printf '#!/bin/sh\necho "Usage: %s <COMMAND>"\n' "$CLI_BIN" > "$work/fake-cli"
  chmod +x "$work/fake-cli"
  expect fail 'CLI reference of another binary' env DOCS_CLI="$work/fake-cli" scripts/docs/cli-reference.sh --check
else
  echo "skip CLI reference cases: no binary at $bin"
fi

if command -v "$mdbook" > /dev/null 2>&1; then
  expect pass 'the site builds' env MDBOOK="$mdbook" scripts/docs/build.sh
  expect pass 'the site links resolve' python3 scripts/docs/check-links.py --site target/docs-site
  printf '\nA dash: &mdash; here.\n' >> "$work/repo/docs/start/install.md"
  expect fail 'an em dash in a rendered page' env MDBOOK="$mdbook" scripts/docs/build.sh
else
  echo "skip site cases: no mdbook"
fi

if [ "$failures" -ne 0 ]; then
  echo "error: $failures case(s) did not behave" >&2
  exit 1
fi
