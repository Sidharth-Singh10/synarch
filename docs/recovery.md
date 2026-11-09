# Recovery operations

Operational recovery cannot accept code, waive a failed check, reject a finding,
or manufacture a QA pass. All existing technical gates remain in force.

## Inspect first

```sh
synarch status RUN_ID
synarch attempts RUN_ID
synarch events RUN_ID
```

`status` includes the run's `execution_control`: absolute deadline, launch count,
limits, cancellation intent, and authorized escalation return phase. `attempts`
contains the frozen dispatch, immutable session/prompt IDs, attempt sequence,
start/deadline timestamps, state, failure code, and `stop_confirmed` flag.

Attempt states are `prepared`, `running`, `succeeded`, `failed`, `blocked`,
`cancelling`, `interrupted`, and `timed_out`. A running state represents a journaled
launch intent, not proof that the provider started generating. Unknown transport
outcomes are treated conservatively. Raw provider messages and response bodies
are not copied into the failure ledger.

Frozen native dispatches contain the exact prompt, resolved model reference,
permissions, agent ID, directory, and normalized server endpoint. Credentials are
not included. The server must still match its saved session-ownership metadata,
agent, location, model, and session permission overrides. Reconnecting to a different
endpoint cannot establish that a worker on the original server stopped.

This freezes the submitted request, not OpenCode's entire runtime: provider/model
settings, agent definitions, project instructions, tool implementations, and the
host environment remain externally managed.

## Retry

```sh
synarch retry RUN_ID --reason 'Network recovered; inspect and continue the existing attempt'
synarch run RUN_ID
```

- A blocked transport/verification attempt reuses its dispatch and IDs; its deadline
  is not restarted. A workspace problem must first be repaired by the operator.
- A confirmed failed/interrupted worker may get a new session and prompt ID, while
  retaining its original dispatch payload. The old attempt and evidence remain intact.
- A still-running worker is only observed through the same IDs. Unknown stop state
  cannot authorize a replacement worker. A failed reconciliation leaves work blocked.
- A pre-dispatch failure with no external launch can be retried before freezing.
- Stage attempt limits and run launch/deadline limits still apply. Exhaustion escalates
  the run rather than silently opening another automatic retry loop.

Retry does not modify source files, clean worktrees, change models, or relax checks.
It also does not automatically repair malformed reports. An unchanged invalid final
report will remain invalid; cancel/start a revised workflow if fresh instructions
or a different model are needed.

## Cancel

```sh
synarch cancel RUN_ID --reason 'Stop this workflow and retain its artifacts'
```

The controller first journals terminal cancellation intent, blocking any further
scheduling or report admission. It then cancels only its own pending prompt,
requests interruption of the owned session, and checks both the running-session
map and durable inbox. An interrupt response alone is not accepted as stop proof.
Other sessions and other pending prompt IDs are not deleted.

If the server cannot be reached, ownership differs, the original endpoint is not
selected, or input/execution remains active, the command fails with attempts left
`cancelling` and `stop_confirmed: false`. **A cancelled run phase alone does not
prove the remote worker stopped.** Reissue `cancel` with the correct server to
finish reconciliation. Never assume an uncertain worker can safely be replaced.

Cancellation is terminal; `resume`, `retry`, and `resolve` cannot reopen the run.
No files, worktrees, sessions, commits, or evidence are deleted. Completed `ready`
runs are not operationally cancelled. An undispatched run can be cancelled offline.

## Resolve escalation

```sh
synarch resolve RUN_ID --reason 'Approve ten more minutes' --additional-seconds 600
synarch resolve RUN_ID --reason 'Approve two more launch intents' --additional-launches 2
synarch resolve RUN_ID --reason 'Approve one more Jury-authorized iteration' --additional-iterations 1
```

Resolution requires a reason and confirmed quiescence of prior workers. It restores
only the recorded authorized stage, creating a new scheduling revision and recording
the operator's context. It cannot jump to acceptance or readiness. Source snapshots,
mandatory checks, findings, and applicable prior engineering acceptance are retained;
the normal stage/snapshot guards apply on the next step.

Expired run time, exhausted launch allowance, and exhausted Jury iteration allowance
must be extended explicitly when applicable. All extensions are bounded and audited.
Resolving stage-attempt exhaustion authorizes a new bounded scheduling revision;
the run-wide launch count is never reset. Resolution never restarts the whole run's
clock or refunds earlier launch intents.

Pause/resume remain scheduling controls, not cancellation. They do not renew budgets.
If pausing prevents a completed report from being admitted, inspect its blocked
attempt and explicitly retry after resuming.

## Limits and guarantees

The optional `limits` object in workflow JSON defaults to:

```json
{
  "worker_timeout_seconds": 900,
  "run_timeout_seconds": 3600,
  "max_agent_launches": 30,
  "max_attempts_per_stage": 3
}
```

- Run time starts at creation, includes paused/downtime, and is stored as an absolute
  Unix-millisecond deadline. Worker time starts with the first journaled launch intent.
- A launch intent is counted transactionally before contacting OpenCode. Polling the
  same attempt does not consume another launch. Failed/uncertain intents are not refunded.
- Native HTTP requests and mandatory commands are clamped to remaining execution time.
  Late reports are rejected, including inside the final report-commit transaction.
- Deadline/launch exhaustion records escalation and stop intent before attempting
  native interruption. Cancellation has a separate bounded network grace period.
- These checks require an active controller. There is no resident watchdog to stop
  remote execution while the CLI is dead or paused. The next execution/recovery call
  enforces expired deadlines; already incurred provider usage cannot be undone.
- The local system clock and configured OpenCode server are trusted. This is not a
  hard realtime or distributed lease protocol.
- Quiescence means the owned session has no active drain or queued input. It does not
  prove every background shell process, external service, or in-flight provider request
  has stopped. Use OS isolation/supervision for that guarantee.
- Launch/time limits are **not dollar or token caps**. Native model step settings also
  do not establish an account-wide billing limit.

## Schema upgrade

Schema V2 retains V1 run state, commit references, reports, and event history.
Migration is transactional and malformed legacy data rolls the schema change back.
Opening a newer, unsupported schema is rejected.

Back up an existing database using SQLite's backup API/tooling before upgrading.
Do not copy only a live `.sqlite` file while it may have outstanding WAL data.
Only open trusted Synarch-owned databases; unrelated SQLite files are refused.

An unfinished V1 attempt did not persist its prompt, so its original dispatch cannot
be reconstructed faithfully. It is migrated as `blocked`, marked `legacy_unfrozen`,
and requires explicit reconciliation. Do not synthesize a new prompt with its old
ID. The simplest safe path is to cancel/reconcile that run and create a new one.
Completed legacy runs remain inspectable; their historical prompts cannot be
retroactively frozen.

## Verification

Automated tests cover frozen dispatch drift, bounded fresh attempts, unchanged replay
deadlines, late-report rejection, persisted stop intent, unknown termination, wrong
server/ownership, queued input, explicit budget extensions, terminal cancellation,
transactional launch counting, and migration preservation/rollback.

A native control-path check also migrated a SQLite-backup copy of the earlier live
smoke-test ledger and cancelled/reconciled an already-failed owned session using
OpenCode 2.2.5. It confirmed stop state and preserved the completed `ready` run.
The original V1 ledger was unchanged; no model calls were made. That check does
not claim live interruption of an actively generating model or every host process.
