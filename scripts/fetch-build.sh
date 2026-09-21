#!/usr/bin/env bash
# Fetches a build of geneva made by GitHub Actions and unpacks it, so a
# machine that cannot build the media libraries itself can still run one.
#
# Usage: fetch-build.sh [--ref REF] [--rerun] [--dir DIR]
#   --ref REF   the branch or tag to build from; main by default
#   --rerun     build again even when a finished build of REF exists
#   --dir DIR   where to unpack; ./geneva-build by default
#
# Without --rerun it takes the newest successful run of release.yml for
# REF and downloads that, which costs nothing and takes seconds. With
# --rerun, or when there is no such run, it dispatches the workflow with
# no tag, which builds the binaries and releases nothing, and waits.
#
# Needs: gh, logged in to an account that can read the repository and run
# its workflows.
set -u

ref=main
rerun=0
dir=geneva-build
while [ $# -gt 0 ]; do
  case "$1" in
    --ref) ref=$2; shift 2 ;;
    --rerun) rerun=1; shift ;;
    --dir) dir=$2; shift 2 ;;
    -h | --help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "fetch-build.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

command -v gh >/dev/null 2>&1 || {
  echo "fetch-build.sh: gh is not installed; see https://cli.github.com" >&2
  exit 1
}
gh auth status >/dev/null 2>&1 || {
  echo "fetch-build.sh: gh is not logged in; run 'gh auth login'" >&2
  exit 1
}

# The artifact this machine can run. The workflow builds these three.
case "$(uname -s) $(uname -m)" in
  "Darwin arm64") target=aarch64-apple-darwin ;;
  "Linux x86_64") target=x86_64-unknown-linux-gnu ;;
  "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-gnu ;;
  *) echo "fetch-build.sh: no build for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

latest_run() {
  gh run list --workflow=release.yml --branch "$ref" --status success \
    --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null
}

run=""
if [ "$rerun" = 0 ]; then run=$(latest_run); fi
if [ -z "$run" ]; then
  echo "building $ref on GitHub Actions (about fifteen minutes when the media caches are cold)"
  gh workflow run release.yml --ref "$ref" || exit 1
  # The run needs a moment to exist before it can be watched.
  sleep 10
  run=$(gh run list --workflow=release.yml --branch "$ref" --limit 1 \
    --json databaseId --jq '.[0].databaseId')
  [ -n "$run" ] || { echo "fetch-build.sh: the run did not start" >&2; exit 1; }
  gh run watch "$run" --exit-status || {
    echo "fetch-build.sh: the build failed; see gh run view $run --log-failed" >&2
    exit 1
  }
else
  echo "using the finished build in run $run (pass --rerun to build $ref again)"
fi

rm -rf "$dir"
mkdir -p "$dir"
gh run download "$run" -n "geneva-$target" -D "$dir" || {
  echo "fetch-build.sh: no artifact geneva-$target in run $run" >&2
  exit 1
}
tarball=$(find "$dir" -name 'geneva-*.tar.gz' | head -1)
[ -n "$tarball" ] || { echo "fetch-build.sh: the artifact has no tarball" >&2; exit 1; }
tar xzf "$tarball" -C "$dir"
unpacked=$(find "$dir" -mindepth 1 -maxdepth 1 -type d -name 'geneva-*' | head -1)
[ -n "$unpacked" ] || { echo "fetch-build.sh: the tarball has no directory" >&2; exit 1; }
[ -x "$unpacked/geneva" ] || {
  echo "fetch-build.sh: no binary in $unpacked" >&2
  exit 1
}
# A binary from a download carries the quarantine flag, and an unsigned
# one is then refused outright.
if command -v xattr >/dev/null 2>&1; then
  xattr -dr com.apple.quarantine "$unpacked" 2>/dev/null || true
fi

echo
echo "$("$unpacked/geneva" --version) in $unpacked"
echo
echo "next:"
echo "  cd $unpacked && ./check.sh --keep"
echo "  cd $unpacked && ./geneva --help"
