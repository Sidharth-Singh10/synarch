# Capability map

Synarch has two local modes. The protected orchestrator console is the
conversational native-subagent adapter; the evidence-gated controller remains a
separate CLI workflow. Both are local-first and single-user.

| Module id | Responsibility | Depends on |
| --- | --- | --- |
| local-access | Authorize browser access and protect reads and writes | — |
| console-config | Private dashboard state root and generated Synarch agents | local-access |
| workspace-chat | Dashboard workspace selection, full-capability orchestrator, direct work and delegation | local-access, console-config |
| native-runs | Optional confirmed worktree run, native subagents, activity, transcripts, diff, interrupt | workspace-chat |
| gated-controller | Strict Jury/Actor/Critic/QA pipeline, checks, ledger, recovery (CLI) | — |
| read-only-dashboard | Inspect persisted gated runs from SQLite | gated-controller |

The console does not use the gated controller's acceptance gates. Users select a
workspace before chatting; ordinary tasks run directly there. All console roles
have full tools and instruction-based responsibilities. Only optional isolated
worktree runs require proposal confirmation. Native subagent activity, transcripts,
diffs and interrupts work in either path. There is no hard token/cost cap or
role-based security boundary. Worktree cleanup, automatic merge/deploy, and
OS-level isolation remain outside this initiative.
