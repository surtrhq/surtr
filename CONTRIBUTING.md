# Contributing to Surtr

Thanks for helping. Surtr is early in development, so the most useful
contributions right now are bug reports and hardware reports from machines we
don't have, followed by fixes to the sensor backends.

## Reporting a bug or missing hardware

Open an issue with the matching form. Both ask for the output of
`surtr diag --summary` (a short Markdown block with versions, providers,
health and recent warnings, and no serial numbers, addresses or user names).
For a deeper look we may ask for the full bundle: `surtr diag` shows what it
contains and asks before writing a zip that you can attach.

Security problems go through private vulnerability reporting instead; see
[SECURITY.md](SECURITY.md).

## Building and testing

You need a stable Rust toolchain (edition 2024).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

CI runs the same steps on Linux, Windows and macOS (x86-64 and ARM64), plus
`cargo deny check` for licenses and advisories. `--mock` runs any command
against a deterministic fake machine, which is what most tests use.

## Pull requests

- Keep a pull request to one change, with tests where the behavior can be
  tested. Front-end output is covered by snapshot tests (`insta`).
- New dependencies must be under a license that `deny.toml` allows.
- Surtr never bundles third-party binaries (drivers, vendor libraries); vendor
  SDKs are loaded at run time from the user's install.
- Code that could change hardware settings needs a design discussion in an
  issue first.

### Sign-off (DCO)

Sign off every commit with `git commit -s`. The `Signed-off-by` line states
that you wrote the change or have the right to submit it under the project's
license, as described in the [Developer Certificate of
Origin](https://developercertificate.org/).

## License

Unless you say otherwise, contributions are licensed under the same terms as
the project: Apache-2.0 OR MIT, at the user's option.

## Conduct

Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).
