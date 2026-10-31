# Development

The checks every change must pass, and how the tests and releases work. Back to the [README](../../README.md).

Design decisions are in [`doc/`](../README.md); read the relevant one before changing
behaviour. Every change must pass:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release --target x86_64-unknown-linux-musl
```

CI runs the same checks on every push to `main` and on pull requests; a pushed `vX.Y.Z` tag
builds and publishes a release (checklist in [0012](../0012-build-and-distribution.md)).

The tests need no Claude Code account: they use sanitized fixtures under `tests/fixtures/`
and a fake `claude` placed on `PATH` ([0015](../0015-testing-strategy.md)). Nothing
committed may contain personal details ([0013](../0013-privacy-and-repository-hygiene.md)).
