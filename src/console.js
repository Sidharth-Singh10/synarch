(() => {
  "use strict";

  const csrfMeta = document.querySelector('meta[name="synarch-csrf"]');
  const csrf = csrfMeta ? csrfMeta.content : "";
  const $ = (id) => document.getElementById(id);
  const transcriptEl = $("transcript");
  const runListEl = $("run-list");
  const agentsListEl = $("agents-list");
  const composer = $("composer");
  const promptEl = $("prompt");
  const sendBtn = $("send");
  const interruptBtn = $("interrupt");
  const banner = $("banner");
  const bannerText = $("banner-text");
  const bannerRetry = $("banner-retry");
  const statusPill = $("status-pill");
  const statusText = $("status-text");
  const connectionEl = $("connection");
  const connectionText = $("connection-text");
  const titleEl = $("run-title");
  const metaEl = $("run-meta");
  const eyebrowEl = $("context-eyebrow");
  const runView = $("run-view");
  const agentsEl = $("agents");
  const agentsToggle = $("agents-toggle");
  const agentsClose = $("agents-close");
  const agentsCount = $("agents-count");
  const agentsToggleCount = $("agents-toggle-count");
  const newMessages = $("new-messages");
  const hintEl = $("composer-hint");
  const toastEl = $("toast");
  const newRunBtn = $("new-run");
  const lockBtn = $("lock");
  const tabChat = $("tab-chat");
  const tabRun = $("tab-run");
  const panelChat = $("panel-chat");
  const panelRun = $("panel-run");

  let state = null;
  let selected = localStorage.getItem("synarch-selected") || "";
  let inFlight = false;
  let pollTimer = null;
  let toastTimer = null;
  let stopping = new Set();
  let drawerClose = null;
  let drawerReturnFocus = null;

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = String(text);
    return node;
  }

  function clear(node) {
    node.replaceChildren();
  }

  function showToast(message) {
    toastEl.textContent = message;
    toastEl.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { toastEl.hidden = true; }, 4200);
  }

  function statusLabel(status) {
    return ({
      planning: "Planning",
      thinking: "Thinking",
      working: "Working",
      idle: "Idle",
      attention: "Needs attention",
    })[status] || "Ready";
  }

  function formatRelative(ms) {
    if (!ms) return "";
    const delta = Math.max(0, Date.now() - ms);
    if (delta < 30_000) return "just now";
    if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m ago`;
    if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)}h ago`;
    return `${Math.floor(delta / 86_400_000)}d ago`;
  }

  function formatSpan(startMs, endMs) {
    if (!startMs) return "";
    const seconds = Math.max(0, Math.floor(((endMs || Date.now()) - startMs) / 1000));
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
    return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
  }

  function shortId(id) {
    return id ? id.slice(0, 10) : "";
  }

  async function post(path, payload) {
    const body = new URLSearchParams({ ...payload, csrf });
    const response = await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body,
      credentials: "same-origin",
    });
    const data = await response.json().catch(() => ({}));
    if (response.status === 403) {
      window.location.href = "/";
      throw new Error("Session expired");
    }
    if (!response.ok || data.ok === false) {
      throw new Error(data.error || `Request failed (${response.status})`);
    }
    return data;
  }

  async function getJSON(path) {
    const response = await fetch(path, { credentials: "same-origin", cache: "no-store" });
    const data = await response.json().catch(() => ({}));
    if (response.status === 403) {
      window.location.href = "/";
      throw new Error("Session expired");
    }
    if (!response.ok) throw new Error(data.error || `Request failed (${response.status})`);
    return data;
  }

  async function refresh(immediate) {
    if (inFlight) return;
    inFlight = true;
    try {
      const query = selected ? `?session=${encodeURIComponent(selected)}` : "";
      const next = await getJSON(`/api/state${query}`);
      const wasOffline = state && !state.connected;
      state = next;
      if (next.active && next.active.session) {
        selected = next.active.session.id;
        localStorage.setItem("synarch-selected", selected);
      }
      render({ keepOffline: wasOffline && !next.connected });
    } catch (error) {
      if (state) {
        state.connected = false;
        state.error = error.message;
        render({ keepOffline: true });
      } else {
        setBanner(error.message, true);
        setConnection(false);
      }
    } finally {
      inFlight = false;
      schedule(immediate);
    }
  }

  function schedule(immediate) {
    clearTimeout(pollTimer);
    if (document.hidden) return;
    const live = state && state.connected && state.active &&
      (state.active.busy || ["thinking", "working"].includes(state.active.status) ||
        (state.active.children || []).some((child) => child.active));
    const delay = immediate ? 60 : live ? 1800 : 5000;
    pollTimer = setTimeout(() => refresh(false), delay);
  }

  function setBanner(message, retry) {
    if (!message) {
      banner.hidden = true;
      return;
    }
    bannerText.textContent = message;
    bannerRetry.hidden = !retry;
    banner.hidden = false;
  }

  function setConnection(connected) {
    connectionEl.classList.toggle("is-offline", !connected);
    connectionText.textContent = connected ? "Connected" : "Offline";
  }

  function render(options = {}) {
    if (!state) return;
    setConnection(state.connected);
    if (!state.connected) {
      setBanner(state.error || "OpenCode is not reachable. Start it, then retry.", true);
      if (options.keepOffline) {
        updateLiveBits();
        renderComposerState();
        return;
      }
    } else if (state.error) {
      setBanner(state.error, true);
    } else {
      setBanner("");
    }
    renderRuns();
    renderHeader();
    renderTranscript();
    renderAgents();
    renderRunView();
    renderComposerState();
  }

  function updateLiveBits() {
    if (state.active) {
      statusPill.dataset.status = state.active.status || "idle";
      statusText.textContent = statusLabel(state.active.status);
    }
  }

  function renderRuns() {
    clear(runListEl);
    const sessions = (state.sessions || []).slice();
    if (!sessions.length) {
      runListEl.appendChild(el("p", "rail-empty", "No runs yet. Start by telling the orchestrator what to build."));
      return;
    }
    for (const session of sessions) {
      const button = el("button", "run-item");
      button.type = "button";
      if (session.id === selected || (state.active && session.id === state.active.session.id)) {
        button.classList.add("is-active");
      }
      const dot = el("span", "dot run-dot");
      if (
        state.active &&
        session.id === state.active.session.id &&
        ["thinking", "working"].includes(state.active.status)
      ) {
        dot.classList.add("live");
      }
      const copy = el("span", "run-copy");
      copy.appendChild(el("span", "run-title", sessionTitle(session)));
      const sub = el("span", "run-sub", `${session.kind === "intake" ? "Planning" : "Run"} · ${formatRelative(session.updated || session.created)}`);
      copy.appendChild(sub);
      button.append(dot, copy);
      button.addEventListener("click", () => selectSession(session.id));
      runListEl.appendChild(button);
    }
  }

  function selectSession(id) {
    selected = id;
    localStorage.setItem("synarch-selected", id);
    closeDrawer();
    refresh(true);
  }

  function renderHeader() {
    const active = state.active;
    if (!active) {
      eyebrowEl.textContent = "Orchestrator";
      titleEl.textContent = "Talk to the orchestrator";
      metaEl.textContent = "Describe what you want built, fixed, or reviewed. I will inspect the repository and propose the run.";
      statusPill.dataset.status = "idle";
      statusText.textContent = "Ready";
      return;
    }
    const run = active.run;
    eyebrowEl.textContent = run ? "Confirmed run" : "Planning";
    titleEl.textContent = run ? sessionTitle(active.session) : "Talk to the orchestrator";
    if (run) {
      metaEl.textContent = `Repository ${run.repository} · Worktree ${run.worktree} · Baseline ${run.baseline.slice(0, 10)}`;
    } else {
      metaEl.textContent = `Planning workspace: ${active.session.workspace || "unknown"} · This is Synarch's control directory, not your repository.`;
    }
    statusPill.dataset.status = active.status || "idle";
    statusText.textContent = statusLabel(active.status);
  }

  function sessionTitle(session) {
    const title = String(session.title || "").replace(/^Synarch · /, "").trim();
    return title || "Untitled run";
  }

  function renderTranscript() {
    const active = state.active;
    const atBottom = transcriptEl.scrollHeight - transcriptEl.scrollTop - transcriptEl.clientHeight < 90;
    clear(transcriptEl);
    if (!active) {
      transcriptEl.appendChild(emptyState());
      newMessages.hidden = true;
      return;
    }
    const inner = el("div", "transcript-inner");
    if (active.run) {
      inner.appendChild(runStartLine(active));
    }
    if (active.kind === "intake" && !active.messages.length && !active.busy && !active.chat_error) {
      transcriptEl.appendChild(emptyState());
      newMessages.hidden = true;
      return;
    }
    if (!active.messages.length) {
      inner.appendChild(el("p", "muted", "No messages yet."));
    }
    for (const message of active.messages) {
      inner.appendChild(renderMessage(message));
    }
    if (active.busy || active.status === "thinking") {
      const thinking = el("div", "thinking-line");
      thinking.appendChild(el("span", "dot"));
      thinking.appendChild(el("span", "", "Orchestrator is working…"));
      inner.appendChild(thinking);
    }
    if (active.chat_error) {
      const error = el("div", "msg-error", active.chat_error);
      inner.appendChild(error);
    }
    if (active.inspection) {
      inner.appendChild(inspectionCard(active.inspection));
    }
    if (active.proposal) {
      inner.appendChild(proposalCard(active.proposal));
    }
    transcriptEl.appendChild(inner);
    if (atBottom) {
      transcriptEl.scrollTop = transcriptEl.scrollHeight;
      newMessages.hidden = true;
    } else if (active.messages.length) {
      newMessages.hidden = false;
    }
  }

  function runStartLine(active) {
    const line = el("div", "run-start");
    line.appendChild(el("span", "run-start-mark", "Run confirmed"));
    line.appendChild(el("span", "", ` ${active.run.repository}`));
    line.appendChild(el("span", "", ` · worktree ready · checks ${active.run.checks && active.run.checks.length ? active.run.checks.map((check) => check.name).join(", ") : "none"}`));
    return line;
  }

  function emptyState() {
    const empty = el("div", "empty");
    empty.appendChild(el("p", "eyebrow", "New run"));
    empty.appendChild(el("h2", "", "Tell me what to build."));
    empty.appendChild(el("p", "", "Give me a repository and a task. I will inspect the project, propose checks, and ask you to confirm before any subagent starts."));
    const workspace = state.active && state.active.session ? state.active.session.workspace : null;
    if (workspace) {
      const note = el("p", "workspace-note");
      note.appendChild(el("span", "", "Planning workspace: "));
      note.appendChild(el("code", "", workspace));
      note.appendChild(el("span", "", " — your repository is untouched until you confirm."));
      empty.appendChild(note);
    }
    const prompts = el("div", "prompts");
    const examples = [
      ["Fix the failing test", "Fix the failing test in this repository. Inspect it first and propose the run."],
      ["Review recent changes", "Review the most recent changes in this repository for correctness and missing tests."],
      ["Add a regression test", "Add a focused regression test for the reported bug and verify it."],
    ];
    for (const [label, text] of examples) {
      const chip = el("button", "prompt-chip", label);
      chip.type = "button";
      chip.addEventListener("click", () => sendText(text));
      prompts.appendChild(chip);
    }
    empty.appendChild(prompts);
    return empty;
  }

  function renderMessage(message) {
    const wrapper = el("article", `message message-${message.role}`);
    if (message.role === "user") {
      wrapper.appendChild(el("div", "bubble", message.text));
      return wrapper;
    }
    if (message.text) {
      wrapper.appendChild(el("div", "voice", message.text));
    }
    for (const tool of message.tools || []) {
      const line = el("div", "tool-line");
      line.appendChild(el("span", "tool-name", tool.name));
      if (tool.target) line.appendChild(el("span", "target", tool.target));
      if (tool.status) {
        const status = el("span", `tool-status${tool.status === "failed" ? " is-failed" : ""}`, tool.status);
        line.appendChild(status);
      }
      wrapper.appendChild(line);
    }
    if (message.error) wrapper.appendChild(el("div", "msg-error", message.error));
    if (message.role === "assistant") {
      wrapper.appendChild(el("div", "message-meta", message.at ? formatRelative(message.at) : ""));
    }
    return wrapper;
  }

  function proposalCard(proposal) {
    const card = el("section", "proposal");
    const head = el("div", "proposal-head");
    head.appendChild(el("h3", "", "Confirm this run"));
    head.appendChild(el("span", "proposal-tag", proposal.valid ? "Ready to start" : "Needs attention"));
    card.appendChild(head);
    const body = el("div", "proposal-body");
    body.appendChild(proposalRow("Repository", proposal.repository));
    body.appendChild(proposalRow("Task", proposal.task));
    const checks = el("div", "checks");
    if ((proposal.checks || []).length) {
      for (const check of proposal.checks) {
        const chip = el("div", "check-chip");
        chip.appendChild(el("span", "check-name", check.name));
        chip.appendChild(el("code", "", [check.argv[0], ...check.argv.slice(1)].join(" ")));
        checks.appendChild(chip);
      }
    } else {
      checks.appendChild(el("span", "muted", "No mandatory check proposed."));
    }
    body.appendChild(proposalRow("Checks", checks));
    body.appendChild(proposalRow("Agents", "Jury · Actor · Critic · QA"));
    body.appendChild(proposalRow("Isolation", "Fresh detached Git worktree; your checkout stays untouched."));
    if (proposal.warnings && proposal.warnings.length) {
      for (const warning of proposal.warnings) body.appendChild(el("p", "proposal-warn", warning));
    }
    if (proposal.error) body.appendChild(el("p", "proposal-error", proposal.error));
    card.appendChild(body);
    const actions = el("div", "proposal-actions");
    const start = el("button", "primary", "Start run");
    start.type = "button";
    start.disabled = !proposal.valid || Boolean(state.active && state.active.busy);
    start.addEventListener("click", () => startRun(proposal, start, actions));
    const change = el("button", "ghost", "Request changes");
    change.type = "button";
    change.addEventListener("click", () => {
      promptEl.value = "Change the proposal: ";
      promptEl.focus();
    });
    actions.append(start, change);
    card.appendChild(actions);
    return card;
  }

  function proposalRow(key, value) {
    const row = el("div", "proposal-row");
    row.appendChild(el("div", "proposal-key", key));
    const cell = el("div", "proposal-value");
    if (typeof value === "string") cell.textContent = value;
    else cell.appendChild(value);
    row.appendChild(cell);
    return row;
  }

  async function startRun(proposal, button, actions) {
    if (!state.active) return;
    button.disabled = true;
    const old = button.textContent;
    button.textContent = "Starting…";
    const note = el("span", "starting", "Creating worktree and preparing agents.");
    actions.appendChild(note);
    try {
      const result = await post("/api/start", {
        session: state.active.session.id,
        message: proposal.message_id,
      });
      selected = result.session || state.active.session.id;
      localStorage.setItem("synarch-selected", selected);
      await refresh(true);
    } catch (error) {
      button.disabled = false;
      button.textContent = old;
      note.remove();
      const message = el("div", "msg-error", error.message);
      actions.parentElement.appendChild(message);
    }
  }

  function inspectionCard(inspection) {
    const card = el("section", "proposal");
    const head = el("div", "proposal-head");
    head.appendChild(el("h3", "", "Repository inspection"));
    head.appendChild(el("span", "proposal-tag", inspection.error ? "Unavailable" : inspection.clean ? "Clean" : "Dirty"));
    card.appendChild(head);
    const body = el("div", "proposal-body");
    body.appendChild(proposalRow("Path", inspection.path));
    if (inspection.error) {
      body.appendChild(el("p", "proposal-error", inspection.error));
    } else {
      body.appendChild(proposalRow("HEAD", inspection.head || "unknown"));
      body.appendChild(proposalRow("Branch", inspection.branch || "detached"));
      body.appendChild(proposalRow("Working tree", inspection.clean ? "Clean" : `${inspection.dirty_count} changed path(s)`));
      body.appendChild(proposalRow("Recent commit", inspection.recent_commit || "unknown"));
      body.appendChild(proposalRow("Manifests", (inspection.manifests || []).join(", ") || "none detected"));
      const suggestions = el("div", "checks");
      for (const check of inspection.suggestions || []) {
        const chip = el("div", "check-chip");
        chip.appendChild(el("span", "check-name", check.name));
        chip.appendChild(el("code", "", check.argv.join(" ")));
        suggestions.appendChild(chip);
      }
      body.appendChild(proposalRow("Suggested checks", suggestions.childElementCount ? suggestions : el("span", "muted", "None detected.")));
    }
    card.appendChild(body);
    if (!inspection.error && inspection.suggestions && inspection.suggestions.length) {
      const actions = el("div", "proposal-actions");
      const use = el("button", "ghost", "Use suggested checks");
      use.type = "button";
      use.addEventListener("click", () => {
        const commands = inspection.suggestions.map((check) => `${check.name}: ${check.argv.join(" ")}`).join("; ");
        promptEl.value = `Use these checks for the run: ${commands}`;
        promptEl.focus();
      });
      actions.appendChild(use);
      card.appendChild(actions);
    }
    return card;
  }

  function renderAgents() {
    const active = state.active;
    clear(agentsListEl);
    if (!active) {
      agentsListEl.appendChild(el("p", "agents-empty", "Subagents appear here once a run starts."));
      agentsCount.textContent = "0";
      agentsToggleCount.textContent = "0";
      return;
    }
    const children = active.children || [];
    const count = 1 + children.length;
    agentsCount.textContent = String(count);
    agentsToggleCount.textContent = String(count);
    agentsListEl.appendChild(orchestratorCard(active));
    for (const child of children) agentsListEl.appendChild(agentCard(child, active));
    if (!children.length) {
      agentsListEl.appendChild(el("p", "agents-empty", active.run ? "No subagents have been launched yet." : "Confirm the run to let the orchestrator launch subagents."));
    }
  }

  function agentStateClass(active, outcome) {
    if (active) return "is-active";
    if (outcome === "failed") return "is-failed";
    if (outcome === "succeeded") return "is-done";
    return "";
  }

  function orchestratorCard(active) {
    const live = active.busy || active.status === "thinking";
    const card = el("article", `agent ${agentStateClass(live, null)}`);
    const role = el("div", "agent-role");
    role.appendChild(el("strong", "", "Orchestrator"));
    role.appendChild(el("span", "agent-state", live ? "running" : active.run ? "idle" : "planning"));
    card.appendChild(role);
    card.appendChild(el("p", "agent-task", active.run ? active.run.task : "Planning a new run"));
    const now = el("div", "agent-now");
    now.appendChild(el("span", "tool-name", live ? "now" : "state"));
    now.appendChild(el("span", "target", live ? "coordinating the run" : statusLabel(active.status)));
    card.appendChild(now);
    const meta = el("div", "agent-meta");
    meta.appendChild(el("span", "", active.session.id ? shortId(active.session.id) : ""));
    if (active.run) meta.appendChild(el("span", "", `${(active.children || []).length} subagent${(active.children || []).length === 1 ? "" : "s"}`));
    card.appendChild(meta);
    const actions = el("div", "agent-actions");
    if (active.run) {
      const transcript = el("button", "agent-link", "Transcript");
      transcript.type = "button";
      transcript.addEventListener("click", () => openTranscript(active.session.id, active.session.id, "Orchestrator"));
      actions.appendChild(transcript);
    }
    if (live) {
      const stop = stopButton(active.session.id, "Stop");
      actions.appendChild(stop);
    }
    card.appendChild(actions);
    return card;
  }

  function agentCard(child, active) {
    const card = el("article", `agent ${agentStateClass(child.active, child.outcome)}`);
    const role = el("div", "agent-role");
    role.appendChild(el("strong", "", child.role || child.agent));
    role.appendChild(el("span", "agent-state", child.active ? "running" : child.outcome || "idle"));
    card.appendChild(role);
    card.appendChild(el("p", "agent-task", child.title || child.task || "Subagent"));
    if (child.tool || child.text) {
      const now = el("div", "agent-now");
      now.appendChild(el("span", "tool-name", child.tool || "last"));
      now.appendChild(el("span", "target", child.target || child.text || ""));
      card.appendChild(now);
    }
    const meta = el("div", "agent-meta");
    if (child.model) meta.appendChild(el("span", "", child.model));
    if (child.created) {
      const end = child.active ? Date.now() : child.updated || child.last_at;
      meta.appendChild(el("span", "", formatSpan(child.created, end)));
    }
    card.appendChild(meta);
    const actions = el("div", "agent-actions");
    const transcript = el("button", "agent-link", "Transcript");
    transcript.type = "button";
    transcript.addEventListener("click", () => openTranscript(active.session.id, child.session_id, child.role || child.agent));
    actions.appendChild(transcript);
    if (child.active) actions.appendChild(stopButton(child.session_id, "Stop"));
    card.appendChild(actions);
    return card;
  }

  function stopButton(sessionId, label) {
    const button = el("button", "agent-link danger", label);
    button.type = "button";
    button.dataset.session = sessionId;
    button.addEventListener("click", async () => {
      if (!stopping.has(sessionId)) {
        stopping.add(sessionId);
        button.textContent = "Confirm";
        setTimeout(() => {
          stopping.delete(sessionId);
          if (button.isConnected) button.textContent = label;
        }, 4000);
        return;
      }
      stopping.delete(sessionId);
      button.disabled = true;
      button.textContent = "Stopping…";
      try {
        await post("/api/interrupt", { session: sessionId });
        await refresh(true);
      } catch (error) {
        showToast(error.message);
        button.disabled = false;
        button.textContent = label;
      }
    });
    return button;
  }

  function renderComposerState() {
    const active = state.active;
    const connected = state.connected;
    const busy = Boolean(active && (active.busy || active.status === "thinking"));
    const anyActive = Boolean(active && (active.active_sessions || []).length);
    sendBtn.disabled = !connected || busy;
    promptEl.disabled = !connected;
    interruptBtn.hidden = !(busy || anyActive);
    interruptBtn.disabled = !anyActive && !busy;
    if (!connected) hintEl.textContent = "OpenCode is offline. Start it, then retry.";
    else if (busy) hintEl.textContent = "The orchestrator is working. You can interrupt it.";
    else if (!active) hintEl.textContent = "Enter to send · Shift+Enter for a new line";
    else if (active.kind === "intake") hintEl.textContent = "Confirm the proposal to start the run.";
    else hintEl.textContent = "Enter to send · Shift+Enter for a new line";
  }

  function renderRunView() {
    clear(runView);
    const active = state.active;
    if (!active) {
      runView.appendChild(el("p", "muted", "Run details appear after you confirm a plan."));
      return;
    }
    const run = active.run;
    if (!run) {
      const section = el("section", "run-section");
      section.appendChild(el("h2", "", "Planning session"));
      section.appendChild(el("p", "run-task", active.session.title));
      section.appendChild(el("p", "notice", "No run has started. Confirm the proposal in the conversation to create an isolated worktree and launch subagents."));
      const detail = el("dl", "kv");
      kvPair(detail, "Planning workspace", active.session.workspace);
      kvPair(detail, "Repository", "not selected yet");
      section.appendChild(detail);
      runView.appendChild(section);
      return;
    }
    const taskSection = el("section", "run-section");
    taskSection.appendChild(el("h2", "", "Task"));
    taskSection.appendChild(el("p", "run-task", run.task));
    runView.appendChild(taskSection);

    const detail = el("section", "run-section");
    detail.appendChild(el("h2", "", "Run"));
    const kv = el("dl", "kv");
    kvPair(kv, "Repository", run.repository);
    kvPair(kv, "Worktree", run.worktree);
    kvPair(kv, "Baseline", run.baseline);
    kvPair(kv, "Status", statusLabel(active.status));
    kvPair(kv, "Session", active.session.id);
    kvPair(kv, "Checks", run.checks && run.checks.length ? run.checks.map((check) => `${check.name}: ${check.argv.join(" ")}`) : "none");
    detail.appendChild(kv);
    runView.appendChild(detail);

    const stages = el("section", "run-section");
    stages.appendChild(el("h2", "", "Stages"));
    const grid = el("div", "stages");
    for (const stage of active.stages || []) {
      const cell = el("div", "stage");
      cell.dataset.state = stage.state;
      cell.appendChild(el("div", "stage-label", stage.role));
      const stateLine = el("div", "stage-state");
      stateLine.appendChild(el("span", "dot"));
      stateLine.appendChild(el("span", "", stage.state === "working" ? "Working" : stage.state === "done" ? "Done" : stage.state === "failed" ? "Failed" : "Not used"));
      cell.appendChild(stateLine);
      if (stage.session) {
        const link = el("button", "agent-link", shortId(stage.session));
        link.type = "button";
        link.classList.add("stage-link");
        link.addEventListener("click", () => openTranscript(active.session.id, stage.session, stage.role));
        cell.appendChild(link);
      }
      grid.appendChild(cell);
    }
    stages.appendChild(grid);
    const actions = el("div", "run-actions");
    const diff = el("button", "primary", "View changes");
    diff.type = "button";
    diff.addEventListener("click", () => openDiff(active.session.id));
    actions.appendChild(diff);
    const copy = el("button", "ghost", "Copy session ID");
    copy.type = "button";
    copy.addEventListener("click", () => copyText(active.session.id));
    actions.appendChild(copy);
    if (run.intake_session) {
      const planning = el("button", "ghost", "Planning transcript");
      planning.type = "button";
      planning.addEventListener("click", () => openTranscript(run.intake_session, run.intake_session, "Planning"));
      actions.appendChild(planning);
    }
    if ((active.active_sessions || []).length) {
      const stopAll = el("button", "ghost", "Stop all active");
      stopAll.type = "button";
      stopAll.addEventListener("click", () => stopAllActive(active));
      actions.appendChild(stopAll);
    }
    stages.appendChild(actions);
    runView.appendChild(stages);

    const children = el("section", "run-section");
    children.appendChild(el("h2", "", "Subagents"));
    if (!(active.children || []).length) {
      children.appendChild(el("p", "muted", "No subagents have been launched yet."));
    } else {
      const kv = el("dl", "kv");
      for (const child of active.children) {
        kvPair(kv, child.role || child.agent, `${child.title || "Subagent"} · ${child.active ? "running" : child.outcome || "idle"}`);
      }
      children.appendChild(kv);
    }
    runView.appendChild(children);
  }

  function kvPair(dl, key, value) {
    const item = el("div");
    item.appendChild(el("dt", "", key));
    const dd = el("dd");
    dd.textContent = value || "—";
    item.appendChild(dd);
    dl.appendChild(item);
  }

  async function stopAllActive(active) {
    const ids = (active.active_sessions || []).filter((id) =>
      id === active.session.id || (active.children || []).some((child) => child.session_id === id));
    for (const id of ids) {
      try { await post("/api/interrupt", { session: id }); } catch (error) { showToast(error.message); }
    }
    await refresh(true);
  }

  async function copyText(text) {
    try {
      await navigator.clipboard.writeText(text);
      showToast("Copied");
    } catch {
      showToast(text);
    }
  }

  function openDrawer(title, sub, loader) {
    closeDrawer();
    drawerReturnFocus = document.activeElement;
    const root = $("drawer-root");
    const backdrop = el("div", "drawer-backdrop");
    const drawer = el("div", "drawer");
    drawer.setAttribute("role", "dialog");
    drawer.setAttribute("aria-modal", "true");
    drawer.setAttribute("aria-label", title);
    const head = el("div", "drawer-head");
    const copy = el("div");
    copy.appendChild(el("h2", "", title));
    if (sub) copy.appendChild(el("p", "drawer-sub", sub));
    head.appendChild(copy);
    const close = el("button", "drawer-close", "×");
    close.type = "button";
    close.setAttribute("aria-label", "Close");
    head.appendChild(close);
    const body = el("div", "drawer-body");
    body.appendChild(el("p", "drawer-empty", "Loading…"));
    drawer.append(head, body);
    root.append(backdrop, drawer);
    document.body.classList.add("drawer-open");
    $("app").inert = true;
    const closeFn = () => closeDrawer();
    close.addEventListener("click", closeFn);
    backdrop.addEventListener("click", closeFn);
    drawer.addEventListener("keydown", (event) => {
      if (event.key !== "Tab") return;
      const focusable = [...drawer.querySelectorAll('button, [href], input, textarea, select, [tabindex]:not([tabindex="-1"])')].filter((node) => !node.disabled);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    });
    drawerClose = closeFn;
    close.focus();
    loader(body).catch((error) => {
      clear(body);
      body.appendChild(el("p", "proposal-error", error.message));
    });
  }

  function closeDrawer() {
    const root = $("drawer-root");
    clear(root);
    document.body.classList.remove("drawer-open");
    $("app").inert = false;
    drawerClose = null;
    if (drawerReturnFocus && drawerReturnFocus.isConnected) {
      drawerReturnFocus.focus();
    }
    drawerReturnFocus = null;
  }

  async function openTranscript(root, session, label) {
    openDrawer(`${label} transcript`, session, async (body) => {
      const data = await getJSON(`/api/transcript?root=${encodeURIComponent(root)}&session=${encodeURIComponent(session)}`);
      clear(body);
      if (!data.messages || !data.messages.length) {
        body.appendChild(el("p", "muted", "No messages recorded yet."));
        return;
      }
      for (const message of data.messages) {
        const item = el("article", `transcript-msg is-${message.role}`);
        item.appendChild(el("div", "who", message.role === "user" ? "Task" : "Agent"));
        if (message.text) item.appendChild(el("div", "body", message.text));
        for (const tool of message.tools || []) {
          const toolBox = el("div", "transcript-tool");
          toolBox.appendChild(el("div", "cmd", `${tool.name} ${tool.target || ""}`.trim()));
          if (tool.output) toolBox.appendChild(el("div", "out", tool.output));
          item.appendChild(toolBox);
        }
        body.appendChild(item);
      }
    });
  }

  async function openDiff(session) {
    openDrawer("Changes in the run worktree", session, async (body) => {
      const data = await getJSON(`/api/diff?session=${encodeURIComponent(session)}`);
      clear(body);
      const meta = el("div", "diff-meta");
      meta.appendChild(el("span", "", `HEAD ${String(data.head || "").slice(0, 10)}`));
      meta.appendChild(el("span", "", data.dirty ? `${data.dirty_count} uncommitted path(s)` : "clean working tree"));
      meta.appendChild(el("span", "", "Baseline to working tree"));
      body.appendChild(meta);
      if (!data.diff || !String(data.diff).trim()) {
        body.appendChild(el("p", "muted", "No changes since the baseline."));
      } else {
        body.appendChild(el("pre", "diff-pre", data.diff));
      }
    });
  }

  function sendText(text) {
    if (!state || !state.connected) {
      showToast("OpenCode is offline. Start it, then retry.");
      return;
    }
    if (state.active && (state.active.busy || state.active.status === "thinking")) {
      showToast("The orchestrator is still working. Interrupt it first.");
      return;
    }
    promptEl.value = text;
    composer.requestSubmit();
  }

  async function submitComposer(event) {
    event.preventDefault();
    const text = promptEl.value.trim();
    if (!text) return;
    if (!state || !state.connected) {
      showToast("OpenCode is offline. Start it, then retry.");
      return;
    }
    if (state.active && (state.active.busy || state.active.status === "thinking")) {
      showToast("The orchestrator is still working. Interrupt it first.");
      return;
    }
    const session = state.active ? state.active.session.id : "";
    promptEl.value = "";
    sendBtn.disabled = true;
    const previous = sendBtn.textContent;
    sendBtn.textContent = "Sending…";
    try {
      const result = await post("/api/chat", { session, text });
      if (result.session) {
        selected = result.session;
        localStorage.setItem("synarch-selected", selected);
      }
      await refresh(true);
    } catch (error) {
      showToast(error.message);
      promptEl.value = text;
      } finally {
      sendBtn.textContent = previous;
      sendBtn.disabled = false;
    }
  }

  function setTab(name) {
    const chat = name === "chat";
    tabChat.classList.toggle("is-active", chat);
    tabRun.classList.toggle("is-active", !chat);
    tabChat.setAttribute("aria-selected", String(chat));
    tabRun.setAttribute("aria-selected", String(!chat));
    panelChat.hidden = !chat;
    panelRun.hidden = chat;
  }

  function toggleAgents(open) {
    const mobile = window.innerWidth <= 900;
    agentsEl.classList.toggle("is-open", open);
    agentsToggle.setAttribute("aria-expanded", String(open));
    agentsEl.inert = !open && mobile;
    $("agents-backdrop").hidden = !open || !mobile;
    $("workspace").inert = open && mobile;
    document.querySelector(".rail").inert = open && mobile;
    document.body.classList.toggle("agents-open", open && mobile);
    if (open) agentsClose.focus();
  }

  function syncAgentsMode() {
    if (window.innerWidth > 900) {
      agentsEl.classList.remove("is-open");
      agentsToggle.setAttribute("aria-expanded", "false");
      agentsEl.inert = false;
      $("agents-backdrop").hidden = true;
      $("workspace").inert = false;
      document.querySelector(".rail").inert = false;
      document.body.classList.remove("agents-open");
    } else if (!agentsEl.classList.contains("is-open")) {
      agentsEl.inert = true;
      $("agents-backdrop").hidden = true;
      $("workspace").inert = false;
      document.querySelector(".rail").inert = false;
      document.body.classList.remove("agents-open");
    }
  }

  newRunBtn.addEventListener("click", async () => {
    newRunBtn.disabled = true;
    try {
      const result = await post("/api/new", {});
      if (result.session) {
        selected = result.session;
        localStorage.setItem("synarch-selected", selected);
        setTab("chat");
        await refresh(true);
        promptEl.focus();
      }
    } catch (error) {
      showToast(error.message);
    } finally {
      newRunBtn.disabled = false;
    }
  });

  lockBtn.addEventListener("click", async () => {
    try {
      await post("/api/logout", {});
    } finally {
      window.location.href = "/";
    }
  });

  composer.addEventListener("submit", submitComposer);
  promptEl.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      composer.requestSubmit();
    }
  });
  interruptBtn.addEventListener("click", async () => {
    if (!state.active) return;
    const ids = (state.active.active_sessions || []).filter((id) =>
      id === state.active.session.id || (state.active.children || []).some((child) => child.session_id === id));
    if (!ids.length) return;
    interruptBtn.disabled = true;
    interruptBtn.textContent = "Stopping…";
    for (const id of ids) {
      try { await post("/api/interrupt", { session: id }); } catch (error) { showToast(error.message); }
    }
    interruptBtn.textContent = "Interrupt";
    await refresh(true);
  });
  bannerRetry.addEventListener("click", () => refresh(true));
  tabChat.addEventListener("click", () => setTab("chat"));
  tabRun.addEventListener("click", () => setTab("run"));
  agentsToggle.addEventListener("click", () => toggleAgents(true));
  agentsClose.addEventListener("click", () => toggleAgents(false));
  $("agents-backdrop").addEventListener("click", () => toggleAgents(false));
  newMessages.addEventListener("click", () => {
    transcriptEl.scrollTop = transcriptEl.scrollHeight;
    newMessages.hidden = true;
  });
  transcriptEl.addEventListener("scroll", () => {
    if (transcriptEl.scrollHeight - transcriptEl.scrollTop - transcriptEl.clientHeight < 90) {
      newMessages.hidden = true;
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      if (drawerClose) closeDrawer();
      else if (agentsEl.classList.contains("is-open")) toggleAgents(false);
    }
  });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) refresh(true);
  });
  window.addEventListener("resize", syncAgentsMode);

  syncAgentsMode();
  refresh(true);
})();
