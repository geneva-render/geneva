# Spikes

Throwaway code that answers one question about how the engine should be
built. Nothing here is part of the engine: no spike is a member of the
workspace, none is published, and the gate in the working agreement does
not build any of them. A spike is kept only while the question it
answers is still open.

Each spike has a `[workspace]` of its own in its `Cargo.toml`, so cargo
treats it as a separate project that happens to sit in this tree and
depend on the crates beside it by path. Build one from its own
directory:

```sh
cd spikes/vello-painter && cargo build --release
```

## vello-painter

Whether the markup painter should be rewritten on `vello`: one scene
translation, rasterized by `vello_cpu` on the processor and by the
sparse-strips GPU renderer on the device, in place of the painter in
`geneva-render`. It translates the markup display list to a `vello_cpu`
scene, draws documents both ways, and compares them with the same
comparator the golden tests use.

```sh
cargo run --release -- emit   <dir>   # writes the scenes and their timelines
geneva frame <dir>/<name>.json --at 0 -o <dir>/ours-<name>.png
cargo run --release -- render <dir>   # draws them through vello and compares
cargo run --release -- glyphs <dir>   # one glyph through swash and through glifo
```
