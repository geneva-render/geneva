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

On a machine that cannot build the media libraries, `scripts/fetch-build.sh`
takes a build from GitHub Actions instead. It needs `gh`, logged in:

```sh
scripts/fetch-build.sh            # newest finished build of main
scripts/fetch-build.sh --rerun    # build the current main first, then take it
```

It downloads the artifact for the machine it runs on, unpacks it and clears
the quarantine flag, leaving the binary and `check.sh` in `geneva-build/`.
Downloading works for anyone; `--rerun` dispatches the workflow, which needs
write access to the repository.

`scripts/check-markup.sh` then checks that machine's markup rendering
against the committed references without a toolchain: it renders the frames
of the two markup golden cases with the binary and compares them with
`tests/golden/*/expected/`, byte for byte. Font matching and coverage
arithmetic have differed between machines before, and this is what catches
that.

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
