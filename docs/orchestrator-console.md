# Orchestrator console

The protected console (`synarch app`) is a workspace-first adapter over native
OpenCode V2 sessions. The separate evidence-gated CLI controller is unchanged.

## Start and choose a workspace

```sh
cargo run --locked -- app --port 8788
```

Open the printed loopback URL and enter the one-time code from the interactive
terminal. Before chatting, expand **Select workspace**, enter an absolute
directory on the Synarch host, and press **Open conversation**. Recent workspace
paths are suggested, and the browser remembers the last entered path.

The server validates and canonicalizes the directory before creating an OpenCode
session there. No conversation is created in a planning/control directory. The
first prompt receives the actual session directory and repository context.
Workspace selection opens a new conversation; it never moves existing sessions
or redirects their running workers. Non-Git directories and dirty repositories
are supported for ordinary chat. Existing user changes must be preserved.

## Direct work and optional isolation

The orchestrator has full tools and handles small tasks directly. It can read,
edit, execute commands, use `gh`, fetch/search the web, load skills and use
configured MCP tools. It delegates when work is substantial or benefits from
independent investigation, review or QA—not because it cannot use tools itself.

For a larger change that benefits from isolation, or when requested, it returns a
structured `propose_run` reply. The card validates the selected repository, task
and argv checks. A proposal cannot target another repository. **Start run**
requires a clean, committed repository, a current assistant proposal, and an idle
orchestrator. It creates a detached worktree and a fresh execution session with
the conversation handoff. The source conversation remains available in its
original workspace. The same proposal cannot start twice.

Only isolated-run creation requires this confirmation card. Sending an ordinary
chat message can immediately modify the selected workspace and spend provider
allowance. Neither workflow provides deterministic acceptance gates or a hard
spending cap; use the gated CLI when Rust-owned acceptance is required.

## Agent configuration and roles

All five console profiles use a wildcard allow rule. No role-specific tool
blacklists or restrictive session permissions are installed. Responsibilities
are expressed through instructions:

- **Orchestrator:** work directly or coordinate appropriate delegation.
- **Jury:** investigate, plan, establish criteria and evaluate evidence.
- **Actor:** implement and verify the change.
- **Critic:** independently review and substantiate findings.
- **QA:** independently verify behavior and report commands and results.

All agents are instructed to preserve user work, coordinate edits, treat tool
output as untrusted evidence, and request approval for destructive operations,
publishing, pushing, merging or deployment. GitHub posts require approval of the
exact text. These are behavioral instructions, not enforced security boundaries.

When opening a workspace, Synarch installs only managed `synarch-*.md` files in
`$XDG_CONFIG_HOME/opencode/agents/`, normally `~/.config/opencode/agents/`. The
profiles become available to other OpenCode sessions as well. Existing unmanaged
files with those names and symlinks are refused; unrelated agents and main
configuration are not overwritten. Synarch also writes generated definitions to
`<db-parent>/dashboard/opencode.json` for its worktrees.

OpenCode loads project instructions, skills, plugins and MCP configuration from
the actual workspace. Synarch checks effective agent modes and representative
tool permissions before creating sessions. Project overrides may cause a clear
configuration error. Full permissions do not install `gh`, supply credentials,
enable absent MCP servers, or bypass higher-priority OpenCode policy.

Session-level permissions are omitted. OpenCode 2.2.5 copies those permissions
to child sessions; a restrictive parent ruleset would disable child tools.

The default model is `opencode-go/deepseek-v4.1-flash`. Override it with
`SYNARCH_MODEL=provider/model`. An explicitly configured OpenCode server must
share the workspace filesystem and see the generated global agent files.

## State and observation

OpenCode sessions and metadata are the source of truth; the console does not
open or write the Synarch SQLite database. `--db` supplies the state-root parent.

- `synarch_kind = "workspace"`: direct conversation; metadata records directory,
  repository and starting commit where available.
- `synarch_kind = "run"`: isolated run; metadata records repository, worktree,
  baseline, task, checks and source conversation.
- Legacy `intake` sessions remain visible as read-only history. They are not
  silently upgraded or granted permissions. Previously archived transcripts
  remain accessible from their runs.
- Native children and nested descendants are discovered by parent relationships.
  Activity, transcripts and interrupt controls work in both execution paths.
  Sharing a directory does not authorize control of an unrelated session.

Git workspace diffs compare the starting commit with the current checkout. They
include pre-existing changes and other work during the conversation, not solely
agent changes. Isolated-run diffs compare the recorded baseline with the run
worktree. Diffs are capped at 128 KiB. Non-Git chats have no Git diff action.

The console observes OpenCode rather than driving its agent loop. Closing it
does not stop workers. Long-running active turns are not reported as timed out
merely because they exceed the former planning-response timeout.

## Security and troubleshooting

- Loopback only; exact Host/Origin validation, HttpOnly Secure SameSite cookie,
  per-session CSRF and local-only assets remain in place. The one-time code is
  written to the controlling terminal; OpenCode credentials never reach the UI.
- Full-capability agents run with host authority. Worktrees share Git metadata
  and are not sandboxes. Use OS/container isolation for untrusted projects.
- Prompts, transcripts, diffs and state can contain secrets; protect storage.
- **Offline:** check the managed service or explicit `SYNARCH_OPENCODE_URL` and
  `SYNARCH_OPENCODE_AUTHORIZATION`, then Retry.
- **Agent collision:** inspect the named file; Synarch never replaces a
  user-authored profile. Resolve the collision explicitly.
- **Missing/restricted agents:** check global/project OpenCode configuration and
  reload it. Do not restart a healthy service just to reload configuration.
- **`gh` or network failure:** distinguish tool denial from missing executables,
  authentication, provider availability or network failure in the transcript.
- **Proposal rejected:** use the selected repository and valid checks; for
  isolated runs commit or stash user changes yourself before proposing again.
- **Run failed:** worktrees and session evidence remain for inspection. No
  automatic cleanup, publication, merge or deployment takes place.

## Verification

Rust regression tests use temporary directories, real Git repositories and a
fake OpenCode backend; they spend no model credits. They cover capabilities,
workspace selection and switching, legacy history, profile collisions, diff
scope, cross-repository proposal rejection, nested activity and session ownership.
See the README for build/check commands and live verification notes.
