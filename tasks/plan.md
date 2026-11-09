# Implementation plan: local-access

## Overview

Add an explicit protected local app mode as the foundation for later setup and
workflow controls. The existing `dashboard` command remains read-only. This
phase does not change OpenCode configuration, start workers, or add operational
controls.

## Design decisions

- Bind only to IPv4 loopback. Protected routes require a valid, bounded
  in-memory session; all POST requests require an exact Origin and Host, and
  mutations require a per-session CSRF token. No CORS is enabled.
- Generate separate, high-entropy OS-random bootstrap, session, and CSRF
  credentials. The bootstrap is single-use and expires; sessions expire and
  are invalidated on restart. Bound failed attempts and input sizes.
- Deliver the bootstrap code only through the launching user's controlling
  terminal (`/dev/tty`), never through a command argument, HTTP URL, normal
  log, file, or browser persistent storage. Fail closed if there is no
  controlling terminal. The user enters the code into a same-origin login form;
  the code is exchanged via a bounded POST body. No credentials are returned
  in error messages or URLs.
- Use an HttpOnly, SameSite=Strict, Secure session cookie with bounded Max-Age.
  Real-browser verification on `http://127.0.0.1` is a mandatory checkpoint:
  if the target browser will not accept Secure cookies on loopback, stop and
  revise the design rather than silently weaken it.
- Use simple server-rendered forms where possible; avoid JavaScript and
  third-party assets. Escape all untrusted text, use no-store and a restrictive
  CSP, and avoid exposing workflow data before authentication.
- Authentication is not OS isolation: other processes under the same user or a
  compromised browser remain outside the protection claim.

## Risks

| Risk | Mitigation |
| --- | --- |
| DNS rebinding or cross-site POST | Exact Host and Origin checks, no CORS, CSRF token |
| Login secret leaks | `/dev/tty` delivery, POST body only, no logging, no redirects carrying credentials |
| Concurrent or replayed bootstrap | Atomic in-memory single-use exchange and tests |
| HTTP parser ambiguity | Bounded, strict parsing; reject duplicate or conflicting headers and malformed framing |
| Browser cookie incompatibility | Verify real loopback browser before claiming the flow works |
| Existing server regressions | Keep read-only route unchanged and run its security tests |

## Ordered tasks

See `tasks/todo.md`. No database migration, model call, external network
integration, or user configuration modification is needed for this module.
