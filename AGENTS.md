# AGENTS.md

## Project overview

`efatura` is an unofficial Rust client for Portugal's e-fatura portal
(<https://faturas.portaldasfinancas.gov.pt>). It logs in *as the user* through
acesso.gov.pt (NIF + password + SMS second factor, resumable across processes), lists
and reads the invoices where the user is the buyer, and assigns IRS sectors. The AT has
no public API, so everything is reverse-engineered and can break when their site changes.

Rust 2024 edition, MSRV 1.88 (`rust-version` in `Cargo.toml`, enforced in CI), async with
`reqwest`, HTML parsing with `scraper`/`regex`, cookies with `cookie_store`.

## Commands

```bash
cargo fmt                                       # format (checked in CI)
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked                             # unit + wiremock flow tests + doctests
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo audit                                     # RustSec advisories (CI runs it)
cargo publish --dry-run --locked                # what would be uploaded (CI runs it)
```

CI runs all of these. The tests never reach the real portal. The examples do, so only
run them with your own account (see below).

## Structure

```
src/
  lib.rs            Crate docs (compile-checked examples) and the public re-exports
  auth/mod.rs       Authenticator, Credentials, LoginOutcome, PendingLogin(+State): the login flow
  auth/login_page.rs  Parser for the login app's server-rendered state (regex over its JS model)
  client.rs         Client: documents, document (detail), set_sector, categorize
  session.rs        Cookie jar, SessionState and the HTTP wrapper around reqwest
  model.rs          DocumentId, Document, DocumentDetail, DocumentQuery, Sector, DocumentState
  config.rs         Config (portal URLs, user agent, timeout): tests point it at wiremock
  error.rs          Error (non_exhaustive), is_auth_failure(), needs_login()
  html.rs           Small HTML helpers
tests/flow.rs       End-to-end flows against wiremock
tests/fixtures/     Synthetic pages in the shapes the parsers match (no real data)
examples/efatura.rs          CLI: login / pending / list / show / set (real portal)
examples/resumable_login.rs  `start` and `finish <code>` as two separate processes
scripts/set-version.sh       Bumps Cargo.toml and Cargo.lock for a release (used by CI)
```

## Rules that matter

- **Never use or commit real data.** No real NIFs, passwords, SMS codes, cookies, invoice
  ids, names or phone numbers: not in code, tests, fixtures, issues or PRs. Placeholders:
  `123456789`, `000000000001`, `*******321`. Fixtures are synthetic and trimmed to what
  the parser reads; keep new ones that way.
- **Never retry authentication failures.** The AT suspends credentials after a few wrong
  passwords or SMS codes. `Error::is_auth_failure()` errors must reach the caller
  untouched; `Error::needs_login()` is where starting a new login is the right answer.
  Don't add retry loops around anything in `auth/`.
- **Don't test against other people's accounts**, and don't hammer your own: failed
  attempts count towards the lockout.
- **Secrets stay out of `Debug` and logs.** `Credentials`, `CookieJarState`, `PendingLogin`
  and `Client` redact; `tracing` output only carries URLs and statuses
  (`RUST_LOG=efatura=debug`). Keep it that way: no passwords, codes or cookies in errors.
- `PendingLoginState` and `SessionState` hold live session cookies. Document that callers
  must encrypt them at rest whenever you touch them.
- Money is integer cents (`2043` = 20.43 €) and VAT rates are hundredths of a percent
  (`2300` = 23 %). `DocumentId` wraps the portal's `idDocumento` (a number or a string).
- `Error` and `Sector` are `#[non_exhaustive]`; unknown sector codes become
  `Sector::Unknown`. Adding variants is not a breaking change; keep it that way.
- Every public item has a doc comment. Doc examples are `no_run`, read-only by default,
  and never change a user's tax data unless clearly commented out (see the
  `categorize` line in `src/lib.rs`).
- Parsers are matched against the real portal's output but tested on synthetic pages.
  If the portal changes, look for `UnexpectedResponse`, fix the parser, and update the
  fixture with a trimmed synthetic page, not a capture.

## Examples against the real portal

```bash
cargo run --example efatura -- login                      # `r` resend, `c` cancel at the SMS prompt
cargo run --example resumable_login -- start              # then, from another process:
cargo run --example resumable_login -- finish 123456
```

The session is stored in `.efatura-session.json` and the pending login in
`.efatura-pending.json` (mode 600, git-ignored). Never commit or paste them.

## Commits and releases

Versions are semver, decided by [Conventional Commits](https://www.conventionalcommits.org):
`feat!:`/`BREAKING CHANGE:` is major (minor while `0.y.z`), `feat:` is minor (patch while
`0.y.z`), `fix:`/`perf:` are patch, and `docs:`/`chore:`/`ci:`/`test:`/`refactor:` release
nothing. Pull requests are squash-merged, so the **PR title** is the commit message.

A bot keeps a **Release merge request** on GitLab with the changelog and the next
version (`Cargo.toml` and `Cargo.lock` bumped in the same commit); merging it publishes to
crates.io. **Do not** edit `RELEASE-NOTES.md`, the `version` in `Cargo.toml`, or the
`efatura` entry in `Cargo.lock` by hand, and do not create `v*` tags.

## Repository and CI

GitLab (`finances/efatura`) is the source of truth; this GitHub repository is mirrored from
it, and merged pull requests are pushed back to GitLab (fast-forward only, never forced).
`.gitlab-ci.yml` runs fmt, clippy, tests, docs, audit and a publish dry-run, mirrors master
(and its release tags) to GitHub, and publishes to crates.io when a release lands.
`.github/workflows/` runs the same checks on pull requests plus an MSRV build. Workflow
files under `.github/` can't be changed from a GitLab commit (the mirror's deploy key
cannot push them), so change those through GitHub pull requests.
