# Live smoke test

## Result

The single-Actor workflow reached `ready` with both `engineering_accepted: true`
and `qa_validated: true` on 2026-11-09.

- Harness: OpenCode V2.2.5, existing authenticated local service.
- Model for every role: `opencode-go/deepseek-v4.1-flash` (not the Free variant).
- Run: `23bf62cf-3bd9-45fc-a9e9-ffa4a64af904`.
- Accepted fixture commit: `88a4e52b46bc74957741e1377ea1f16773bd2bf7`.
- Baseline: `cf0d35faf1707050a810e090642cdff06c93de64`.

## Task and evidence

The disposable Rust fixture initially implemented `is_even(i32)` incorrectly as
`value % 2 == 1`. Its committed positive-number regression test failed before
dispatch. The task required a minimal fix, preserved public API and dependencies,
and new tests for zero, negative values, and both i32 boundaries.

The persisted event ledger confirms:

1. Jury established acceptance criteria and a single-Actor plan.
2. Actor changed only `src/lib.rs`, fixed parity, added five regression tests,
   and created a local Conventional Commit.
3. Controller ran `cargo test --locked` (six tests passed) and strict Clippy
   (`cargo clippy --locked --all-targets -- -D warnings`, exit zero).
4. An independent Critic session reported no findings.
5. A fresh Jury decision session accepted the exact reviewed commit.
6. An independent QA session verified behavior in a separate worktree. Its tool
   transcript includes source/diff inspection, test/lint execution, and a separate
   compiled parity-checking program that reported `EXHAUSTIVE_OK`. This transcript
   was inspected rather than relying only on the final QA narrative.
7. Controller reran mandatory checks in the QA worktree; both passed before readiness.

Final filesystem inspection confirmed that Actor and QA worktrees were clean and
at the accepted commit. The original checkout remained clean at its baseline.
No push, merge, deployment, or global OpenCode configuration change was performed.

## Issues exposed by the live test

- The configured free default and DeepSeek Free both returned
  `provider.free-limit` / HTTP 429. They were not treated as workflow passes.
  The user selected the non-Free DeepSeek model before subsequent dispatch.
- The initial Jury plan incorrectly prohibited local commits. This contradicted
  the controller's clean committed-snapshot contract, so implementation admission
  stopped. Role prompts now carry an explicit mandatory artifact policy, with
  regression assertions. The engine's existing dirty-tree gate remains enforced.
- Provider failures previously had an opaque message. Diagnostics now include
  the session, bounded error code, and HTTP status without logging raw provider
  messages or response bodies. Regression tests cover this boundary.
- A shell wrapper timed out while the Critic was still running. Resuming reused
  the same persisted Critic session and prompt IDs; no replacement Critic was
  launched. The resumed workflow continued through Jury acceptance and QA.

## Local evidence and limits

At verification time, the SQLite ledger and worktrees were retained under
`/tmp/opencode/synarch-smoke.hLzGcg`. These are temporary local artifacts, not
portable checked-in fixtures, and may be removed by normal temporary-file cleanup.
Credentials were passed in memory and were not included in workflow JSON or logs.

This validates one small live success path and controller restart/monitoring
continuity. It does not establish multi-Actor integration, complex-task reliability,
hostile-agent isolation, hermetic builds, or production release readiness.
The automated suite separately covers rework, QA rejection, mandatory-gate failures,
stale snapshots, report deduplication, and persistence.
