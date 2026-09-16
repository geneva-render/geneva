# Working on geneva

## Branch

Commit to `main`. It is the default branch and carries the whole history.

## Writing

This applies to every word that ships: the README, `docs/`, `examples/`,
the changelog, Rust doc comments, and the text of diagnostics.

**No em-dashes.** Not in prose, not in tables, not in doc comments. If a
sentence wants one, it wants restructuring: use a full stop, a colon, a
semicolon, or brackets. Do not swap the character for a hyphen and leave
the sentence as it was.

**ASCII quotes and apostrophes.** `'` and `"`, not `’` `‘` `”` `“`. Web
editors substitute the curly forms silently, so check after editing
outside the repository. The typographic ellipsis `…` is fine where it
stands for omitted content.

**Matter of fact.** State what the thing does and what it does not. No
salesmanship, no "real" or "properly" or "with no browser anywhere", no
punchlines, no telling the reader what to be impressed by. If a number
makes the point, give the number.

**Say what is not true too.** Where a feature has a limit, a silent
difference or a known gap, write it down next to the feature rather than
leaving the reader to find it.

Keep the README short. Long-form examples belong in `examples/README.md`
with their document, command, figure and output; reference material
belongs in `docs/`. Link to them instead of inlining.

## Before committing

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --features geneva-cli/media -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace --features geneva-cli/media
cargo test --workspace --no-default-features
```

Both test runs should report 28 passing binaries. Regenerate the JSON
schema with `GENEVA_UPDATE_SCHEMA=1 cargo test -p geneva-timeline` after
any change to the format types.

Do not put a model name or identifier in a commit message, a PR, a code
comment or anything else that lands in the repository.

## Releases

Tagging runs GitHub Actions minutes, so do not tag or cut a release
unless asked.
