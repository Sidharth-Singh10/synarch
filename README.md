<p align="center">
  <img src="docs/images/mark.svg" width="56" alt="Synarch mark">
</p>

<h1 align="center">Synarch</h1>

<p align="center">
  <strong>A conversational orchestrator for evidence-gated coding work.</strong><br>
  Workspace-first chat · full-capability agents · optional isolated worktrees.
</p>

<p align="center">
  <img alt="Rust 1.89+" src="https://img.shields.io/badge/rust-1.89%2B-b53c16?style=flat-square&labelColor=151814">
  <img alt="OpenCode V2" src="https://img.shields.io/badge/opencode-v2-b53c16?style=flat-square&labelColor=151814">
  <img alt="Loopback only" src="https://img.shields.io/badge/local--first-loopback-b53c16?style=flat-square&labelColor=151814">
</p>

<p align="center">
  <img src="docs/images/console-run.png" width="100%" alt="Synarch console showing a confirmed run, live Jury, Actor, Critic and QA subagents, and the isolated worktree path">
</p>

<p align="center">
  <sub>One confirmed isolated run: the orchestrator, four native subagents, live tool activity, and the worktree path.</sub>
</p>

---

## Two ways to run

|  | Orchestrator console | Evidence-gated controller |
| --- | --- | --- |
| **Start** | `synarch app` | `synarch create` + `synarch run` |
| **Interface** | Conversation with a primary orchestrator | Strict typed role reports |
| **Workspace** | Choose a directory in the dashboard before chatting | Configured repository |
| **Execution** | Work directly or delegate; confirm an optional isolated run | Actor → checks → Critic → Jury → QA |
| **Acceptance** | Human review of transcripts and diff | Rust-enforced checks and acceptance gates |
| **State** | OpenCode sessions and session metadata | SQLite ledger |

The console is the conversational surface. The Rust controller remains a
separate CLI workflow when you need deterministic acceptance evidence. The
console does **not** use the controller's gates.

## What a run feels like

```mermaid
flowchart LR
  U[Choose workspace] -->|chat| P[Full-capability orchestrator]
  P -->|small tasks directly| R[Selected workspace]
  P -->|optional isolated proposal| C{Confirm?}
  C -->|Start run| W[Detached worktree]
  W --> O[Run orchestrator]
  O --> J[Jury]
  O --> A[Actor]
  O --> C2[Critic]
  O --> Q[QA]
  A -->|local commit| W
  Q -->|checks| W
```

1. **Choose.** Open **Select workspace**, enter an absolute directory on the
   Synarch host, and press **Open conversation**. The first prompt runs there.
   Recent workspace paths are suggested; changing workspace creates a new chat.
2. **Talk.** The orchestrator can research online, use `gh`, edit, and run checks
   itself. Small tasks do not require subagents or a proposal card.
3. **Delegate or isolate.** Larger work can use Jury, Actor, Critic and QA. Ask
   for an isolated run when changes should happen in a detached worktree; that
   path still requires confirmation through **Start run**.
4. **Review.** Inspect tool activity, transcripts and diffs, or interrupt active
   workers. Direct-workspace diffs include pre-existing edits and other work
   since the conversation's starting commit—not just agent changes.

Screenshots use a fictional repository and mocked session data.

### Workspace selection and confirmation

![Synarch workspace selector and optional isolated-run proposal](docs/images/console-planning.png)

*Select your workspace before chatting. The orchestrator has full tools from the
first prompt. Confirmation applies to creating an isolated run, not to ordinary
workspace tasks.*

### Inside a confirmed run

![Synarch run details showing task, worktree, stages and subagents](docs/images/console-run-details.png)

*All work happens in the detached worktree. Jury and Actor are done; Critic and
QA are running independently. `Stop all active` interrupts the live subagents.*

### Review the diff

![Synarch worktree diff drawer](docs/images/console-diff.png)

*The diff drawer compares the recorded baseline with the current working tree.
Large diffs are capped at 128 KiB.*

### On a phone

<p align="center">
  <img src="docs/images/console-mobile.png" width="330" alt="Synarch orchestrator console on a phone">
</p>

<p align="center">
  <sub>The main conversation stays first. Agents open in a sheet with a backdrop and focus trap.</sub>
</p>

## Execution and trust

- **Workspace first.** Each conversation is bound to its selected directory;
  opening another workspace does not move existing conversations or workers.
- **Direct work is direct.** Agents can modify your checkout immediately when
  you request a task. Existing changes are allowed; agents are instructed to
  preserve them. This is not a sandbox or a hard safety guarantee.
- **Isolation is optional and explicit.** A confirmed isolated run gets a
  detached worktree at the recorded baseline. Its proposal must match the
  conversation's repository, which must be clean and committed at that point.
- **Equal capabilities, different responsibilities.** All console roles have
  full tools. Instructions govern planning, implementation, review and QA;
  publishing and destructive operations require approval by instruction, not
  a command blacklist.
- **No automatic merge, push, deploy, or worktree cleanup.** Review evidence and
  use your own release process. Worktrees share Git metadata and host authority.

## Agents

| Agent | Mode | Responsibility |
| --- | --- | --- |
| **Orchestrator** | primary | Handle small tasks directly; delegate or propose isolation when useful. |
| **Jury** | subagent | Research, plan, establish acceptance criteria, and evaluate evidence. |
| **Actor** | subagent | Implement and verify changes; create local commits in isolated runs. |
| **Critic** | subagent | Independently review changes and substantiate findings. |
| **QA** | subagent | Independently verify behavior and report exact results. |

All roles have shell, edit, web, skills, and configured MCP access. Permissions
do not install tools, authenticate `gh`, or override higher-level OpenCode policy.

Synarch installs managed `synarch-*.md` profiles in
`$XDG_CONFIG_HOME/opencode/agents/` (normally `~/.config/opencode/agents/`) so
they are available in selected projects. These names are visible to other
OpenCode sessions too. Unrelated configuration is preserved; existing unmanaged
files with those names cause an error rather than being overwritten. Generated
worktree configuration also lives at `<db-parent>/dashboard/opencode.json`.

## Quick start

Requirements:

- Rust 1.89+ and Git
- A running OpenCode **V2** server with a provider connected
- An existing directory; isolated runs additionally require a clean Git
  repository with an initial commit and Git author identity
- A modern browser with JavaScript for the console
- Linux is the initial tested platform

Start the console from an interactive terminal:

```sh
cargo run --locked -- app --port 8788
```

Open the printed loopback URL and enter the one-time code shown in the terminal.
It expires after five minutes or ten failed attempts. A browser session lasts one
hour and is invalidated when the app stops.

Choose your workspace in the dashboard, then describe the task. For example:

> Fix the audit-log race that drops an entry
> when two writers flush at once. Keep the public API stable, add a concurrency
> regression test, and use `cargo test --locked`.

The default model is `opencode-go/deepseek-v4.1-flash`. Override it with a
`provider/model` reference:

```sh
SYNARCH_MODEL=your-provider/your-model cargo run --locked -- app --port 8788
```

The console auto-discovers the managed OpenCode service. Explicit
`SYNARCH_OPENCODE_URL` and `SYNARCH_OPENCODE_AUTHORIZATION` values take
precedence. Credentials are never returned to the browser.

### State and storage

The console uses `<db-parent>/dashboard/` and does not open or write the Synarch
SQLite database. The default `--db` path is
`$XDG_STATE_HOME/synarch/state.sqlite`, or
`$HOME/.local/state/synarch/state.sqlite`. Keep it outside the project checkout.

```text
dashboard/
  opencode.json      generated Synarch agent definitions
  worktrees/         one isolated worktree per run
```

OpenCode sessions and metadata are the run registry and conversation store.
Closing the console does not stop OpenCode sessions or subagents already running.
Legacy planning conversations remain readable; open a new workspace conversation
to use the full-capability workflow. If using an explicit OpenCode server, it
must share the host filesystem and see these agent files and workspace paths.

## The evidence-gated controller

The original controller owns every transition:

```text
Jury plan → Actor → mandatory checks → Critic → Jury decision → QA → ready
              ↑                                  │            │
              └──────────── Jury-directed rework ─┴── triage ──┘
```

It gives you strict typed reports, role attribution from the reserved assignment,
an isolated Actor worktree, deterministic argv checks, a separate QA worktree,
SQLite state and an ordered event ledger, and explicit recovery controls. Role
selection, transitions, checks, and gates are enforced in Rust; model text is
untrusted structured input, and SQL is parameterized.

The controller needs its agent profiles and workflow config. Copy
[`examples/workflow.json`](examples/workflow.json), merge
[`examples/opencode.jsonc`](examples/opencode.jsonc) into the configuration
OpenCode loads, set `SYNARCH_OPENCODE_URL`, and review the target repository
requirements before creating a run. The console generates its own agent
definitions and does not need that merge.

```sh
# Journal a run (does not launch agents)
cargo run --locked -- create --config /path/to/workflow.json \
  --request 'Fix shutdown handling'

# Advance until ready/escalated, an error, or the poll limit
cargo run --locked -- run RUN_ID --max-polls 200

# Inspect and recover
cargo run --locked -- status RUN_ID
cargo run --locked -- attempts RUN_ID
cargo run --locked -- events RUN_ID
cargo run --locked -- pause RUN_ID
cargo run --locked -- resume RUN_ID
cargo run --locked -- cancel RUN_ID --reason 'Stop this workflow'
cargo run --locked -- retry RUN_ID --reason 'Reconcile the stopped worker'
```

The read-only ledger dashboard remains available (it requires an existing
version 2 database):

```sh
cargo run --locked -- dashboard --port 8787
```

`ready` means engineering acceptance plus QA validation for the recorded commit.
It does not authorize a merge, push, deployment, or release.

See [recovery operations](docs/recovery.md) for retries, cancellation, escalation,
budgets, and the schema upgrade path.

## Build and test

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
cargo audit
```

Tests use real temporary Git repositories and SQLite databases. Console tests use
a fake OpenCode backend and spend no model credits. On OpenCode 2.2.5, a live
workspace smoke test verified direct editing without subagents, web fetching,
read-only `gh`, Jury shell/network access, transcripts, diffs and workspace
switching. The earlier isolated-run flow exercised all four roles and a clean
local commit. These are bounded smoke tests, not reliability guarantees. The
controller live tests are documented in [the smoke-test report](docs/live-smoke-test.md).

## Limits and trust boundary

- The console has **no deterministic acceptance gate**. The QA subagent runs the
  agreed checks and the UI shows evidence, but the orchestrator decides when work
  is done. Use the gated controller when you need Rust-owned acceptance.
- There is **no hard token or cost cap** in the console. Sending a message can
  immediately execute tools and spend provider allowance. Proposal confirmation
  applies only to isolated runs.
- Role instructions are **not security boundaries**. Every console agent can
  technically edit files and execute commands, including network operations.
- **Worktrees are not sandboxes.** Agents share Git metadata and the host account.
  Use separate OS identities or containers before running untrusted repositories.
- The read-only database dashboard has **no authentication** and binds to loopback.
  Do not forward it. The console has a one-time code, a Secure HttpOnly session
  cookie, exact loopback Host/Origin checks, and CSRF-protected mutations.
- OpenCode V2 HTTP behavior, including native subagents and session permissions,
  is experimental. Pin your OpenCode build and run a live smoke test.
- Session data, transcripts, prompts, and diffs can contain secrets. Protect
  OpenCode storage and the Synarch state root.
- This is a single-user, single-host MVP. There is no multi-user auth, distributed
  scheduler, automatic merge, or deployment integration.

## Docs

- [Orchestrator console operations](docs/orchestrator-console.md) — lifecycle,
  generated agents, permission inheritance, state root, troubleshooting
- [Recovery operations](docs/recovery.md) — retries, cancellation, escalation,
  budgets, schema upgrades
- [Live controller smoke test](docs/live-smoke-test.md)
- [Live rework harness](docs/live-rework-test.md)
- [Live cancellation report](docs/live-cancellation-test.md)
- [Local access security boundary](SPEC-local-access.md)

## Layout

| Path | Responsibility |
| --- | --- |
| `src/app.rs` | Protected console HTTP server and local access |
| `src/console.rs` | Orchestrator state, proposals, native runs, activity, diff |
| `src/console.html` · `.css` · `.js` | Console shell and assets |
| `src/opencode.rs` | OpenCode V2 client, durable dispatch, new session/chat API |
| `src/controller.rs` | Evidence-gated single-Actor coordination |
| `src/engine.rs` | Acceptance, authority, and invalidation guards |
| `src/store.rs` · `src/store/` | SQLite ledger, attempts, controls, migrations |
| `src/workspace.rs` | Git worktrees, snapshots, bounded checks |
| `src/dashboard.rs` · `src/dashboard.html` | Read-only ledger dashboard |
| `src/main.rs` | CLI commands and the app entry point |

<p align="center">
  <sub>Synarch is a local-first project. Do not expose its loopback listeners or run untrusted repositories on a shared host.</sub>
</p>
