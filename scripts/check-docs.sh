#!/bin/sh
# Fails when the docs are wrong in a way a script can see: a generated
# page out of date (the CLI reference, the ADR and evidence indexes, the
# "what moves" matrix copies, the fault points), a broken relative link, a
# claim without the qualifier the evidence needs (scripts/docs/check-claims.py),
# or a site that does not build or renders an em dash. CI runs it on every pull request and
# push to main; scripts/check-hygiene.sh covers em dashes in the sources.
#
# Usage: scripts/check-docs.sh
# Needs python3, mdBook ($MDBOOK or on PATH) and the CLI binary
# ($DOCS_CLI, or target/debug/<CLI_BIN> from cargo build).
set -eu

toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"

python3 scripts/docs/gen.py --check
scripts/docs/cli-reference.sh --check
python3 scripts/docs/check-links.py
python3 scripts/docs/check-claims.py
scripts/docs/build.sh
python3 scripts/docs/check-links.py --site target/docs-site
