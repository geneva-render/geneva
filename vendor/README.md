# Vendored crates

## harfrust 0.5.2

The OpenType shaper cosmic-text 0.19 uses, from crates.io, with one
function changed: `WouldApply for SequenceContextFormat3` in
`src/hb/ot/contextual.rs`.

A format-3 context rule lists a coverage for every input glyph, the first
included. 0.5.2 matched it as formats 1 and 2 are matched (one glyph more
than coverages, each glyph against the coverage before it), so a test of
whether such a rule would apply always failed. The Indic shapers ask that
question to place consonants: a font whose below-base forms are format-3
rules (Noto Sans Bengali's `blwf`, "virama then ব or র") had its
ra-phala and ba-phala drawn as a visible hasant and a full letter. The
fix matches the sequence as HarfBuzz does and as harfrust 0.14 has it.

Remove this directory and the `[patch.crates-io]` entry in the workspace
`Cargo.toml` once cosmic-text depends on a harfrust with the fix.
