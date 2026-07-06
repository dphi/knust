# Contributing to knust

Thanks for considering a contribution. This project is a young, unpublished
KNX/IP library, so the API is still allowed to change — prefer a clean
breaking change over a backwards-compatibility shim.

## Getting started

```bash
git clone <repo-url>
cd knust
cargo build --all-features
cargo test --all-features
```

The crate is feature-gated (see the [README](README.md#cargo-features)):

- `dpt` (default) — datapoint encode/decode
- `ets` — ETS CSV group-address import
- `secure` — KNX IP Secure / Data Security / keyring parsing
- `server` — KNXnet/IP tunneling server role

If your change only touches one feature area, it's still worth running with
`--all-features` locally before opening a PR — CI does, and some bugs only
surface when features combine (e.g. `secure` implies `ets`).

## Before opening a PR

Run the same checks CI runs, in this order:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings -W clippy::pedantic
cargo test --all-features
```

All three must be clean. The clippy invocation runs `clippy::pedantic` as a
warn-level lint promoted by `-D warnings`, not a suggestion — fix the
lint, don't `#[allow]` it, unless you're adding a *new* crate-level allow to
`src/lib.rs` with a one-line rationale comment explaining why the lint
doesn't apply here.

If you add or remove a `#[cfg(feature = ...)]` boundary, also check the
feature matrix compiles:

```bash
cargo install cargo-hack --locked
cargo hack check --each-feature --no-dev-deps
```

CI additionally gates PRs on **coverage of the lines you actually changed**
(via `cargo-llvm-cov` + `diff-cover`, currently 80%) — not overall project
coverage. Non-trivial logic (a new branch, parser, or protocol state
transition) needs a test; a one-line fix usually doesn't.

## Commit messages

This repo uses [Conventional Commits](https://www.conventionalcommits.org/):
`type: short summary`, e.g. `fix: handle empty group address list`. Common
types used here: `feat`, `fix`, `docs`, `refactor`, `style` (formatting/lint
cleanup, no behavior change), `test`, `chore`, `ci`. Add `!` after the type
(`refactor!: ...`) for a breaking API change, and explain the break in the
body.

Explain *why*, not just *what* — the diff already shows what changed.

## Security-sensitive code

`src/security/` and the `secure` feature implement KNX IP Secure and KNX
Data Security. Two things to know before touching this code:

- KNX IP Secure (the session handshake in `transport::tunnel`) is verified
  against real hardware. KNX Data Security (group encryption,
  `security::group`) is **experimental** and has not been checked against a
  reference implementation or real Data-Secure device — say so in your PR
  if a change touches it, and don't remove that caveat from the docs without
  actually doing that verification.
- Don't silently downgrade security. If a code path is supposed to
  authenticate or encrypt and can't (missing config, disabled feature,
  unimplemented mode), it must return an error, not fall back to plaintext.

## Tests

- Property-based tests (`proptest`) are used throughout for protocol
  encode/decode round-trips — follow that pattern for new wire-format code
  rather than only hand-picked examples.
- Don't mock around the actual parsing/crypto code; test it directly.
- `examples/*_smoke.rs` are manual, real-hardware verification tools (they
  take a gateway address as an argument) — they're not part of `cargo test`
  and won't run in CI.

## Dependencies

Don't add a new dependency for something a few lines of `std` can do. If a
new dependency is genuinely needed, say why in the PR description; expect
that to be scrutinized more than the rest of the diff (`cargo audit` also
runs in CI on every push).

## License

By contributing, you agree your contribution is licensed under this
project's [MIT license](LICENSE).
