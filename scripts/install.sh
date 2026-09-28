#!/bin/sh
# Installs the geneva binary onto the PATH.
#
# From inside an unpacked release archive, installs the binary next to this
# script:
#
#   sh install.sh
#
# Otherwise downloads the release for this machine first:
#
#   curl -fsSL https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.sh | sh
#
# Options (environment variables):
#   GENEVA_VERSION   release tag to download, for example v0.1.1 (default: latest)
#   GENEVA_PREFIX    directory to install into (default: /usr/local/bin when
#                    writable, otherwise ~/.local/bin)
#   GITHUB_TOKEN     used only when the public download fails, for a fork or
#                    mirror kept private; never needed for this repository
set -eu

repo=geneva-render/geneva
api=https://api.github.com/repos/$repo

install_binary() {
  prefix=${GENEVA_PREFIX:-}
  if [ -z "$prefix" ]; then
    if [ -w /usr/local/bin ]; then prefix=/usr/local/bin; else prefix=$HOME/.local/bin; fi
  fi
  mkdir -p "$prefix"
  install -m 755 "$1" "$prefix/geneva"
  # A browser download carries the quarantine flag, which makes macOS refuse
  # to run an unsigned binary; the installed copy starts without it.
  if command -v xattr >/dev/null 2>&1; then xattr -d com.apple.quarantine "$prefix/geneva" 2>/dev/null || true; fi
  echo "installed $("$prefix/geneva" --version) to $prefix/geneva"
  case ":$PATH:" in
    *":$prefix:"*) ;;
    *) echo "add $prefix to your PATH, for example: export PATH=\"$prefix:\$PATH\"" ;;
  esac
}

# Inside an unpacked archive there is nothing to download. The archive
# itself may carry the quarantine flag when a browser fetched it, and
# macOS extends it to everything unpacked; the copies here are cleared
# too so that check.sh and the binary in place run as well.
here=$(cd "$(dirname "$0")" 2>/dev/null && pwd)
if [ -n "$here" ] && [ -x "$here/geneva" ]; then
  if command -v xattr >/dev/null 2>&1; then xattr -dr com.apple.quarantine "$here" 2>/dev/null || true; fi
  install_binary "$here/geneva"
  exit 0
fi

case "$(uname -s)" in
  Linux) os=unknown-linux-gnu ;;
  Darwin) os=apple-darwin ;;
  *) echo "install.sh: unsupported operating system $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) echo "install.sh: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
target=$arch-$os
if [ "$target" = x86_64-apple-darwin ]; then
  echo "install.sh: no prebuilt binary for Intel Macs; build from source (see README)" >&2
  exit 1
fi

auth=""
if [ -n "${GITHUB_TOKEN:-}" ]; then
  auth="Authorization: Bearer $GITHUB_TOKEN"
fi

# The repository is public, so nothing here needs a token; one found in the
# environment is only tried after the public route fails, since a stale
# token would otherwise turn a download anyone can make into an error.
latest() {
  curl -fsSL "$@" "$api/releases/latest" | tr -d '\n' | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1
}
version=${GENEVA_VERSION:-}
if [ -z "$version" ]; then
  version=$(latest) || version=""
  if [ -z "$version" ] && [ -n "$auth" ]; then
    version=$(latest -H "$auth") || version=""
  fi
  [ -n "$version" ] || { echo "install.sh: could not find the latest release" >&2; exit 1; }
fi
name=geneva-$version-$target
archive=$name.tar.gz

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "downloading $archive"
if ! curl -fsSL -o "$tmp/$archive" "https://github.com/$repo/releases/download/$version/$archive"; then
  [ -n "$auth" ] || { echo "install.sh: could not download $archive from release $version" >&2; exit 1; }
  # A private copy serves assets through the API only; pick the asset
  # whose name matches. The API pretty-prints, so join the lines first.
  asset_url=$(curl -fsSL -H "$auth" "$api/releases/tags/$version" \
    | tr -d '\n' | tr '{' '\n' | grep "\"name\": *\"$archive\"" | sed -n 's/.*"url": *"\([^"]*\/assets\/[0-9]*\)".*/\1/p' | head -1)
  [ -n "$asset_url" ] || { echo "install.sh: no $archive in release $version" >&2; exit 1; }
  curl -fsSL -H "$auth" -H "Accept: application/octet-stream" -o "$tmp/$archive" "$asset_url"
fi
tar xzf "$tmp/$archive" -C "$tmp"

install_binary "$tmp/$name/geneva"
