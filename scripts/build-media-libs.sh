#!/usr/bin/env bash
# Builds the media libraries Geneva links statically, from pinned sources,
# with only the components Geneva uses. Everything is LGPL or BSD licensed.
#
# Usage: scripts/build-media-libs.sh [prefix]
#   prefix defaults to target/media-libs (absolute path is printed at the end)
#
# Needs: a C/C++ compiler, make, cmake, meson, ninja, nasm, pkg-config, curl,
# git. On Debian/Ubuntu: build-essential cmake meson ninja-build nasm
# pkg-config curl git. On macOS: xcode command line tools plus
# `brew install cmake meson ninja nasm pkg-config`.
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

root=$(cd "$(dirname "$0")/.." && pwd)
prefix_arg=${1:-$root/target/media-libs}
mkdir -p "$prefix_arg"
prefix=$(cd "$prefix_arg" && pwd)
src=$root/target/media-src
jobs=${JOBS:-$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)}
mkdir -p "$prefix" "$src" "$prefix/share/licenses"
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig"
export CFLAGS="${CFLAGS:-} -fPIC -O2"
export CXXFLAGS="${CXXFLAGS:-} -fPIC -O2"

fetch() { # name url [strip-components, default 1]
  local name=$1 url=$2 strip=${3:-1} dir=$src/$1
  # A source tree counts only when its marker says the unpack finished;
  # a directory alone may be the remains of an interrupted or pruned one.
  if [ -f "$dir/.fetched" ]; then return; fi
  echo "==> fetching $name"
  rm -rf "$dir"
  curl -sSL --retry 5 --retry-all-errors --retry-delay 3 -o "$src/$name.tar" "$url"
  mkdir -p "$dir"
  tar xf "$src/$name.tar" --strip-components="$strip" -C "$dir"
  rm -f "$src/$name.tar"
  touch "$dir/.fetched"
}

done_marker() { [ -f "$prefix/.built-$1" ]; }
mark_done() { touch "$prefix/.built-$1"; }

# --- opus (BSD-3) -----------------------------------------------------------
if ! done_marker opus; then
  fetch opus "https://downloads.xiph.org/releases/opus/opus-$OPUS_VERSION.tar.gz"
  echo "==> building opus"
  (cd "$src/opus" && ./configure --prefix="$prefix" --disable-shared --enable-static \
      --disable-doc --disable-extra-programs >/dev/null && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/opus/COPYING" "$prefix/share/licenses/opus.txt"
  mark_done opus
fi

# --- libogg and libvorbis (BSD-3) -----------------------------------------
if ! done_marker vorbis; then
  fetch ogg "https://downloads.xiph.org/releases/ogg/libogg-$OGG_VERSION.tar.gz"
  echo "==> building libogg"
  (cd "$src/ogg" && ./configure --prefix="$prefix" --disable-shared --enable-static >/dev/null \
      && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/ogg/COPYING" "$prefix/share/licenses/libogg.txt"
  fetch vorbis "https://downloads.xiph.org/releases/vorbis/libvorbis-$VORBIS_VERSION.tar.gz"
  echo "==> building libvorbis"
  # The configure script hands Apple's linker a flag it no longer accepts.
  sed -i.bak 's/-force_cpusubtype_ALL//g' "$src/vorbis/configure"
  (cd "$src/vorbis" && ./configure --prefix="$prefix" --disable-shared --enable-static \
      --disable-docs --disable-examples --disable-oggtest >/dev/null \
      && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/vorbis/COPYING" "$prefix/share/licenses/libvorbis.txt"
  mark_done vorbis
fi

# --- LAME (LGPL-2.0+), MP3 encoding ---------------------------------------
if ! done_marker lame; then
  fetch lame "https://downloads.sourceforge.net/project/lame/lame/$LAME_VERSION/lame-$LAME_VERSION.tar.gz"
  echo "==> building lame"
  (cd "$src/lame" && ./configure --prefix="$prefix" --disable-shared --enable-static \
      --disable-frontend --disable-decoder --enable-nasm >/dev/null \
      && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/lame/COPYING" "$prefix/share/licenses/lame.txt"
  mark_done lame
fi

# --- libvpx (BSD-3), VP8/VP9 ----------------------------------------------
if ! done_marker libvpx; then
  fetch libvpx "https://chromium.googlesource.com/webm/libvpx/+archive/refs/tags/v$LIBVPX_VERSION.tar.gz" 0
  echo "==> building libvpx"
  # libvpx's configure only recognizes Darwin up to 23 (macOS 14); on a
  # newer host it silently falls back to a generic build without NEON or
  # SSE, so the target is spelled out.
  vpx_target=""
  if [ "$(uname -s)" = Darwin ]; then
    case "$(uname -m)" in
      arm64) vpx_target="--target=arm64-darwin23-gcc" ;;
      x86_64) vpx_target="--target=x86_64-darwin23-gcc" ;;
    esac
  fi
  (cd "$src/libvpx" && ./configure --prefix="$prefix" --disable-shared --enable-static --enable-pic \
      --disable-examples --disable-tools --disable-docs --disable-unit-tests \
      --enable-vp9-highbitdepth --enable-runtime-cpu-detect $vpx_target >/dev/null \
      && make -j"$jobs" >/dev/null && make install >/dev/null)
  cp "$src/libvpx/LICENSE" "$prefix/share/licenses/libvpx.txt"
  cat "$src/libvpx/PATENTS" >> "$prefix/share/licenses/libvpx.txt"
  mark_done libvpx
fi

# --- dav1d (BSD-2), AV1 decoding ------------------------------------------
if ! done_marker dav1d; then
  fetch dav1d "https://code.videolan.org/videolan/dav1d/-/archive/$DAV1D_VERSION/dav1d-$DAV1D_VERSION.tar.gz"
  echo "==> building dav1d"
  (cd "$src/dav1d" && rm -rf build && meson setup build --prefix="$prefix" --libdir=lib --buildtype=release \
      --default-library=static -Denable_tools=false -Denable_tests=false >/dev/null \
      && ninja -C build >/dev/null && ninja -C build install >/dev/null)
  cp "$src/dav1d/COPYING" "$prefix/share/licenses/dav1d.txt"
  mark_done dav1d
fi

# --- SVT-AV1 (BSD-2 + patent grant), AV1 encoding --------------------------
if ! done_marker svtav1; then
  fetch svtav1 "https://gitlab.com/AOMediaCodec/SVT-AV1/-/archive/v$SVTAV1_VERSION/SVT-AV1-v$SVTAV1_VERSION.tar.gz"
  echo "==> building SVT-AV1"
  (cd "$src/svtav1" && rm -rf build && cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
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
  (cd "$src/openh264" && make -j"$jobs" PREFIX="$prefix" libraries >/dev/null \
      && make PREFIX="$prefix" install-static >/dev/null)
  cp "$src/openh264/LICENSE" "$prefix/share/licenses/openh264.txt"
  mark_done openh264
fi

# --- NVIDIA encoder headers (MIT); the driver is loaded at run time ---------
if [ "$(uname -s)" = Linux ] && ! done_marker nvheaders; then
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
  case "$(uname -s)" in
    Darwin) extra="--enable-videotoolbox --enable-audiotoolbox --enable-encoder=h264_videotoolbox,hevc_videotoolbox,aac_at" ;;
    Linux) extra="--enable-ffnvcodec --enable-nvenc --enable-encoder=h264_nvenc,hevc_nvenc" ;;
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
  cp "$src/ffmpeg/COPYING.LGPLv2.1" "$prefix/share/licenses/ffmpeg-libraries.txt"
  mark_done ffmpeg
fi

echo
echo "media libraries installed under $prefix"
echo "build with: cargo build --release"
echo "(after rebuilding the libraries, run: cargo clean -p ffmpeg-sys-next, whose"
echo " compiled form bundles the archives it was first built against)"
