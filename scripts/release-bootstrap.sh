#!/usr/bin/env bash
# Cuts a Geneva release on a machine that has nothing checked out yet.
# Makes sure gh is acting as the right account, clones or updates the
# repository, optionally bumps the version and moves the changelog, then
# hands over to scripts/release-local.sh to build and publish.
#
# Usage: release-bootstrap.sh [--dir path] [--bump X.Y.Z | --tag vX.Y.Z]
#                             [--targets a,b,c] [--no-publish]
#
#   release-bootstrap.sh                      report what a release would be
#   release-bootstrap.sh --bump 0.4.4         cut 0.4.4 from Unreleased
#   release-bootstrap.sh --tag v0.4.3         release the version already set
#
# Safe to run more than once: without --bump or --tag it only looks.
#
# Needs: git, gh logged in as fbnt, docker for the Linux builds, and the
# build tools scripts/build-media-libs.sh lists for the macOS one.

set -euo pipefail

ACCOUNT=fbnt
REPO=geneva-render/geneva
GIT_NAME=fbnt
GIT_EMAIL=8363862+fbnt@users.noreply.github.com

dir=./geneva
bump=
tag=
targets=
publish=--publish

while [ $# -gt 0 ]; do
  case $1 in
    --dir) dir=$2; shift ;;
    --dir=*) dir=${1#--dir=} ;;
    --bump) bump=$2; shift ;;
    --bump=*) bump=${1#--bump=} ;;
    --tag) tag=$2; shift ;;
    --tag=*) tag=${1#--tag=} ;;
    --targets) targets=$2; shift ;;
    --targets=*) targets=${1#--targets=} ;;
    --no-publish) publish= ;;
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ -n "$bump" ] && [ -n "$tag" ] && die "give --bump or --tag, not both"
[ -z "$bump" ] || case $bump in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) die "--bump takes a version like 0.4.4, not '$bump'" ;;
esac
[ -z "$tag" ] || case $tag in
  v[0-9]*) ;;
  *) die "--tag takes a tag like v0.4.4, not '$tag'" ;;
esac

# ------------------------------------------------------------- the tools

command -v git >/dev/null || die "git is not installed (xcode-select --install)"
command -v gh >/dev/null || die "gh is not installed (brew install gh)"
# Everything else is checked by release-local.sh, which knows which
# targets need what. cargo is checked here because --bump refreshes
# Cargo.lock before that runs.
[ -z "$bump" ] || command -v cargo >/dev/null || die "cargo is not installed, and --bump refreshes Cargo.lock.
    Install the Rust toolchain:
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"

# ----------------------------------------------------------- the account

say "checking which GitHub account gh is using"

# The token is looked up for this account by name and passed on through
# the environment, so nothing here changes which account gh is set to
# globally and the other accounts on this machine are left alone.
token=$(gh auth token --user "$ACCOUNT" 2>/dev/null || true)
if [ -z "$token" ]; then
  active=$(GH_TOKEN= gh api user -q .login 2>/dev/null || true)
  if [ "$active" = "$ACCOUNT" ]; then
    token=$(gh auth token)
  elif [ -n "$active" ]; then
    die "gh is logged in as $active, not $ACCOUNT. Run: gh auth login --hostname github.com"
  else
    die "gh is not logged in. Run: gh auth login --hostname github.com"
  fi
fi
export GH_TOKEN=$token

login=$(gh api user -q .login 2>/dev/null || true)
[ "$login" = "$ACCOUNT" ] || die "that token belongs to '${login:-nobody}', not $ACCOUNT"
gh api "repos/$REPO" -q .full_name >/dev/null 2>&1 || die "$ACCOUNT cannot see $REPO"
echo "    $login, and $REPO is readable"

# ------------------------------------------------------------ the clone

if [ -d "$dir/.git" ]; then
  # The configured value, not `remote get-url`, which applies any
  # url.<base>.insteadOf rewrite and is a common thing to have set up
  # when juggling several accounts.
  origin=$(git -C "$dir" config --get remote.origin.url 2>/dev/null || true)
  case $origin in
    *"$REPO"*) ;;
    *) die "$dir is a checkout of '$origin', not $REPO" ;;
  esac
  say "updating $dir"
  [ -z "$(git -C "$dir" status --porcelain)" ] || die "$dir has uncommitted changes"
  git -C "$dir" fetch -q origin main
  git -C "$dir" checkout -q main
  git -C "$dir" reset -q --hard origin/main
elif [ -e "$dir" ]; then
  die "$dir already exists and is not a git checkout"
else
  say "cloning $REPO into $dir"
  gh repo clone "$REPO" "$dir" -- --quiet
fi

dir=$(cd "$dir" && pwd)

# Pushes and tags from this checkout go out as the account above, whatever
# the global git config says.
git -C "$dir" config --local user.name "$GIT_NAME"
git -C "$dir" config --local user.email "$GIT_EMAIL"
git -C "$dir" config --local --replace-all credential."https://github.com".helper "!gh auth git-credential"

cd "$dir"
echo "    at $(git rev-parse --short HEAD), $(git log -1 --format=%s | cut -c1-60)"

# ------------------------------------------------------------- the state

version=$(sed -n '/^\[workspace.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
latest=$(git tag --sort=-v:refname | head -1)
# An entry starts with a bold title; the bullets under one do not count.
pending=$(awk '/^## Unreleased/{on=1;next} /^## [0-9]/{on=0} on' CHANGELOG.md | grep -c '^- \*\*' || true)

say "where the project is"
printf '    Cargo.toml version   %s\n' "$version"
printf '    latest tag           %s\n' "${latest:-none}"
printf '    unreleased entries   %s\n' "$pending"

if [ -z "$bump" ] && [ -z "$tag" ]; then
  cat <<EOF

Nothing asked for, so nothing done. To cut a release:

    $0 --bump $(awk -F. -v v="$version" 'BEGIN{split(v,p,".");print p[1]"."p[2]"."p[3]+1}')    move the $pending unreleased entries into a new version
    $0 --tag v$version    release $version, which is already set

EOF
  exit 0
fi

# -------------------------------------------------------------- the bump

if [ -n "$bump" ]; then
  [ "$pending" -gt 0 ] || die "CHANGELOG.md has no unreleased entries to release"
  grep -q "^## $bump\( \|$\)" CHANGELOG.md && die "CHANGELOG.md already has a $bump section"
  git rev-parse -q --verify "refs/tags/v$bump" >/dev/null && die "the tag v$bump already exists"

  say "bumping $version to $bump and moving the changelog"

  # Cargo.toml and CHANGELOG.md are edited before anything is committed,
  # so put them back if this does not get as far as the commit. Leaving
  # them half-written makes the next run refuse over a dirty tree.
  bump_committed=false
  bump_rollback() {
    $bump_committed && return 0
    bump_committed=true
    git checkout -- Cargo.toml Cargo.lock CHANGELOG.md 2>/dev/null || true
    echo "    put Cargo.toml, Cargo.lock and CHANGELOG.md back" >&2
  }
  trap bump_rollback EXIT

  # The version is written ten times: once under [workspace.package] and
  # once in each path dependency on a crate in this workspace. All of
  # them move together, and nothing else is touched.
  awk -v old="$version" -v new="$bump" '
    BEGIN { esc = old; gsub(/\./, "\\.", esc)
            pat = "version = \"" esc "\""
            rep = "version = \"" new "\"" }
    /^\[/ { insec = ($0 == "[workspace.package]") }
    insec && $0 ~ "^" pat "$" { print rep; changed++; next }
    /path = "crates\// { n = gsub(pat, rep); changed += n }
    { print }
    END { if (changed < 2) { print "bump changed only " changed " lines" > "/dev/stderr"; exit 1 } }
  ' Cargo.toml > Cargo.toml.new && mv Cargo.toml.new Cargo.toml

  awk -v new="$bump" -v today="$(date +%Y-%m-%d)" '
    $0 == "## Unreleased" { print "## Unreleased"; print ""; print "## " new " (" today ")"; next }
    { print }
  ' CHANGELOG.md > CHANGELOG.md.new && mv CHANGELOG.md.new CHANGELOG.md

  # --locked is used for the release builds, so the lock file has to
  # carry the new version before anything is built.
  cargo update --workspace --quiet

  git add Cargo.toml Cargo.lock CHANGELOG.md
  git commit -q -m "Release $bump"
  bump_committed=true
  git push -q origin main
  echo "    committed and pushed as $(git rev-parse --short HEAD)"
  tag=v$bump
fi

# ------------------------------------------------------------ the release

say "handing over to scripts/release-local.sh $tag ${publish:-(build only)}"
set -- "$tag"
[ -n "$publish" ] && set -- "$@" "$publish"
[ -n "$targets" ] && set -- "$@" "--targets=$targets"
exec scripts/release-local.sh "$@"
