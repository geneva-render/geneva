#!/usr/bin/env bash
# Builds a release on this machine and, with --publish, tags it and puts
# it on GitHub. Does the same three builds as .github/workflows/release.yml
# without using any GitHub Actions minutes.
#
# Usage: scripts/release-local.sh <tag> [--publish] [--targets a,b,c]
#
#   scripts/release-local.sh v0.4.4              build and package only
#   scripts/release-local.sh v0.4.4 --publish    also tag, push and release
#
# The macOS binary is built on the host and needs an Apple Silicon Mac.
# The two Linux binaries are built in ubuntu:22.04 containers, which is
# where their glibc 2.35 floor comes from; on Apple Silicon the arm64 one
# is native and the x86_64 one is emulated, so it is the slow one. Set
# APPLE_CERTIFICATE_P12 and the other APPLE_* variables the workflow reads
# to get a signed and notarized macOS binary (APPLE_API_KEY may hold the
# .p8 key's text or its path); without them it ships unsigned, as the
# workflow also does.
#
# Needs: docker (for the Linux targets), gh and a push remote (--publish),
# and the build tools scripts/build-media-libs.sh lists (macOS target).
#
# Colima works in place of Docker Desktop, but not with its defaults: it
# mounts the home directory read-only and gives the VM 2 CPUs and 2 GiB,
# which is neither writable enough nor big enough to build ffmpeg and
# link with thin LTO. Start it with
#   colima start --cpu 6 --memory 12 --disk 80 \
#     --vm-type=vz --vz-rosetta --mount-type=virtiofs --mount $HOME:w
# and this checks the mount is writable before it builds anything.
#
# Tarballs are written to dist/. Everything else is built under
# target/local-release/<target>, so the three builds do not share anything
# and the checkout is left as it was.

set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

MACOS=aarch64-apple-darwin
LINUX_ARM=aarch64-unknown-linux-gnu
LINUX_X86=x86_64-unknown-linux-gnu

tag=
publish=false
targets=

while [ $# -gt 0 ]; do
  case $1 in
    --publish) publish=true ;;
    --targets) targets=$2; shift ;;
    --targets=*) targets=${1#--targets=} ;;
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "$0"; exit 0 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) [ -z "$tag" ] || { echo "give one tag, not two" >&2; exit 2; }; tag=$1 ;;
  esac
  shift
done

[ -n "$tag" ] || { echo "usage: scripts/release-local.sh <tag> [--publish] [--targets a,b,c]" >&2; exit 2; }
case $tag in
  v[0-9]*) ;;
  *) echo "a tag looks like v0.4.4, not '$tag'" >&2; exit 2 ;;
esac
version=${tag#v}

# Which targets to build. Everything the host can manage, unless asked.
if [ -z "$targets" ]; then
  targets="$LINUX_X86,$LINUX_ARM"
  [ "$(uname -s)" = Darwin ] && targets="$MACOS,$targets"
fi
targets=${targets//,/ }

say() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- checks

say "checking the tree"

manifest=$(sed -n '/^\[workspace.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
[ "$manifest" = "$version" ] || die "Cargo.toml is at $manifest but the tag says $version"

grep -q "^## $version\( \|$\)" CHANGELOG.md || die "CHANGELOG.md has no '## $version' section"

if $publish; then
  [ -z "$(git status --porcelain)" ] || die "the working tree is not clean"
  branch=$(git rev-parse --abbrev-ref HEAD)
  [ "$branch" = main ] || die "releases are cut from main, not $branch"
  git fetch -q origin main
  [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "HEAD is not what origin/main is"
  command -v gh >/dev/null || die "gh is not installed"
  gh auth status >/dev/null 2>&1 || die "gh is not logged in"
  git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "the tag $tag already exists here"
  ! gh release view "$tag" >/dev/null 2>&1 || die "a release for $tag already exists on GitHub"
fi

needs_docker=false
for t in $targets; do [ "$t" = "$MACOS" ] || needs_docker=true; done
if $needs_docker; then
  command -v docker >/dev/null || die "docker is needed for the Linux targets"
  docker info >/dev/null 2>&1 || die "the docker daemon is not running"
fi

for t in $targets; do
  if [ "$t" = "$MACOS" ]; then
    [ "$(uname -s)" = Darwin ] || die "$MACOS can only be built on macOS"
    [ "$(uname -m)" = arm64 ] || die "$MACOS needs an Apple Silicon Mac"
    # The Linux targets bring their own toolchain in the container. This
    # one builds on the host, so the host needs the lot. Report all of
    # what is missing at once rather than one per run.
    missing=
    for tool in cargo cmake meson ninja nasm pkg-config; do
      command -v "$tool" >/dev/null || missing="$missing $tool"
    done
    if [ -n "$missing" ]; then
      # cargo comes from rustup, the rest from brew, so the advice
      # splits in two. Built by hand rather than by filtering a pipe,
      # which exits non-zero when it filters everything out.
      brewable=
      wants_rust=false
      for tool in $missing; do
        if [ "$tool" = cargo ]; then wants_rust=true; else brewable="$brewable $tool"; fi
      done
      note=
      $wants_rust && note="
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
      [ -n "$brewable" ] && note="$note
      brew install$brewable"
      die "$MACOS needs these and they are not installed:$missing$note"
    fi
  fi
done

dist=$root/dist
mkdir -p "$dist"
# The crate registry is the same whatever the architecture, so one is
# shared; everything built is not, so each target keeps its own.
shared_cargo=$root/target/local-release/cargo
mkdir -p "$shared_cargo"

# Colima, and some other runtimes, mount the host read-only unless asked
# otherwise. The build would then fail somewhere deep inside configure,
# so ask the question here where the answer can say what to do about it.
if $needs_docker; then
  say "checking the container can write to the build directory"
  if ! docker run --rm -v "$shared_cargo:/probe" ubuntu:22.04 \
      sh -c 'touch /probe/.probe && rm -f /probe/.probe' >/dev/null 2>&1; then
    die "the container cannot write to $shared_cargo.
    Colima mounts the home directory read-only unless told not to. Restart
    it with the mount writable and room to build in:
      colima stop
      colima start --cpu 6 --memory 12 --disk 80 \\
        --vm-type=vz --vz-rosetta --mount-type=virtiofs --mount \$HOME:w"
  fi
fi

# --------------------------------------------------------------- package

# package <target> <binary> <media-prefix>
package() {
  local target=$1 bin=$2 prefix=$3
  local name=geneva-$tag-$target
  local stage=$root/target/local-release/$target/$name
  rm -rf "$stage"
  mkdir -p "$stage/licenses"
  cp "$bin" README.md LICENSE THIRD-PARTY-NOTICES.md "$stage/"
  cp scripts/install.sh scripts/check.sh "$stage/"
  cp licenses/* "$prefix"/share/licenses/* "$stage/licenses/"
  tar czf "$dist/$name.tar.gz" -C "$(dirname "$stage")" "$name"
  say "packaged $(cd "$dist" && ls -lh "$name.tar.gz" | awk '{print $9, $5}')"
}

# --------------------------------------------------------- macOS signing

# The certificate goes in a keychain of its own, which is added to the
# search list for as long as signing takes and then removed again. The
# workflow can replace the list outright because its runner is thrown
# away; here it is someone's Mac, and dropping the login keychain from
# it would be rude.
# The trap below also runs on EXIT, by which time a function's locals are
# gone, so everything it touches is global on purpose.
_sign_restored=true
_sign_keychain=
_sign_cert=
_sign_before=()

restore_keychains() {
  $_sign_restored && return 0
  _sign_restored=true
  if [ ${#_sign_before[@]} -gt 0 ]; then
    security list-keychain -d user -s "${_sign_before[@]}" 2>/dev/null || true
  fi
  [ -n "$_sign_keychain" ] && security delete-keychain "$_sign_keychain" 2>/dev/null
  [ -n "$_sign_cert" ] && rm -f "$_sign_cert"
  return 0
}

# A string field of notarytool's one-line JSON answer.
json_field() {
  printf '%s' "$2" | sed -n "s/.*\"$1\" *: *\"\([^\"]*\)\".*/\1/p"
}

sign_macos() {
  local bin=$1 work=$2 password line
  password=$(uuidgen)
  _sign_keychain=$work/signing.keychain-db
  _sign_cert=$work/certificate.p12

  # The existing search list, one path per line, kept as an array so a
  # path with a space in it survives.
  _sign_before=()
  while IFS= read -r line; do
    line=${line#"${line%%[![:space:]]*}"}
    line=${line%\"}; line=${line#\"}
    [ -n "$line" ] && _sign_before+=("$line")
  done < <(security list-keychain -d user)

  _sign_restored=false
  trap restore_keychains RETURN EXIT

  security delete-keychain "$_sign_keychain" 2>/dev/null || true
  echo "$APPLE_CERTIFICATE_P12" | base64 --decode > "$_sign_cert"
  chmod 600 "$_sign_cert"
  security create-keychain -p "$password" "$_sign_keychain"
  security set-keychain-settings -lut 21600 "$_sign_keychain"
  security unlock-keychain -p "$password" "$_sign_keychain"
  security import "$_sign_cert" -P "$APPLE_CERTIFICATE_PASSWORD" \
    -A -t cert -f pkcs12 -k "$_sign_keychain"
  security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$_sign_keychain"

  # Put it at the front of the list rather than in place of it.
  security list-keychain -d user -s "$_sign_keychain" "${_sign_before[@]}"

  codesign --sign "$APPLE_SIGNING_IDENTITY" --options runtime --timestamp --force "$bin"
  codesign --verify --strict --verbose=2 "$bin"

  # Notarized with an App Store Connect API key, as in the workflow;
  # notarytool exits 0 on a rejection, so the status is read.
  local key=$APPLE_API_KEY result
  if [ ! -f "$key" ]; then
    key=$work/AuthKey.p8
    printf '%s\n' "$APPLE_API_KEY" > "$key"
    chmod 600 "$key"
  fi
  local auth=(--key "$key" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID")
  ditto -c -k "$bin" "$work/geneva.zip"
  result=$(xcrun notarytool submit "$work/geneva.zip" --wait --output-format json "${auth[@]}")
  echo "$result"
  if [ "$(json_field status "$result")" != Accepted ]; then
    xcrun notarytool log "$(json_field id "$result")" "${auth[@]}" || true
    [ "$key" = "$work/AuthKey.p8" ] && rm -f "$key"
    die "notarization was not accepted"
  fi
  [ "$key" = "$work/AuthKey.p8" ] && rm -f "$key"
}

# ----------------------------------------------------------- macOS build

build_macos() {
  local work=$root/target/local-release/$MACOS
  local prefix=$work/media-libs
  mkdir -p "$work"

  say "building the media libraries for $MACOS"
  MEDIA_SRC=$work/media-src scripts/build-media-libs.sh "$prefix"

  say "building geneva for $MACOS"
  PKG_CONFIG_PATH=$prefix/lib/pkgconfig \
  CARGO_TARGET_DIR=$work/target \
  CARGO_HOME=$shared_cargo \
    cargo build --release --locked

  local bin=$work/target/release/geneva

  if [ -n "${APPLE_CERTIFICATE_P12:-}" ]; then
    say "signing and notarizing"
    sign_macos "$bin" "$work"
  else
    say "no APPLE_CERTIFICATE_P12, shipping the macOS binary unsigned"
  fi

  package "$MACOS" "$bin" "$prefix"
}

# ----------------------------------------------------------- Linux build

build_linux() {
  local target=$1 platform=$2
  local work=$root/target/local-release/$target
  mkdir -p "$work"

  say "building $target in an ubuntu:22.04 container ($platform)"
  docker run --rm --platform "$platform" \
    -v "$root:/src" -v "$work:/work" -v "$shared_cargo:/cargo" \
    -w /src -e "CARGO_TERM_COLOR=always" \
    ubuntu:22.04 bash -euo pipefail -c '
      export DEBIAN_FRONTEND=noninteractive
      if [ ! -x /work/rustup/bin/cargo ]; then
        apt-get update -qq
        apt-get install -y -qq build-essential cmake meson ninja-build nasm \
          pkg-config clang zlib1g-dev curl git ca-certificates >/dev/null
        export RUSTUP_HOME=/work/rustup CARGO_HOME=/work/rustup
        curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal >/dev/null
      else
        apt-get update -qq
        apt-get install -y -qq build-essential cmake meson ninja-build nasm \
          pkg-config clang zlib1g-dev curl git ca-certificates >/dev/null
      fi
      export PATH=/work/rustup/bin:$PATH
      export CARGO_HOME=/cargo RUSTUP_HOME=/work/rustup

      # The source tree is mounted, so everything written goes to /work
      # and the checkout is left as it was.
      MEDIA_SRC=/work/media-src /src/scripts/build-media-libs.sh /work/media-libs
      PKG_CONFIG_PATH=/work/media-libs/lib/pkgconfig \
      CARGO_TARGET_DIR=/work/target \
        cargo build --release --locked
    '

  package "$target" "$work/target/release/geneva" "$work/media-libs"
}

# ------------------------------------------------------------------ build

say "building $tag: $targets"
for t in $targets; do
  case $t in
    "$MACOS") build_macos ;;
    "$LINUX_ARM") build_linux "$LINUX_ARM" linux/arm64 ;;
    "$LINUX_X86") build_linux "$LINUX_X86" linux/amd64 ;;
    *) die "unknown target: $t" ;;
  esac
done

say "built"
ls -lh "$dist"/geneva-"$tag"-*.tar.gz | awk '{print "   ", $9, $5}'

$publish || { say "not publishing; pass --publish to tag and release"; exit 0; }

# ---------------------------------------------------------------- publish

# The notes are the tag's section of CHANGELOG.md, read the same way
# .github/workflows/release.yml reads them.
notes=$root/target/local-release/notes.md
awk -v v="$version" '/^## /{on = ($2 == v)} on && !/^## /' CHANGELOG.md \
  | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}' > "$notes"
[ -s "$notes" ] || die "no changelog entry for $tag"

say "release notes"
sed 's/^/    /' "$notes"

# Pushing the tag still starts ci.yml and release.yml, which both watch
# v* and will both fail immediately while the account is out of minutes.
# That costs nothing and changes nothing, but it leaves two red runs on
# the tag. `gh workflow disable release.yml` silences the second.
for wf in release.yml ci.yml; do
  state=$(gh workflow view "$wf" --json state -q .state 2>/dev/null || echo unknown)
  [ "$state" = active ] && echo "note: $wf is enabled and will run (and fail) on this tag"
done

printf '\nTag %s at %s and publish %d files? [y/N] ' "$tag" "$(git rev-parse --short HEAD)" \
  "$(ls "$dist"/geneva-"$tag"-*.tar.gz | wc -l | tr -d ' ')"
read -r reply
case $reply in [yY]*) ;; *) die "stopped" ;; esac

git tag -a "$tag" -m "geneva $version"
git push origin "$tag"
gh release create "$tag" --title "$tag" --notes-file "$notes" \
  --target "$(git rev-parse HEAD)" "$dist"/geneva-"$tag"-*.tar.gz

say "released: $(gh release view "$tag" --json url -q .url)"
