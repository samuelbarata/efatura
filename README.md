# efatura

Unofficial Rust client for Portugal's **e-fatura** portal
(<https://faturas.portaldasfinancas.gov.pt>). It logs in *as the user*
(NIF + password + SMS second factor) and drives the endpoints the website uses,
so you can list invoices, read their detail and assign IRS sectors from code.

The AT has no public consumer API. Everything here is reverse-engineered and
can break whenever they change the site.

> **Unofficial.** This project is not affiliated with, endorsed by or supported by
> the Autoridade Tributária e Aduaneira (AT). It automates the website you could
> use by hand, with your own credentials. Check the portal's terms of use for your
> situation, and use it at your own risk: the AT suspends credentials after a few
> wrong passwords or SMS codes (see [Careful with the account](#careful-with-the-account)).

```toml
[dependencies]
efatura = "0.1"
```

Requires Rust 1.88 or newer.

## What makes it different

The SMS second factor is a first-class, **resumable** step. A login can start
in one process and finish in another:

```rust
use efatura::{Authenticator, Config, Credentials, LoginOutcome, PendingLogin};

// Request A: user asked to sync -> start login, AT sends the SMS
let outcome = Authenticator::new().login(&Credentials::new(nif, password)?).await?;
let LoginOutcome::SmsRequired(pending) = outcome else { /* no 2FA */ todo!() };
let state = serde_json::to_string(&pending.state()?)?;   // store it (encrypted!)

// Request B, minutes later: the code arrives through a webhook
let mut pending = PendingLogin::restore(serde_json::from_str(&state)?, Config::default())?;
let client = pending.submit_code("123456").await?;
let session = client.session()?;                         // store it too, reuse until it expires
```

## Operations

| | Method |
|---|---|
| Log in (password, SMS, resend, cancel) | `Authenticator::login`, `PendingLogin::{submit_code, resend_code, cancel}` |
| Persist / restore | `PendingLogin::{state, restore}`, `Client::{session, from_session}` |
| Session alive? | `Client::is_authenticated` |
| List invoices (buyer side), filtered by period, sector, state | `Client::documents(&DocumentQuery)`, `Client::pending_documents` |
| Invoice detail (lines, current sector, allowed sectors) | `Client::document(key)` |
| Assign a sector | `Client::set_sector`, `Client::categorize` (set + re-read to confirm) |

Amounts are integer cents (`2043` = 20.43€); VAT rates are hundredths of a
percent (`2300` = 23%). `DocumentId` (`idDocumento`) is unique and stable, so
it's the value to store next to a bank transaction.

Sectors (`Sector`): C01 car repair, C02 motorcycle repair, C03 restaurants and
accommodation, C04 hairdressers, C05 health, C06/C26 education, C07 housing,
C08 care homes, C09 vets, C10 transport passes, C11 gyms, C12 newspapers,
C13 books, C14 arts, C15 museums, C99 other (general family expenses). Each
invoice only accepts the sectors in `DocumentDetail::available_sectors`.

## Try it against the real portal

```bash
cargo run --example efatura -- login
```

```bash
cargo run --example efatura -- pending 2026-01-01 2026-12-31
```

```bash
cargo run --example efatura -- list 2026-01-01 2026-12-31 --json
```

```bash
cargo run --example efatura -- show <ID> <YYYY-MM-DD>
```

```bash
cargo run --example efatura -- set <ID> <YYYY-MM-DD> C05
```

To see the two-process login (the point of this crate), use the `resumable_login`
example: `start` asks for NIF + password and the AT sends the SMS, `finish` completes
it from a different process:

```bash
cargo run --example resumable_login -- start
```

```bash
cargo run --example resumable_login -- finish 123456
```

The session is saved to `.efatura-session.json` (mode 600, git-ignored).
`RUST_LOG=efatura=debug` logs every request.

## Status

Verified against the live portal (October 2026, a real account with SMS 2FA):
login with the SMS code, listing pending invoices, invoice detail, and setting
a sector (`resolverPendenciaAdquirente.action`).

Not yet exercised live: `resend_code`, `cancel`, and error pages such as a wrong
password, a wrong code or suspended credentials. Those follow the login app's
JS bundle and are covered by mock tests only. If something breaks, run with
`RUST_LOG=efatura=debug` and look for `UnexpectedResponse`.

## Careful with the account

* The AT locks credentials after a few wrong passwords or SMS codes (for one
  hour on the SMS side). `Error::is_auth_failure()` marks errors that must
  **never** be retried automatically.
* `PendingLoginState` and `SessionState` contain live session cookies, and the
  AT password opens the user's whole tax record. Encrypt all of them at rest.

## Integrating into an app (sketch)

This is how the crate is used in a self-hosted personal-finance app (MoneyScale):

1. Store the NIF and password encrypted at rest.
2. A sync job calls `Authenticator::login`. On `SmsRequired`, it stores the
   `PendingLoginState` (encrypted, with a TTL of about 5 minutes) keyed by user.
3. An HTTP endpoint receives the code, for example from an iOS Shortcuts automation
   ("When I get a message from AT containing 'código'" → extract the 6 digits →
   *Get contents of URL*). Authenticate that endpoint with a per-user secret, not a
   browser session. It restores the pending login, calls `submit_code`, stores the
   resulting `SessionState` and wakes the job.
4. The job reuses `SessionState` until `Error::SessionExpired`, then starts again
   from step 2.
5. Matching invoices to bank transactions: candidates are matched by issuer NIF,
   amount (`total_cents`), date and ATCUD, then `categorize` assigns the sector from
   the transaction's category.

## Contributing

Issues and pull requests are welcome, on GitHub (<https://github.com/samuelbarata/efatura>).
See [CONTRIBUTING.md](CONTRIBUTING.md). Never include real NIFs, passwords, session
cookies or invoice data in issues, tests or fixtures.

## License

[MIT-0](LICENSE) (MIT No Attribution): do whatever you like with it, commercially
or not, no attribution required. It comes with no warranty and no liability.
