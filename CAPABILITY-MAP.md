# Capability map

Synarch has two local modes. The protected orchestrator console is the
conversational native-subagent adapter; the evidence-gated controller remains a
separate CLI workflow. Both are local-first and single-user.

| Module id | Responsibility | Depends on |
| --- | --- | --- |
| local-access | Authorize browser access and protect reads and writes | — |
| console-config | Private dashboard state root and generated Synarch agents | local-access |
| orchestrator-planning | Intake session, read-only inspection, proposal, explicit confirmation | local-access, console-config |
| native-runs | Worktree, run orchestrator, native subagents, live activity, transcripts, diff, interrupt | orchestrator-planning |
| gated-controller | Strict Jury/Actor/Critic/QA pipeline, checks, ledger, recovery (CLI) | — |
| read-only-dashboard | Inspect persisted gated runs from SQLite | gated-controller |

The console does not use the gated controller's acceptance gates. It starts no
work until the user confirms a proposal, isolates changes in a detached worktree,
and shows the native subagents and their tool activity. Parallel native subagents
are allowed; there is no hard token or cost cap in this mode. Worktree cleanup,
automatic merge/deploy, and OS-level isolation remain outside this initiative.
