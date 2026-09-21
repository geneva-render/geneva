#!/usr/bin/env bash
# Runs geneva's test suite on a rented Linux box with a GPU, and prints one
# report. Written for a throwaway machine: it installs what it needs, builds
# everything from a clone, and leaves nothing worth keeping behind.
#
# Usage: gpu-box.sh [--ref REF] [--quick]
#   --ref REF   the branch or tag to test; main by default. Ignored when
#               GENEVA_ARCHIVE names a tarball of the tree, which is how a
#               machine with no credentials for the repository gets it
#   --quick     skip the media libraries, the suite and check.sh, and report
#               only what the machine is and whether Vulkan works
#
# It is safe to run again: the machine setup, the clone and the media
# libraries are each done once, so a second run goes straight to the tests.
#
# The point of renting is the parts CI cannot reach. Continuous integration
# has only Mesa's lavapipe, a software device, so the goldens have never run
# against a vendor driver. This checks three things in order: that a real
# device agrees with the CPU renderer pixel by pixel, that the full frame
# sweep of the opening holds (it is marked ignored because it takes minutes
# on a software device and has therefore never run at all), and what the GPU
# path actually costs against the CPU one.
set -u

ref=main
quick=0
while [ $# -gt 0 ]; do
  case "$1" in
    --ref) ref=$2; shift 2 ;;
    --quick) quick=1; shift ;;
    -h | --help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "gpu-box.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

work=${GENEVA_WORK:-/root/work}
repo=$work/geneva
mkdir -p "$work/.done"
export DEBIAN_FRONTEND=noninteractive
export CARGO_TERM_COLOR=never
started=$(date +%s)

say() { echo; echo "=== $* ($(($(date +%s) - started))s) ==="; }
once() { [ -f "$work/.done/$1" ]; }
did() { touch "$work/.done/$1"; }

say "machine"
uname -srm
echo "cpus: $(nproc)"
free -g 2>/dev/null | awk '/^Mem:/ {print "ram: " $2 " GB"}'
df -h "$work" | awk 'NR==2 {print "disk: " $4 " free of " $2}'

say "gpu"
nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader 2>&1 || echo "no nvidia-smi"

if ! once apt-vulkan; then
  say "installing vulkan tools"
  apt-get update -qq && apt-get install -y -qq vulkan-tools libvulkan1 curl ca-certificates >/dev/null 2>&1
  did apt-vulkan
fi

say "vulkan"
summary=$(vulkaninfo --summary 2>&1)
echo "$summary" | sed -n '/Devices:/,/^$/p' | head -20
if ! echo "$summary" | grep -q "deviceName"; then
  echo "NO VULKAN DEVICE. This box cannot answer the question it was rented for."
  echo "--- loader error"
  echo "$summary" | head -20
  echo "--- icds"
  ls -l /usr/share/vulkan/icd.d/ 2>&1
  echo "--- driver libraries"
  ldconfig -p | grep -iE "vulkan|GLX_nvidia|nvidia-glvkspirv" | head
  echo "--- capabilities"
  echo "NVIDIA_DRIVER_CAPABILITIES=${NVIDIA_DRIVER_CAPABILITIES:-unset}"
  ls /dev/dri 2>&1
  exit 1
fi
[ "$quick" = 1 ] && { say "quick run, stopping here"; exit 0; }

if ! once apt-build; then
  say "installing the build tools"
  apt-get install -y -qq build-essential cmake meson ninja-build nasm pkg-config \
    clang zlib1g-dev git curl ffmpeg >/dev/null 2>&1 || {
    echo "apt failed"; exit 1; }
  did apt-build
fi

if ! once rustup; then
  say "installing rust"
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable >/dev/null
  did rustup
fi
export PATH="$HOME/.cargo/bin:$PATH"
rustc --version

if ! once source; then
  if [ -n "${GENEVA_ARCHIVE:-}" ] && [ -f "$GENEVA_ARCHIVE" ]; then
    say "unpacking $GENEVA_ARCHIVE"
    rm -rf "$repo"; mkdir -p "$repo"
    tar xzf "$GENEVA_ARCHIVE" -C "$repo" || exit 1
    did source
  else
    say "cloning $ref"
    rm -rf "$repo"
    git clone --depth 1 --branch "$ref" https://github.com/geneva-render/geneva.git "$repo" 2>&1 | tail -2
    did source
  fi
fi
cd "$repo" || exit 1
if [ -d .git ]; then git log --oneline -1; else echo "source from an archive, no history"; fi

if ! once media-libs; then
  say "building the media libraries"
  if scripts/build-media-libs.sh > "$work/media-libs.log" 2>&1; then
    did media-libs
    echo "built, $(du -sh target/media-libs | cut -f1)"
  else
    echo "the media libraries failed to build; last 30 lines:"
    tail -30 "$work/media-libs.log"
    exit 1
  fi
fi

say "building the tests"
cargo test --workspace --features geneva-cli/media --no-run > "$work/build.log" 2>&1 || {
  echo "the build failed; last 40 lines:"; tail -40 "$work/build.log"; exit 1; }
echo "built"

say "the suite"
cargo test --workspace --features geneva-cli/media > "$work/test.log" 2>&1
suite=$?
echo "binaries passing: $(grep -c '^test result: ok' "$work/test.log")  (38 expected)"
grep -E "^test result: FAILED|^failures:" -A 12 "$work/test.log" | head -40
echo "exit $suite"

say "the frame sweep that has never run on hardware"
cargo test -p geneva-gpu --test goldens -- --ignored --nocapture \
  > "$work/sweep.log" 2>&1
sweep=$?
tail -25 "$work/sweep.log"
echo "exit $sweep"

say "the two renderers, pixel by pixel"
cargo test -p geneva-gpu --test goldens -- --nocapture \
  > "$work/compare.log" 2>&1
compare=$?
grep -vE "^\s*$" "$work/compare.log" | tail -30
echo "exit $compare"

say "check.sh"
cargo build --release --features geneva-cli/media > "$work/release.log" 2>&1 || {
  echo "the release build failed"; tail -20 "$work/release.log"; }
scripts/check.sh 2>&1 | tail -45

say "cpu against gpu on a real job"
# Two runs each: the first pays for page cache and any driver warm-up.
for r in cpu gpu; do
  for n in 1 2; do
    a=$(date +%s%N)
    ./target/release/geneva render examples/opening.json -o "$work/out-$r-$n.mp4" \
      --renderer "$r" > "$work/render-$r-$n.log" 2>&1
    b=$(date +%s%N)
    ms=$(( (b - a) / 1000000 ))
    echo "$r run $n: $((ms / 1000)).$(printf %03d $((ms % 1000)))s"
  done
  grep -iE "device|renderer|note" "$work/render-$r-2.log" | head -4
done

say "failures kept"
for d in target/golden-failures target/golden-failures-gpu; do
  if [ -d "$d" ] && [ -n "$(ls -A "$d" 2>/dev/null)" ]; then
    cp -r "$d" "$work/" && echo "$d has $(find "$d" -type f | wc -l) files, copied to $work"
  fi
done
ls "$work"

say "done in $(( ($(date +%s) - started) / 60 )) minutes"
