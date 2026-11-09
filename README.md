# Synarch

A Rust-first, local agent workspace built on OpenCode V2.

There are two local modes:

- **Evidence-gated CLI controller** — the original single-Actor pipeline with
  independent Critic review, Jury decisions, mandatory checks, and QA. The
  controller, not an agent prompt, owns every transition. The CLI and the
  read-only database dashboard remain available.
- **Orchestrator console** — a protected local dashboard where you talk to a
  primary orchestrator. It inspects a repository, proposes a run, and then
  launches native OpenCode subagents (Jury, Actor, Critic, QA) in an isolated
  worktree. You can watch every subagent, open its transcript, and read the
  diff while it works. This mode does **not** use the controller's gates.

The console does not replace the controller. Use the CLI when you need
deterministic acceptance evidence; use the console when you want a conversational
orchestrator driving native subagents.

## Evidence-gated controller

```text
Jury plan → Actor → mandatory checks → Critic → Jury decision → QA → ready
              ↑                                  │            │
              └──────────── Jury-directed rework ─┴── triage ──┘
```

- Strict, typed JSON reports; role attribution comes from the reserved assignment.
- Controller artifact policy is supplied explicitly to every role: an Actor must
  create a local Conventional Commit, even if narrative planning text contradicts it.
  The engine still refuses an uncommitted implementation; prompts do not bypass that gate.
- SQLite state, decisions, findings, check evidence, and an ordered event ledger.
- Fresh OpenCode sessions per assignment and configurable per-role models/variants.
- An isolated Actor worktree and a separate accepted-commit QA worktree.
- Real argv-based mandatory checks, bounded runtime, and bounded retained output.
  Agents receive check names and the Jury receives evidence, but not controller-owned
  check command paths in their initial task context. Check evidence may contain
  command paths, and workers share the host; this is not an isolation boundary.

### Read-only dashboard

Run `synarch --db /path/to/state.sqlite dashboard` and open
`http://127.0.0.1:8787/`. Use `--port 0` to choose an available port, printed
on startup. Refresh the page to see changes. The dashboard requires an existing
version 2 Synarch database; it does not initialize or migrate it. It shows the
latest 500 runs and up to 500 transitions per selected run, current checks and
findings, execution limits, and attempt states. The on-demand diff compares the
recorded baseline and current snapshot in the Actor worktree, not uncommitted
changes or QA files; it is not a fresh snapshot verification. Diff output is
limited to 128 KiB and the Git process to three seconds. Missing worktrees or
objects, large diffs, and unavailable snapshots are reported in the UI. It does
not yet show historical diffs or check evidence, or workflow controls.

The server binds to IPv4 loopback and checks Host and Origin, but has **no
authentication**: local users and processes can access it. Do not forward its
port or use it on an untrusted shared host. Requests, reports and check output
may contain secrets, paths or other sensitive project data; they are not
automatically redacted. Frozen dispatches and configuration are not displayed.
Diffs and check output can reveal source code or secrets. Diff viewing invokes
local Git against the worktree recorded in the database; only use trusted local
databases and repositories. The database is opened read-only; SQLite may still need access to existing WAL
and shared-memory files. The dashboard is not a sandbox or deployment approval.

## Orchestrator console

Start the protected console in an interactive terminal:

```sh
cargo run --locked -- app --port 8788
```

Open the printed loopback URL and enter the one-time code shown in that terminal.
The code expires after five minutes or ten unsuccessful attempts; restart the app
for a new code. The browser session lasts one hour and is invalidated when the app
stops. The app requires a controlling terminal and will not run as a background
service without one. Stop it with Ctrl+C.

There is **no setup form**. After login, tell the orchestrator what you want. It
can inspect an absolute repository path without executing anything, then returns a
structured proposal. The console validates the proposal and shows a confirmation
card with the repository, task, checks, agents, and worktree isolation. Nothing is
created or started until you press **Start run**.

Starting a run:

1. Validates that the repository is a clean, committed Git repository.
2. Creates a detached worktree under `<db-parent>/dashboard/worktrees/`.
3. Creates a fresh primary orchestrator session in that worktree.
4. Seeds it with the agreed task, checks, and a planning handoff.
5. Lets it launch the native `synarch-jury`, `synarch-actor`, `synarch-critic`,
   and `synarch-qa` subagents.

The console shows the orchestrator and every subagent in the right-hand rail,
including its task, model, elapsed time, and current tool or target. Open any
agent to read its transcript. The Run tab shows the worktree, baseline, agreed
checks, derived stage activity, and the live baseline-to-working-tree diff.
"Interrupt" stops the orchestrator or a selected subagent; closing the console
does not stop OpenCode sessions or subagents that are already running.

The app owns a private state root at `<db-parent>/dashboard/`. Keep the database
path (and therefore this state root) outside the project checkout.

```text
dashboard/
  opencode.json      generated Synarch agent definitions
  control-<hash>/    planning sessions for this generated config
  worktrees/         one isolated worktree per run
```

It does not write the Synarch SQLite database. OpenCode sessions and session
metadata are the run registry and conversation store. The generated agent config
uses the default model `opencode-go/deepseek-v4.1-flash`; set `SYNARCH_MODEL` to a
`provider/model` reference to override it. Subagents inherit the session model.
The managed local OpenCode service is discovered automatically; explicit
`SYNARCH_OPENCODE_URL` and `SYNARCH_OPENCODE_AUTHORIZATION` values take
precedence. Credentials are never returned to the browser.

The console is not the evidence-gated controller. The QA subagent runs the agreed
checks, and the UI presents transcripts and diffs, but there is no Rust-owned
acceptance gate, no deterministic QA pass, and no automatic stop on a failed
check. The orchestrator decides when the work is done. There is also no hard cost
cap: a run can spend provider allowance, and the confirmation card is the approval
point. Review the final diff before merging anything.

The console uses an HttpOnly, SameSite=Strict, Secure session cookie, exact
loopback Host and Origin checks, CSRF tokens for mutations, a restrictive
`script-src 'self'` CSP, and no third-party assets. Keep the code private and do
not forward the port. Other processes running as your user, browser extensions,
and compromised browsers are not isolated. Worktrees share Git metadata and the
host account; they are not sandboxes. OpenCode session data, prompts, transcripts,
and diffs can contain secrets.

See [orchestrator console operations](docs/orchestrator-console.md) for the
lifecycle, generated agents, session permissions, and troubleshooting.

## Evidence-gated controller guarantees

The following apply to the `create`/`step`/`run`/`status` CLI workflow and the
read-only database dashboard, not to the native orchestrator console.

- Every downstream review and verdict is bound to a clean, full Git commit ID.
- Critic/Jury receive a real integrated Git diff without needing shell access.
  Diffs larger than 128 KiB stop the single-task slice rather than being silently truncated.
- Evidence-based finding dispositions; Jury cannot override failed mandatory checks.
- QA-check failures block readiness even when the QA agent reports a pass.
- Rework clears all current evidence and approvals. Historical evidence stays in the ledger.
- Restart recovery reuses persisted session and prompt IDs instead of launching a new Actor.
- Exact prompt, resolved model reference, session permissions, workspace, and server
  endpoint are frozen before dispatch. Recovery does not regenerate them.
- Explicit attempt outcomes, safe retry/cancel/escalation controls, absolute execution
  deadlines, and transactional launch budgets. See [recovery operations](docs/recovery.md).
- Single-host controller locking, idempotent report admission, pause/resume, and iteration limits.

`ready` means **engineering acceptance plus QA validation for the recorded commit**.
It does not authorize a merge, push, deployment, or release. No such actions are implemented.

## Requirements

- Rust 1.89+ and Git; Linux is the initial tested platform.
- A running OpenCode **V2** HTTP server with the required providers connected.
- A clean target Git repository with an initial commit and working Git author identity.
- Trusted, repeatable verification commands and any dependencies they require.
- For the console: a modern browser with JavaScript. The console generates its own
  agent definitions under `<db-parent>/dashboard/` and does not require an existing
  SQLite database. The default agent model is
  `opencode-go/deepseek-v4.1-flash`; set `SYNARCH_MODEL` to another
  `provider/model` if that model is not available in your OpenCode account.

The implementation uses the published V2 HTTP contract directly via `reqwest`.
No TypeScript runtime or plugin is necessary for this slice. A thin native plugin
can add TUI commands later without moving the engine out of Rust.

## Build and verify

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
cargo audit
```

Tests use real temporary Git repositories, SQLite databases, and processes.
Agent execution in controller tests is deterministic; HTTP adapter tests use a
local mock server. These tests do **not** spend model credits or claim to prove a
live OpenCode/model integration.

A separate live smoke test completed the full workflow on OpenCode 2.2.5 using
`opencode-go/deepseek-v4.1-flash`, reaching engineering acceptance and independent
QA validation. See [the smoke-test report](docs/live-smoke-test.md) for scope,
evidence, and issues discovered. This is one small verified workflow, not a
general reliability or isolation guarantee.

An opt-in live rework harness exists for the failure path:

```sh
SYNARCH_LIVE_CONFIRM=1 ./scripts/live-rework.sh
```

It deliberately fails a project policy check on the first implementation and
verifies that rework repeats every gate against the new snapshot. It spends model
credits and is not part of ordinary `cargo test`. See
[the rework harness guide](docs/live-rework-test.md).

An independent live cancellation harness can be run with
`SYNARCH_LIVE_CONFIRM=1 ./scripts/live-cancel.sh`. It uses provider allowance;
see [the cancellation report](docs/live-cancellation-test.md) for the passing
case, an inconclusive follow-up, and the limits of what the test proves.

## Configure

This section configures the evidence-gated CLI. The orchestrator console writes
its own agent definitions under `<db-parent>/dashboard/opencode.json` and does not
need this merge; it does honor the same OpenCode server connection settings below.

1. Merge the `agents` entries from [`examples/opencode.jsonc`](examples/opencode.jsonc)
   into the configuration your OpenCode server loads for the target project.
   Preserve your existing configuration. Do not replace it with the example.
   Profiles use `mode: "all"` because the controller creates sessions through the API.
   They deny tools by default; the controller supplies phase-specific session permissions.
2. Copy [`examples/workflow.json`](examples/workflow.json) to your own configuration file.
   Set `repository`, an external `workspace_root`, required checks, and agent profiles.
   Relative paths resolve against the configuration file's directory.
3. Set the server connection explicitly. The Rust adapter does not discover or start
   the managed background service:

   ```sh
   export SYNARCH_OPENCODE_URL=http://127.0.0.1:4096
   ```

   Use the actual address of your server, not necessarily that example port. If your
   server requires authentication, supply its full Authorization header value through
   `SYNARCH_OPENCODE_AUTHORIZATION` in your environment. Never put credentials in
   workflow JSON, command-line arguments, or the URL. Remote connections require HTTPS;
   redirects and implicit HTTP proxies are disabled.

An optional explicit worker model uses the V2 API reference shape:

```json
{
  "agent": "synarch-actor",
  "model": {
    "providerID": "your-provider",
    "id": "your-model",
    "variant": "your-supported-variant"
  }
}
```

Use IDs from your configured OpenCode catalog. With `model: null`, the controller
resolves the agent's configured model (or the catalog default) before freezing
dispatch. Workflow configuration is frozen at creation; each resolved worker model
is pinned at first dispatch. Live rerouting of an existing dispatch is not implemented.

## Run

```sh
cargo run --locked -- create --config /path/to/workflow.json --request 'Fix shutdown handling'
```

The JSON output contains the run `id`. Creation only journals the request; it does
not launch agents. Then, using that ID:

```sh
cargo run --locked -- step RUN_ID
cargo run --locked -- run RUN_ID --max-polls 200
cargo run --locked -- status RUN_ID
cargo run --locked -- events RUN_ID
cargo run --locked -- pause RUN_ID
cargo run --locked -- resume RUN_ID
cargo run --locked -- attempts RUN_ID
cargo run --locked -- retry RUN_ID --reason 'Connection restored; reconcile the original worker'
cargo run --locked -- cancel RUN_ID --reason 'Stop this workflow'
cargo run --locked -- resolve RUN_ID --reason 'Approve more execution time' --additional-seconds 600
```

`step` advances one stage or polls an existing agent. `run` repeats it until ready,
escalated/cancelled, an error, or the polling limit. Agent dispatch can spend your provider's
credits. Pause stops future scheduling; it does **not** interrupt active agents.
Polling limits do not stop already-dispatched agents either.

All output is JSON or JSON Lines for a minimal inspectable event viewer. Events
include assigned session IDs so you can inspect workers in OpenCode itself.

The default database is `$XDG_STATE_HOME/synarch/state.sqlite`, or
`$HOME/.local/state/synarch/state.sqlite`. Override it with global `--db PATH`.
Keep the database outside the source checkout and all agent workspaces. It contains
requirements, reports, and test output; protect it like private source code.

## Decisions and recovery

- Jury verdicts: `accept`, `rework`, `experiment`, `retry_qa`, `escalate`.
- Each current finding requires one `fix` or `reject` disposition with rationale.
- QA failures enter Jury triage. Acceptance cannot bypass that triage. The Jury may
  request corrections, unchanged-snapshot experiments, or reject substantiated false
  positives and request a **new** independent QA run; it cannot manufacture a QA pass.
- Rework, experiments, and QA retries consume the shared iteration allowance.
- A frozen assignment and launch intent are stored **before** session creation/prompt
  admission. Transport recovery uses the same session/prompt IDs, payload, and deadline.
  A new worker attempt is allowed only after a confirmed stopped failure and an
  explicit retry; both stage and run budgets still apply. Prompt replay safety depends
  on V2's documented durable ID-based admission.
- Workflow transitions, report deduplication, and event insertion commit atomically.
  Deterministic checks may execute again after a crash before their result commits;
  configure them to be repeatable. Exactly-once external execution is not claimed.
- Malformed reports, failed sessions, ownership mismatches, dirty worktrees, and
  unexpected commit changes stop progress rather than silently relaxing a gate.
  Inspect `attempts` and the recorded session/worktree before proceeding. Use the
  explicit recovery controls; malformed reports are not silently rewritten or approved.
- Provider failures identify the worker session, bounded error code, and HTTP
  status without printing raw provider messages or response bodies. An admitted
  prompt with no execution outcome yet remains pending rather than being reported
  as a failed worker.
- State changes persist immediately. A restart does not depend on replaying live events.
- Schema upgrades are transactional. Existing V1 evidence is retained, but unfinished
  V1 dispatches are blocked because their original prompts were not stored. Back up
  your database before upgrading; see [migration guidance](docs/recovery.md#schema-upgrade).
- Worktrees and sessions are retained for inspection. There is no automatic cleanup
  or integration into the original checkout. Merge the reviewed commit only after
  your own release process; do not delete worktrees with unknown user changes.

## Trust boundary and limitations

This MVP assumes a **trusted local user, trusted OpenCode server, and trusted test
configuration**. Model reports and HTTP responses are untrusted structured input.
Role selection, transitions, snapshot checks, and mandatory gates are enforced in Rust.
SQL is parameterized; LLM text never becomes a controller shell command.

**Worktrees are not sandboxes.** They share Git metadata, configuration, hooks, and
the host account. Actor/QA shell commands and repository tests execute with host
authority. OpenCode permission rules restrict normal tool paths, but do not prove
that a hostile process cannot access state, shared Git metadata, secrets, or another
checkout. Use separate OS identities/containers and protected evidence storage before
running untrusted repositories or agents. Snapshot checks detect ordinary drift;
they do not defend against a hostile process that mutates and restores source.

Snapshots cover committed tracked source, not ignored build outputs, dependency
caches, external services, or toolchain state. Actor and QA worktrees have separate
working copies, **not** hermetic environments. Reproducible environment manifests,
hard cost/token accounting, integration scheduling, and deployment requirements
belong to subsequent slices. Absolute wall-clock deadlines and launch counts are
enforced during controller execution, but are not a global spending guarantee:
there is no resident watchdog when the controller is stopped. Stop confirmation
covers the owned OpenCode session/inbox, not every background process a test might
spawn. See [budget and cancellation limits](docs/recovery.md#limits-and-guarantees).

OpenCode's HTTP surface (including `session.wait`, native subagents, session
moves, and permission overrides) is experimental. Pin your OpenCode build and run
a live smoke test before relying on unattended execution. The controller supports
one execution step at a time per database and is not a distributed scheduler. The
orchestrator console is a separate local adapter built on native OpenCode
sessions; it does not share the controller's serializer or acceptance gates.
Closing the console does not stop OpenCode sessions or subagents that are already
running.

## Layout

| File | Responsibility |
| --- | --- |
| `src/contracts.rs` | Strict reports, roles, phases, and persisted run state |
| `src/engine.rs` | Pure acceptance, authority, and invalidation guards |
| `src/store.rs` | SQLite transactions, attempts, locking, and event ledger |
| `src/store/` | Schema migration, frozen execution records, and operational controls |
| `src/runtime.rs` | Execution states, deadlines, and budget contracts |
| `src/recovery.rs` | Explicit retry, cancellation, and escalation recovery |
| `src/workspace.rs` | Git worktrees/snapshots and bounded check execution |
| `src/opencode.rs` | Authenticated V2 HTTP dispatch and report parsing |
| `src/controller.rs` | Single-Actor coordination and context handoffs |
| `src/app.rs` | Protected orchestrator console HTTP server |
| `src/console.rs` | Native orchestrator state, proposals, runs, activity, and diffs |
| `src/console.html` / `.css` / `.js` | Orchestrator console assets |
| `src/favicon.svg` | Console icon |
| `src/main.rs` | Minimal CLI controls and JSON event viewer |

API sources: [OpenCode V2 API](https://opencode.ai/v2/docs/api),
[agents](https://opencode.ai/v2/docs/agents),
[server plugin/session semantics](https://opencode.ai/v2/docs/build/plugins/server),
[permissions](https://opencode.ai/v2/docs/permissions).
