# Spec: local-access

> This spec records the original local-access phase. The protected app has since
> become the orchestrator console. It now uses first-party JavaScript and CSS
> served under `script-src 'self'`, while retaining the session, CSRF, loopback
> Host and exact Origin boundary described here. The read-only dashboard remains
> separate.

## Objective

Introduce a protected localhost app mode suitable for future configuration and
run controls without weakening the existing read-only dashboard. An individual
on the same computer can open the protected app from a URL generated at startup;
websites and unauthenticated clients cannot invoke protected operations.

## Scope and assumptions

- Linux, one trusted local user, Rust backend, modern browser, IPv4 loopback.
- Keep `synarch dashboard` read-only and compatible with existing links. Add a
  separate explicit protected app mode rather than silently granting it writes.
- Generate fresh high-entropy, short-lived bootstrap credentials per launch;
  communicate them without placing them in HTTP request URLs, logs, or persistent
  browser storage. Establish a bounded browser session with an HttpOnly,
  SameSite=Strict cookie and require a per-session CSRF token for mutations.
- Bind only to loopback; validate the exact Host and Origin, reject cross-origin
  requests, and do not enable CORS. Keep untrusted content escaped and retain a
  restrictive Content Security Policy.
- No real workflow or configuration mutation is introduced in this module.
  Its protected test endpoint must have no side effects. The authorization
  boundary is reusable by subsequent modules and applies to reads as well as
  writes in protected app mode.
- Local processes running as the same user, browser extensions, a compromised
  browser, and host-level compromise are not isolated. Do not expose or forward
  the listener; session credentials are not an OS sandbox.

## Commands

- Build: `cargo build --locked`
- Tests: `cargo test --locked`
- Focused tests: `cargo test --locked --test dashboard`
- Lint: `cargo clippy --locked --all-targets -- -D warnings`
- Format: `cargo fmt --check`
- Existing read-only mode: `cargo run --locked -- dashboard --port 8787`

## Structure and style

- `src/dashboard.rs` and related Rust modules own HTTP routing and access policy.
- `src/dashboard.html` and adjacent assets own browser presentation.
- `src/main.rs` exposes the explicit app entry point.
- `tests/` contains integration and adversarial HTTP tests.
- Use existing Rust conventions: typed inputs, `Result`, bounded reads, and
  explicit validation. Never render untrusted strings as HTML without escaping.

## Acceptance criteria

1. Protected mode can start without an existing state database; the existing
   read-only mode continues to require one and remains read-only.
2. Startup generates a fresh, expiring, single-use bootstrap credential. The
   browser can establish a session without credentials appearing in HTTP URLs,
   logs, referrers, or persistent browser storage. Restart invalidates sessions.
3. Protected endpoints reject missing, invalid, expired, and replayed credentials.
   Mutating requests additionally reject missing or invalid CSRF tokens, foreign
   origins, incorrect Host headers, and unsupported methods and content types.
4. No bootstrap or session credential appears in page content, error messages,
   request logs, or persisted application data. The per-session CSRF token may
   appear in authenticated forms but not URLs or logs.
5. The app makes no OpenCode model calls and changes no project, OpenCode config,
   or workflow state merely by starting or opening a page.
6. Tests exercise valid bootstrap and expiry/replay, session authorization,
   CSRF, hostile Origin/Host, disabled CORS, output escaping, and existing
   read-only behavior. Browser verification covers login, refresh, keyboard
   use, and error states.
7. Documentation describes startup, credential handling, limitations, and how
   to stop the app. Errors do not expose secrets or internal stack traces.

## Boundaries

- Always: validate input server-side; bound request sizes and lifetimes; use OS
  randomness; avoid logging credentials; test hostile requests; retain old tests.
- Ask first: changes to user OpenCode configuration, external integrations, or
  any model call during verification.
- Never: add unauthenticated write endpoints, expose non-loopback bindings,
  weaken controller acceptance gates, or claim that localhost is a sandbox.

## Open design detail

The implementation plan must specify how the one-time bootstrap is delivered
and exchanged without leaking it to the HTTP server's access log or referrers,
and how cookie security works on localhost HTTP. The exact HTTP interface will
be designed before implementation and tested against browser behavior.
