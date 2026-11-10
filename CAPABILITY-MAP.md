# Capability map

Synarch is two products. This repository is the evidence-gated controller and
its read-only ledger dashboard. The interactive orchestrator is a separate
OpenCode plugin (`opencode-synarch`) that runs inside the OpenCode TUI and is not
implemented here.

| Module id | Responsibility | Depends on |
| --- | --- | --- |
| gated-controller | Strict Jury/Actor/Critic/QA pipeline, checks, ledger, recovery (CLI) | — |
| read-only-dashboard | Inspect persisted gated runs from SQLite | gated-controller |

The controller owns every transition. Role selection, worktree isolation,
deterministic argv checks and the acceptance gates are enforced in Rust; model
text is untrusted structured input. It has no token or cost cap beyond the
configured budgets and no automatic merge, push, deploy or worktree cleanup.

The OpenCode plugin gives the conversational, full-capability workflow: a
`synarch` agent, build-capable delegated workers, optional isolated worktree
runs, and live agent activity in the TUI sidebar. It deliberately has no
Rust-owned acceptance gate, so the controller remains the choice when acceptance
must be mechanically enforced.
