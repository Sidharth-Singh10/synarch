# Live active-worker cancellation test

Run `SYNARCH_LIVE_CONFIRM=1 ./scripts/live-cancel.sh` to exercise the real
controller and OpenCode against a disposable Git fixture. It makes a real model
call, consumes provider allowance, and defaults to
`opencode-go/deepseek-v4.1-flash`. Ordinary `cargo test` ignores this test.
The launcher reads the local OpenCode service registration without printing its
credentials. The harness retains its fixture and ledger under
`/tmp/opencode/synarch-live-cancel.*` for inspection.

The test starts a Jury attempt and requires an unfinished assistant message with
nonempty streamed text or reasoning, alongside an active worker. A queued input
alone is not sufficient. It then invokes controller cancellation, checks that
stop intent precedes the observed stop, requires OpenCode to report the worker
interrupted with no active session or queued input, and checks that the run is
terminal. It observes the worker for a further two seconds and checks for late
transitions or extra attempts. If observation or assertions fail, a cleanup guard
attempts to cancel only the harness-owned run; an unconfirmed stop is printed as
a warning and must be investigated. A model that completes too quickly or does
not expose partial output within 45 seconds makes the test fail, not pass.

## Observed results, 2026-11-09

- **Passed initial live test:** `/tmp/opencode/synarch-live-cancel.k8sRzk`.
  Unfinished assistant output was observed while the Jury was active. The
  controller recorded stop intent, then observed `interrupted`, marked the
  attempt stopped and left the run cancelled. A later independent API check
  found the session interrupted, absent from the active list, and with an empty
  inbox. This initial version did not contain the two-second automated follow-up.
- **Follow-up run did not meet the observation prerequisite:**
  `/tmp/opencode/synarch-live-cancel.Yyhw47`. It did not expose observable
  unfinished output within 45 seconds, so the test failed. Cleanup first saw a
  still-running worker, retried, and confirmed it interrupted; the ledger shows
  both observations. The subsequently added two-second follow-up is therefore
  not yet verified by a passing live run.

This is evidence for one live cancellation, not a guarantee of provider-side
termination, billing cessation, or reliable cancellation in all timing and
network-failure cases. OpenCode quiescence is not proof that upstream inference
has ceased. The controller has no resident watchdog.
