# Contributing

Thanks for your interest in Geneva. The project is at an early stage, so the
most useful contributions right now are bug reports with reproducible
timelines, sample files that render incorrectly, and review of the timeline
format.

## Contributor License Agreement

Geneva is source-available and the licensor offers commercial licenses. To
keep that possible, all code contributions require a signed Contributor
License Agreement (CLA) granting the licensor the rights needed to distribute
the contribution under both the Geneva License and commercial terms. The
CLA text and signing flow will be linked from the first pull request that
needs it; a bot will guide you.

## Development

The media libraries are built once from pinned sources into
`target/media-libs` and linked statically. Install the build tools, run the
script, then use cargo as usual:

```sh
# Debian/Ubuntu
sudo apt install build-essential cmake meson ninja-build nasm pkg-config clang git curl
# macOS
brew install cmake meson ninja nasm pkg-config

scripts/build-media-libs.sh        # ~10-20 minutes, once; cached in CI
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

Commit the updated PNGs together with the change that motivated them, and
describe the visual difference in the pull request.

## Ground rules

- No `unsafe` code outside clearly bounded FFI crates.
- Every diagnostic has a stable code, a JSON path, and a suggested fix.
- Rendering must be deterministic: no wall clocks, no thread-order dependent
  output, no uninitialized memory.
- Keep comments technical and local to the code they explain.
