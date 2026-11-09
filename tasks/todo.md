# Tasks: local-access

## 1. Build and test the access policy

- [x] Add in-memory bootstrap, session, expiry, retry-limit, and CSRF validation
      with OS-generated secrets and atomic one-time exchange.
- [x] Test expiry, replay, invalid tokens, restart invalidation and attempt limits.
- Verify: focused Rust tests and `cargo build --locked`.
- Depends on: none. Likely files: new access module, `src/lib.rs`, tests.

## 2. Add protected localhost app mode

- [x] Add an explicit app command and strict bounded HTTP handling; retain the
      existing read-only dashboard behavior.
- [x] Expose only a non-sensitive login page before authentication and an inert
      authenticated test action protected by session, Host, Origin and CSRF.
- Verify: HTTP adversarial integration tests, existing dashboard tests, build.
- Depends on: task 1. Likely files: `src/main.rs`, new app module, tests.

## 3. Complete browser flow and documentation

- [x] Deliver the one-time bootstrap only via controlling terminal; add login,
      expiry and error states and document the local security boundary.
- [x] Verify Secure-cookie login on a real loopback browser, plus refresh and
      keyboard access. HttpOnly/SameSite attributes, replay, and cross-site
      requests are checked by HTTP tests.
- [ ] Independently verify replay and cross-site rejection in a real browser;
      inspect cookie attributes without exposing credential values.
- Verify: `cargo test --locked`, strict Clippy, `cargo fmt --check`, and browser.
- Depends on: task 2. Likely files: app assets, app module, README, tests.

## Checkpoint

- [ ] All local-access acceptance criteria are met; no live model calls occurred.
- [ ] Protected mode cannot start or mutate workflows yet. Existing dashboard is
      unchanged. Review the resulting boundary before project-setup begins.
