# Contributing

Thanks for helping. This crate talks to a government portal with people's real
accounts, so a few rules matter more than usual.

## Ground rules

- **Never commit real data.** No real NIFs, passwords, SMS codes, session cookies,
  invoice ids, names or phone numbers, in code, tests, fixtures, issues or PRs. Use
  obvious placeholders (`123456789`, `000000000001`, `*******321`).
- **Tests never reach the AT.** The suite runs against `wiremock` and the HTML
  fixtures in `tests/fixtures/`. If you capture a new page from the real portal,
  strip everything personal and anything not needed by the parser first.
- **Don't test against other people's accounts.** Wrong passwords and SMS codes
  count towards an account lockout.
- **Never retry authentication failures automatically.** `Error::is_auth_failure()`
  errors must reach the caller untouched.

## Before opening a PR

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same, plus `cargo doc`, `cargo audit` and a build on the minimum
supported Rust version (`rust-version` in `Cargo.toml`).

## License

By submitting a contribution you agree to license it under the project's
[MIT-0](LICENSE) license.

## Reporting a portal change

If the AT changed their site and something broke, run with
`RUST_LOG=efatura=debug`, look for `UnexpectedResponse`, and describe which step
failed (without pasting cookies or personal data).
