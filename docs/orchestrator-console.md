# Orchestrator console

The protected console (`synarch app`) is a conversational adapter over native
OpenCode V2 sessions. It is separate from the evidence-gated CLI controller.

## Lifecycle

1. The app discovers OpenCode and creates a private state root at
   `<db-parent>/dashboard/`.
2. It writes `dashboard/opencode.json` with a primary `synarch-orchestrator` and
   four subagents: `synarch-jury`, `synarch-actor`, `synarch-critic`, and
   `synarch-qa`.
3. A planning session is created in `dashboard/control-<config-hash>/`. Its session-level
   permissions deny everything, including `subagent`. The orchestrator has no
   repository access and cannot launch work.
4. The user chats. The orchestrator may ask the host to inspect an absolute
   repository path. Inspection is read-only and never executes project commands.
5. When the orchestrator returns a `propose_run` reply, the console validates the
   repository (clean, committed), task, and argv checks, then renders an inline
   confirmation card. The reply is bound to its assistant message ID; an older
   proposal cannot be started.
6. On **Start run**, the console creates a detached worktree, creates a fresh run
   orchestrator session there with **no session-level permissions**, seeds it with
   a planning handoff, and archives the planning session.
7. The run orchestrator uses its agent configuration (`deny all`, then allow
   `subagent` for the four roles) and launches native subagents. Child sessions
   receive no session-level permissions, so each role uses its own agent config:
   Jury read-only, Actor edit/shell, Critic read plus read-only Git shell, QA
   shell without edit. Common publish commands (`git push`, `git remote`,
   `gh pr`, `gh release`) are denied for the Actor and QA; this is a permission
   rule, not a general shell sandbox.
8. The console is an observer and control surface: it shows the orchestrator and
   its children, their current tool or target, transcripts, the worktree diff, and
   interrupt controls. It does not drive the agent loop.

The planning session is archived and hidden from the run rail. Its transcript is
available through **Run → Planning transcript**.

## State

OpenCode is the source of truth:

- Root sessions with metadata `synarch_kind = "intake"` are planning sessions.
- Root sessions with metadata `synarch_kind = "run"` are runs.
- Archived intake sessions have `synarch_kind = "archived"` and
  `synarch_replaced_by`.
- Child sessions are listed by `parentID` and are not persisted anywhere else.
- Session metadata records the repository, worktree, baseline, task, checks, and
  the intake session ID.

The app writes `dashboard/opencode.json` on demand. It does not open or write the
Synarch SQLite database. The `--db` flag supplies the state-root location only.

## Permissions and model

The generated agents use `opencode-go/deepseek-v4.1-flash` by default. Set
`SYNARCH_MODEL=provider/model` to override it. The run session inherits the
configured model; subagents inherit the parent session's model unless their agent
config sets one.

The console verifies the effective agents for the worktree before starting. It
refuses to start when a project-level `opencode.json` has overridden a Synarch
agent's mode, removed its expected tools, or granted the orchestrator edit or
shell access.

Session-level permissions are intentionally omitted on the run orchestrator. In
the tested OpenCode build (2.2.5), a subagent child copies the parent's
session-level permissions. Explicit parent permissions therefore make every child
inherit the orchestrator's restricted ruleset and lose its role tools. Omitting
them lets each child use its own agent config. This is why confirmation creates a
fresh run session instead of upgrading the planning session in place.

## Security and limits

- Loopback only, exact Host and Origin, HttpOnly SameSite=Strict session cookie,
  per-session CSRF, and `script-src 'self'` CSP. No third-party assets.
- The one-time code is written only to the controlling terminal.
- The browser never receives OpenCode credentials.
- The console never executes model text. It executes only validated proposal
  fields and session controls.
- Worktrees share Git metadata and the host account; they are not sandboxes.
- There is no hard token or cost cap and no deterministic acceptance gate in this
  mode. The proposal card is the approval point. Review the diff and transcripts.
- OpenCode sessions, transcripts, prompts, and diffs may contain secrets. Protect
  the state root and OpenCode storage accordingly.
- Closing the console does not stop OpenCode sessions or subagents. Reopen it to
  reconnect.

## Troubleshooting

- **OpenCode offline** — start the managed service or set `SYNARCH_OPENCODE_URL`
  and `SYNARCH_OPENCODE_AUTHORIZATION`, then press Retry.
- **Provider limit** — the transcript reports a bounded provider code and HTTP
  status. Change `SYNARCH_MODEL` or connect another provider in OpenCode.
- **Agents missing or overridden** — inspect the worktree's project-level
  `opencode.json`/`.opencode` configuration. The console refuses a run when the
  effective Synarch agents do not match.
- **Proposal rejected** — the repository is dirty, is not a Git repository, or the
  checks are invalid. Commit or stash the repository, then ask the orchestrator to
  propose again.
- **Run session remains after an error** — the worktree and metadata are retained
  for inspection. The console does not delete worktrees automatically.

## Verification status

The console flow was exercised manually on OpenCode 2.2.5 with the default
`opencode-go/deepseek-v4.1-flash` model against a throwaway Git repository:
planning, repository inspection, proposal confirmation, worktree creation, a
fresh run orchestrator, all four native subagents, live activity, child
transcripts, the baseline-to-working-tree diff, and a clean local commit. This
is one small verified flow, not an automated suite or a reliability guarantee.
The regression tests use fake OpenCode backends and do not spend model credits.
