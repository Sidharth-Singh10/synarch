# Live rework harness

`tests/live.rs` is an **opt-in** integration test that drives the real controller
and OpenCode against a disposable fixture. Normal `cargo test` ignores it.

## What it proves

The fixture starts with an intentionally failing `is_even` regression test. Its
workflow has two mandatory checks:

- `tests`: `cargo test --locked`
- `policy`: a controller-owned script outside the source checkout

The `policy` check requires a `Refs: POLICY-42` trailer in the current commit
message. The user request does not mention this project policy. The Actor does not
receive check results; the Jury must interpret the actionable failure and order
rework. The policy deliberately lives in Git metadata, so rework cannot contradict
a reasonable file-scope criterion.

An earlier doc-comment policy was too easy: the model added a doc comment on its
first attempt and never exercised rework.

The test asserts that on success:

1. The first implementation fails the policy check.
2. The Jury records `rework`; acceptance does not occur with a failed gate.
3. A fresh Actor attempt produces a different snapshot.
4. Checks run again for the new snapshot; the Critic reviews that snapshot.
5. The accepted snapshot reaches independent QA, and QA uses the same commit.
6. Final readiness has every mandatory check passing.

A run that converges without a rework, escalates, or weakens a gate fails the test.

## Run

```sh
SYNARCH_LIVE_CONFIRM=1 ./scripts/live-rework.sh
```

The script:

- refuses to run unless `SYNARCH_LIVE_CONFIRM=1` acknowledges provider usage;
- reads the local OpenCode service registration and passes Basic auth in memory;
- never prints credentials;
- uses `opencode-go/deepseek-v4.1-flash` unless `SYNARCH_LIVE_MODEL` is set;
- runs only the ignored live test, single-threaded, with console output.

## Costs and limits

- Real model calls and provider charges/allowance consumption occur.
- The harness assumes a trusted local service, trusted fixture, and the configured
  model's availability. A quota rejection is reported as a failure, never a pass.
- The fixture policy check is hidden from the Actor to force the gate/rework path.
  This is test instrumentation, not a recommendation to hide production checks.
- Evidence is written under a temporary `/tmp/opencode/synarch-live-rework.*`
  directory and is not portable. Copy it before relying on it.
- This covers one rework scenario, not general model reliability.

## Findings from harness development

- The first live attempt exposed a real report-contract failure: the Jury emitted
  a JSON object whose `kind` field sat **after** the closing brace, making the
  response invalid. The controller blocked the attempt; it did not infer the role
  or accept the payload. Cause: the example was serialized with keys alphabetically
  sorted, so `kind` appeared last. The example now puts `kind` first, the prompt
  explicitly forbids trailing fields, and a regression assertion guards the prompt.
- `serde_json` now enables `preserve_order` so generated examples follow the
  documented field order.
- The fixture must commit a real `Cargo.lock`, otherwise `--locked` fails for an
  unrelated reason and muddies the policy-gate scenario.
- An earlier fixture required a `CHANGELOG.md` entry, while the Jury independently
  wrote an acceptance criterion that only `src/lib.rs` may change. After rework the
  Critic correctly flagged the changelog edit, and the Jury correctly escalated
  rather than accept a snapshot with an unresolved finding. That was a genuine
  contradiction in the test scenario, not a workflow defect. The policy now lives
  inside `src/lib.rs`, and the planning prompt warns against inventing file-scope
  constraints that can conflict with mandatory checks.
- A panicking test previously deleted its temporary ledger on unwind. The harness
  now keeps the fixture directory so failures remain diagnosable.
- A doc-comment policy passed on the first Actor attempt, so it did not prove rework.
  The policy now requires a commit-message trailer; the first pass omits it.

## Results

Verified on 2026-11-09 with OpenCode 2.2.5 and
`opencode-go/deepseek-v4.1-flash`:

| Item | Value |
| --- | --- |
| First implementation snapshot | `7bc11a6dbb4ec3aa4dfb6d60f1a92972bff6441a` |
| Accepted snapshot | `d9f9314be526c5210a317b70317dd1e6495c4b4a` |
| First checks | `tests` passed; `policy` failed with the trailer instruction |
| Rework | Jury ordered amending the existing commit, not a new worker path |
| Second checks | `tests` and `policy` passed on the new snapshot |
| Outcome | Critic reviewed, Jury accepted, independent QA passed, `ready` |
| Elapsed | 219 seconds |
| Evidence | `/tmp/opencode/synarch-live-rework.C31Cuh` and `/tmp/opencode/synarch-live-policy.apNlke` |

The test asserts that acceptance happened only after the second passing check
round, that the two reviewed snapshots differ, that a fresh Actor attempt produced
the rework, and that QA and readiness reference the same accepted commit.

An earlier completed live run (`/tmp/opencode/synarch-live-rework.UaQLRz`) showed
the same chain with snapshots `26522a54...` → `ca6f9745...`.

## Additional findings from the verified runs

- The first hidden policy was a doc comment. The model added it spontaneously on
  its first pass, so no rework occurred.
- The next policy was a commit trailer, but agents initially received full check
  command paths. The Actor read the controller script and satisfied the trailer
  immediately. Agents now receive only check **names**; the Jury receives evidence.
  The initial task context omits command paths, but check evidence can contain
  them. Worktrees and host permissions do not isolate checks from workers.
- The first successful rework run failed the harness's own assertion because it
  read check results from the implementation→checking transition instead of the
  checking→reviewing transition. The workflow was correct; the harness selector
  is fixed and guarded by the green run above.
