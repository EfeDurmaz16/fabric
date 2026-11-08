#!/bin/sh
# .github/workflows/release.yml is what `dist generate` writes from
# dist-workspace.toml, with scripts/release-workflow.patch applied on top.
# dist-workspace.toml marks the file allow-dirty, so dist neither rewrites it
# nor checks it; this script does both.
#
# Usage: scripts/release-workflow.sh           regenerate release.yml
#        scripts/release-workflow.sh --check   fail if release.yml differs
#
# After changing dist-workspace.toml or the dist version, run it without
# --check. To change the hand edits, edit release.yml, then rewrite the
# patch as the diff between a fresh `dist generate` and the edited file.
set -eu

mode=${1:-write}
case $mode in
write | --check) ;;
*)
  echo "usage: $0 [--check]" >&2
  exit 2
  ;;
esac

toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"
workflow=.github/workflows/release.yml
patch=scripts/release-workflow.patch

# dist refuses to generate a file it is told to leave alone, so it runs on a
# copy of the tracked and new files whose config drops the allow-dirty line.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
git ls-files --cached --others --exclude-standard >"$work/files"
mkdir "$work/tree"
tar -cf - -T "$work/files" | tar -xf - -C "$work/tree"
sed '/^allow-dirty = \["ci"\]$/d' dist-workspace.toml >"$work/tree/dist-workspace.toml"
(cd "$work/tree" && dist generate --mode=ci >&2)
(cd "$work/tree" && git apply "$toplevel/$patch")

if [ "$mode" = --check ]; then
  if ! cmp -s "$work/tree/$workflow" "$workflow"; then
    diff -u "$work/tree/$workflow" "$workflow" >&2 || true
    echo "error: $workflow is not dist's output plus $patch; run $0" >&2
    exit 1
  fi
  echo "$workflow matches dist-workspace.toml and $patch" >&2
else
  cp "$work/tree/$workflow" "$workflow"
fi
