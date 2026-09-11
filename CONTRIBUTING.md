# Contributing

Thanks for your interest in Geneva. The project is at an early stage, so the
most useful contributions right now are bug reports with reproducible
timelines, sample files that render incorrectly, and review of the timeline
format.

## Certifying your contributions

Geneva is MIT licensed and contributions are accepted under the same
license. Instead of a contributor agreement, every commit carries a
Developer Certificate of Origin sign-off stating that you have the right to
submit it:

```sh
git commit -s
```

adds a `Signed-off-by: Your Name <you@example.com>` line; see
<https://developercertificate.org> for the text you are certifying.

## Development

The media libraries are built once from pinned sources into
`target/media-libs` and linked statically. Install the build tools, run the
script, then use cargo as usual:

```sh
# Debian/Ubuntu
sudo apt install build-essential cmake meson ninja-build nasm pkg-config clang zlib1g-dev git curl
# macOS
brew install cmake meson ninja nasm pkg-config

scripts/build-media-libs.sh        # ~10-20 minutes, once; cached in CI
                                   # after a rebuild: cargo clean -p ffmpeg-sys-next
cargo build
```

`clang` is only used while building, to generate bindings. To work on the
format, validator or renderer without media support at all, skip the
script and use `--no-default-features`.

Before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Golden-frame tests live under `tests/golden/`. To regenerate reference frames
after an intentional rendering change:

```sh
GENEVA_UPDATE_GOLDEN=1 cargo test -p geneva-golden
```

The A/V sync corpus under `tests/media/sync/` is built by
`scripts/make-sync-corpus.sh` (needs ffmpeg) and committed; rebuild it only
when the scene or the traps change, and commit the files with the script.

Commit the updated PNGs together with the change that motivated them, and
describe the visual difference in the pull request.

## Ground rules

- No `unsafe` code outside clearly bounded FFI crates.
- Every diagnostic has a stable code, a JSON path, and a suggested fix.
- Rendering must be deterministic: no wall clocks, no thread-order dependent
  output, no uninitialized memory.
- Keep comments technical and local to the code they explain.
