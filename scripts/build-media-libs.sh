#!/usr/bin/env bash
# Builds the media libraries Geneva links statically, from pinned sources,
# with only the components Geneva uses. Everything is LGPL or BSD licensed.
#
# Usage: scripts/build-media-libs.sh [prefix]
#   prefix defaults to target/media-libs (absolute path is printed at the end)
#   MEDIA_SRC is where sources are unpacked and built, target/media-src by
#   default. Give a build for another architecture its own, since the
#   sources are built in place.
#   MEDIA_HOST=x86_64-w64-mingw32 cross-builds for Windows from Linux, for
#   the x86_64-pc-windows-gnu Rust target.
#
# Needs: a C/C++ compiler, make, cmake, meson, ninja, nasm, pkg-config, curl,
# git. On Debian/Ubuntu: build-essential cmake meson ninja-build nasm
# pkg-config curl git, plus mingw-w64 for Windows. On macOS: xcode command
# line tools plus `brew install cmake meson ninja nasm pkg-config`.
set -euo pipefail

FFMPEG_VERSION=7.1.1
OPENH264_VERSION=2.5.0
LIBVPX_VERSION=1.15.0
SVTAV1_VERSION=2.3.0
DAV1D_VERSION=1.5.1
OPUS_VERSION=1.5.2
NVCODEC_VERSION=12.2.72.0
OGG_VERSION=1.3.5
VORBIS_VERSION=1.3.7
LAME_VERSION=3.100
ZLIB_VERSION=1.3.1

root=$(cd "$(dirname "$0")/.." && pwd)
prefix_arg=${1:-$root/target/media-libs}
mkdir -p "$prefix_arg"
prefix=$(cd "$prefix_arg" && pwd)
# Sources are unpacked and built in place, so a build for another
# architecture needs its own scratch: MEDIA_SRC keeps them apart.
src=${MEDIA_SRC:-$root/target/media-src}
jobs=${JOBS:-$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)}
mkdir -p "$prefix" "$src" "$prefix/share/licenses"
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig"

# The system the libraries are for: this machine, or Windows through
# MinGW-w64. Each build system is told the same thing its own way.
host=${MEDIA_HOST:-}
case "$host" in
  "") os=$(uname -s) ;;
  x86_64-w64-mingw32) os=Windows ;;
  *) echo "build-media-libs.sh: unsupported MEDIA_HOST $host" >&2; exit 1 ;;
esac
if [ "$os" = Windows ]; then
  # Nothing but the prefix may satisfy a dependency of a cross build.
  export PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig"
  export CFLAGS="${CFLAGS:-} -O2"
  export CXXFLAGS="${CXXFLAGS:-} -O2"
  meson_cross="$src/meson-$host.txt"
  cat >"$meson_cross" <<EOF
[binaries]
c = '$host-gcc'
cpp = '$host-g++'
ar = '$host-ar'
strip = '$host-strip'
windres = '$host-windres'
nasm = 'nasm'

[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF
  cmake_toolchain="$src/cmake-$host.cmake"
  cat >"$cmake_toolchain" <<EOF
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER $host-gcc)
set(CMAKE_CXX_COMPILER $host-g++)
set(CMAKE_RC_COMPILER $host-windres)
set(CMAKE_ASM_NASM_COMPILER nasm)
set(CMAKE_FIND_ROOT_PATH /usr/$host)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
EOF
  openh264_make="OS=mingw_nt ARCH=x86_64 CC=$host-gcc CXX=$host-g++ AR=$host-ar"
else
  export CFLAGS="${CFLAGS:-} -fPIC -O2"
  export CXXFLAGS="${CXXFLAGS:-} -fPIC -O2"
  meson_cross=""
  cmake_toolchain=""
  openh264_make=""
fi

fetch() { # name url [strip-components, default 1] [fallback-url fallback-strip]
  local name=$1 url=$2 strip=${3:-1} dir=$src/$1
  # A source tree counts only when its marker says the unpack finished;
  # a directory alone may be the remains of an interrupted or pruned one.
  if [ -f "$dir/.fetched" ]; then return; fi
  echo "==> fetching $name"
  rm -rf "$dir"
  if ! download "$name" "$url"; then
    [ $# -ge 5 ] || { echo "error: could not fetch $name from $url" >&2; exit 1; }
    echo "==> $url failed; fetching $name from $4"
    url=$4 strip=$5
    download "$name" "$url" || { echo "error: could not fetch $name from $url" >&2; exit 1; }
  fi
  mkdir -p "$dir"
  tar xf "$src/$name.tar" --strip-components="$strip" -C "$dir"
  rm -f "$src/$name.tar"
  touch "$dir/.fetched"
}

# Downloads url into $src/name.tar and checks that tar can read it, three
# times at most, waiting longer each time. -f makes an error page (a 503
# from a busy mirror) a failure; the listing catches a server that answers
# 200 with something that is not the archive, such as a bot check's
# challenge page (code.videolan.org, 2026-10-05, which failed a macOS
# build).
download() { # name url
  local try
  for try in 1 2 3; do
    if curl -fsSL --retry 5 --retry-all-errors --retry-delay 3 -o "$src/$1.tar" "$2" &&
      tar tf "$src/$1.tar" >/dev/null 2>&1; then
      return 0
    fi
    echo "==> $2 did not give a readable archive (try $try of 3)" >&2
    rm -f "$src/$1.tar"
    [ "$try" = 3 ] || sleep $((try * 15))
  done
  return 1
}

done_marker() { [ -f "$prefix/.built-$1" ]; }
mark_done() { touch "$prefix/.built-$1"; }

# --- zlib (zlib licence), for the PNG codecs; Windows has none ------------
if [ "$os" = Windows ] && ! done_marker zlib; then
  fetch zlib "https://zlib.net/fossils/zlib-$ZLIB_VERSION.tar.gz"
  echo "==> building zlib"
  (cd "$src/zlib" && make -f win32/Makefile.gcc PREFIX="$host-" libz.a >/dev/null \
      && make -f win32/Makefile.gcc PREFIX="$host-" install SHARED_MODE=0 \
        BINARY_PATH="$prefix/bin" INCLUDE_PATH="$prefix/include" LIBRARY_PATH="$prefix/lib" >/dev/null)
  mkdir -p "$prefix/lib/pkgconfig"
  printf 'prefix=%s\nName: zlib\nDescription: zlib\nVersion: %s\nLibs: -L${prefix}/lib -lz\nCflags: -I${prefix}/include\n' \
    "$prefix" "$ZLIB_VERSION" >"$prefix/lib/pkgconfig/zlib.pc"
  cp "$src/zlib/LICENSE" "$prefix/share/licenses/zlib.txt"
  mark_done zlib
fi

# --- opus (BSD-3) -----------------------------------------------------------
if ! done_marker opus; then
  fetch opus "https://downloads.xiph.org/releases/opus/opus-$OPUS_VERSION.tar.gz"
  echo "==> building opus"
  (cd "$src/opus" && ./configure ${host:+--host=$host} --prefix="$prefix" --disable-shared --enable-static \
      --disable-doc --disable-extra-programs >/dev/null && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/opus/COPYING" "$prefix/share/licenses/opus.txt"
  mark_done opus
fi

# --- libogg and libvorbis (BSD-3) -----------------------------------------
if ! done_marker vorbis; then
  fetch ogg "https://downloads.xiph.org/releases/ogg/libogg-$OGG_VERSION.tar.gz"
  echo "==> building libogg"
  (cd "$src/ogg" && ./configure ${host:+--host=$host} --prefix="$prefix" --disable-shared --enable-static >/dev/null \
      && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/ogg/COPYING" "$prefix/share/licenses/libogg.txt"
  fetch vorbis "https://downloads.xiph.org/releases/vorbis/libvorbis-$VORBIS_VERSION.tar.gz"
  echo "==> building libvorbis"
  # The configure script hands Apple's linker a flag it no longer accepts.
  sed -i.bak 's/-force_cpusubtype_ALL//g' "$src/vorbis/configure"
  (cd "$src/vorbis" && ./configure ${host:+--host=$host} --prefix="$prefix" --disable-shared --enable-static \
      --disable-docs --disable-examples --disable-oggtest >/dev/null \
      && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/vorbis/COPYING" "$prefix/share/licenses/libvorbis.txt"
  mark_done vorbis
fi

# --- LAME (LGPL-2.0+), MP3 encoding ---------------------------------------
if ! done_marker lame; then
  fetch lame "https://downloads.sourceforge.net/project/lame/lame/$LAME_VERSION/lame-$LAME_VERSION.tar.gz"
  echo "==> building lame"
  (cd "$src/lame" && ./configure ${host:+--host=$host} --prefix="$prefix" --disable-shared --enable-static \
      --disable-frontend --disable-decoder --enable-nasm >/dev/null \
      && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/lame/COPYING" "$prefix/share/licenses/lame.txt"
  mark_done lame
fi

# --- libvpx (BSD-3), VP8/VP9 ----------------------------------------------
if ! done_marker libvpx; then
  # googlesource answers archive requests with 503 when busy; GitHub's
  # mirror of the same tag is the fallback (it has a top-level directory).
  fetch libvpx "https://chromium.googlesource.com/webm/libvpx/+archive/refs/tags/v$LIBVPX_VERSION.tar.gz" 0 \
    "https://github.com/webmproject/libvpx/archive/refs/tags/v$LIBVPX_VERSION.tar.gz" 1
  echo "==> building libvpx"
  # libvpx's configure only recognizes Darwin up to 23 (macOS 14); on a
  # newer host it silently falls back to a generic build without NEON or
  # SSE, so the target is spelled out.
  vpx_target="" vpx_cross=""
  if [ "$os" = Windows ]; then
    vpx_target="--target=x86_64-win64-gcc" vpx_cross="$host-"
  elif [ "$os" = Darwin ]; then
    case "$(uname -m)" in
      arm64) vpx_target="--target=arm64-darwin23-gcc" ;;
      x86_64) vpx_target="--target=x86_64-darwin23-gcc" ;;
    esac
  fi
  (cd "$src/libvpx" && CROSS=$vpx_cross ./configure --prefix="$prefix" --disable-shared --enable-static --enable-pic \
      --disable-examples --disable-tools --disable-docs --disable-unit-tests \
      --enable-vp9-highbitdepth --enable-runtime-cpu-detect $vpx_target >/dev/null \
      && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/libvpx/LICENSE" "$prefix/share/licenses/libvpx.txt"
  cat "$src/libvpx/PATENTS" >> "$prefix/share/licenses/libvpx.txt"
  mark_done libvpx
fi

# --- dav1d (BSD-2), AV1 decoding ------------------------------------------
if ! done_marker dav1d; then
  # From VideoLAN's mirror on GitHub first: code.videolan.org sits behind
  # a bot check (Anubis) that answers scripted downloads with a challenge
  # page some of the time. The same tag there is the fallback.
  fetch dav1d "https://github.com/videolan/dav1d/archive/refs/tags/$DAV1D_VERSION.tar.gz" 1 \
    "https://code.videolan.org/videolan/dav1d/-/archive/$DAV1D_VERSION/dav1d-$DAV1D_VERSION.tar.gz" 1
  echo "==> building dav1d"
  (cd "$src/dav1d" && rm -rf build && meson setup build ${meson_cross:+--cross-file="$meson_cross"} --prefix="$prefix" --libdir=lib --buildtype=release \
      --default-library=static -Denable_tools=false -Denable_tests=false >/dev/null \
      && ninja -C build >/dev/null && ninja -C build install >/dev/null)
  cp "$src/dav1d/COPYING" "$prefix/share/licenses/dav1d.txt"
  mark_done dav1d
fi

# --- SVT-AV1 (BSD-2 + patent grant), AV1 encoding --------------------------
if ! done_marker svtav1; then
  fetch svtav1 "https://gitlab.com/AOMediaCodec/SVT-AV1/-/archive/v$SVTAV1_VERSION/SVT-AV1-v$SVTAV1_VERSION.tar.gz"
  echo "==> building SVT-AV1"
  (cd "$src/svtav1" && rm -rf build && cmake -S . -B build -G Ninja ${cmake_toolchain:+-DCMAKE_TOOLCHAIN_FILE="$cmake_toolchain"} -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF \
      -DBUILD_DEC=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DSVT_AV1_LTO=OFF \
      -DCMAKE_POLICY_VERSION_MINIMUM=3.5 >/dev/null \
      && ninja -C build >/dev/null && ninja -C build install >/dev/null)
  cp "$src/svtav1/LICENSE.md" "$prefix/share/licenses/svt-av1.txt"
  cat "$src/svtav1/PATENTS.md" >> "$prefix/share/licenses/svt-av1.txt"
  mark_done svtav1
fi

# --- OpenH264 (BSD-2), H.264 encoding --------------------------------------
if ! done_marker openh264; then
  if [ ! -f "$src/openh264/.fetched" ]; then
    echo "==> fetching openh264"
    rm -rf "$src/openh264"
    git clone -q --depth 1 --branch "v$OPENH264_VERSION" https://github.com/cisco/openh264 "$src/openh264"
    touch "$src/openh264/.fetched"
  fi
  echo "==> building openh264"
  (cd "$src/openh264" && make -j"$jobs" $openh264_make PREFIX="$prefix" libraries >/dev/null \
      && make $openh264_make PREFIX="$prefix" install-static >/dev/null)
  cp "$src/openh264/LICENSE" "$prefix/share/licenses/openh264.txt"
  mark_done openh264
fi

# --- NVIDIA encoder headers (MIT); the driver is loaded at run time ---------
if { [ "$os" = Linux ] || [ "$os" = Windows ]; } && ! done_marker nvheaders; then
  if [ ! -f "$src/nv-codec-headers/.fetched" ]; then
    echo "==> fetching nv-codec-headers"
    rm -rf "$src/nv-codec-headers"
    git clone -q --depth 1 --branch "n$NVCODEC_VERSION" https://github.com/FFmpeg/nv-codec-headers "$src/nv-codec-headers"
    touch "$src/nv-codec-headers/.fetched"
  fi
  (cd "$src/nv-codec-headers" && make PREFIX="$prefix" install >/dev/null)
  mark_done nvheaders
fi

# --- core media libraries (LGPL-2.1+), trimmed ----------------------------
if ! done_marker ffmpeg; then
  fetch ffmpeg "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
  echo "==> building media libraries"
  extra=""
  case "$os" in
    Darwin) extra="--enable-videotoolbox --enable-audiotoolbox --enable-encoder=h264_videotoolbox,hevc_videotoolbox,aac_at" ;;
    Linux) extra="--enable-ffnvcodec --enable-nvenc --enable-encoder=h264_nvenc,hevc_nvenc" ;;
    # Media Foundation is the system's own encoder, on the GPU where there
    # is one; NVENC loads the driver's DLL at run time like on Linux.
    Windows) extra="--enable-cross-compile --target-os=mingw32 --arch=x86_64 --cross-prefix=$host- --pkg-config=pkg-config
      --enable-mediafoundation --enable-ffnvcodec --enable-nvenc --enable-encoder=h264_mf,hevc_mf,h264_nvenc,hevc_nvenc" ;;
  esac
  (cd "$src/ffmpeg" && ./configure --prefix="$prefix" --pkg-config-flags=--static \
      --enable-static --disable-shared --enable-pic \
      --disable-everything --disable-programs --disable-doc --disable-network \
      --disable-avdevice --disable-avfilter --disable-postproc --disable-debug \
      --disable-autodetect --disable-iconv --disable-sdl2 --disable-xlib --enable-zlib \
      --enable-libopenh264 --enable-libvpx --enable-libsvtav1 --enable-libdav1d --enable-libopus \
      --enable-libvorbis --enable-libmp3lame \
      --extra-cflags="-I$prefix/include" --extra-ldflags="-L$prefix/lib" \
      --enable-protocol=file,pipe \
      --enable-demuxer=mov,matroska,mp3,wav,aac,flac,ogg,image2,mpegts,avi,gif,mxf,srt,webvtt \
      --enable-muxer=mp4,mov,matroska,webm,wav,flac,ogg,opus,adts,image2,mxf,mp3,srt,webvtt \
      --enable-decoder=h264,hevc,vp8,vp9,libdav1d,mpeg4,mpeg2video,mjpeg,png,prores,dnxhd,rawvideo,gif \
      --enable-decoder=aac,mp3,flac,vorbis,libopus,alac,ac3,eac3,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,pcm_s16be,pcm_u8 \
      --enable-decoder=subrip,webvtt,mov_text \
      --enable-encoder=libopenh264,libvpx_vp9,libsvtav1,prores_ks,dnxhd,png,mjpeg,rawvideo \
      --enable-encoder=aac,libopus,flac,alac,ac3,libmp3lame,libvorbis,pcm_s16le,pcm_s24le,pcm_f32le \
      --enable-parser=h264,hevc,vp8,vp9,av1,aac,mpeg4video,mpegvideo,mjpeg,png,flac,vorbis,opus,mpegaudio,ac3,dnxhd \
      --enable-bsf=h264_mp4toannexb,hevc_mp4toannexb,aac_adtstoasc,extract_extradata,vp9_superframe,null \
      $extra >"$src/ffmpeg-configure.log" 2>&1 || { tail -30 "$src/ffmpeg-configure.log"; exit 1; }
   make -j"$jobs" >"$src/ffmpeg-build.log" 2>&1 || { tail -30 "$src/ffmpeg-build.log"; exit 1; }
   make install >/dev/null)
  # The binding crate's bindgen wrapper includes this header when it builds
  # for Windows, and MinGW-w64 has no d3d12video.h for it to include in
  # turn. Nothing here uses D3D12 video.
  rm -f "$prefix/include/libavutil/hwcontext_d3d12va.h"
  cp "$src/ffmpeg/COPYING.LGPLv2.1" "$prefix/share/licenses/ffmpeg-libraries.txt"
  mark_done ffmpeg
fi

echo
echo "media libraries installed under $prefix"
echo "build with: cargo build --release"
echo "(after rebuilding the libraries, run: cargo clean -p ffmpeg-sys-next, whose"
echo " compiled form bundles the archives it was first built against)"
