import { AlertTriangle, Bot, Copy, KeyRound, Plug, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { api } from "../api";
import { ConceptHelp } from "../BeginnerHelp";
import { connectorApi } from "../connectorApi";
import type {
  AgentActionRequest,
  ResolveInputs,
  AutomationActivityEntry,
  AutomationAgentSummary,
  AutomationCredential,
  AutomationStatus,
  ConnectedAppStatus
} from "../connectorTypes";
import type { ProjectSummary } from "../types";
import { HelpPopover, SectionTitle } from "../ui";

async function requireConnectorApi() {
  return connectorApi;
}

function LocalAutomationHelp() {
  return (
    <HelpPopover title="Local automation" compact>
      <p>Local automation lets another program on this computer ask Code Hangar for specific information. The local endpoint is the private address it uses; a token is its one-time password.</p>
      <p>Permissions decide which projects and actions that program can request. Start with the smallest set, and revoke the program when it no longer needs access.</p>
    </HelpPopover>
  );
}

export type IntegrationAvailabilityState = "loading" | "error" | "not-compiled" | "ready";

export function automationIntegrationAvailabilityState(
  status: Pick<AutomationStatus, "enabled"> | null,
  error: string | null
): IntegrationAvailabilityState {
  if (status?.enabled) return "ready";
  if (status) return "not-compiled";
  return error ? "error" : "loading";
}

export function connectedAppsIntegrationAvailabilityState(
  loaded: boolean,
  loadError: string | null,
  hostCount: number
): IntegrationAvailabilityState {
  if (!loaded) return "loading";
  if (loadError) return "error";
  return hostCount > 0 ? "ready" : "not-compiled";
}

function IntegrationAvailabilityPanel({
  title,
  icon,
  state,
  error,
  loadingMessage,
  notCompiledMessage,
  onRetry
}: {
  title: string;
  icon: ReactNode;
  state: Exclude<IntegrationAvailabilityState, "ready">;
  error?: string | null;
  loadingMessage: string;
  notCompiledMessage: string;
  onRetry: () => void;
}) {
  const heading = state === "loading"
    ? "Checking integration"
    : state === "error"
      ? "Integration status unavailable"
      : "Not compiled into this build";
  const message = state === "loading"
    ? loadingMessage
    : state === "error"
      ? error ?? "The integration status could not be read."
      : notCompiledMessage;
  return (
    <section className="pane-section compact integration-availability" aria-busy={state === "loading"}>
      <SectionTitle icon={icon} label={title} />
      <div
        className={`dashboard-card integration-availability-card ${state === "error" ? "warning-card" : ""}`}
        data-state={state}
        role={state === "error" ? "alert" : "status"}
        aria-live="polite"
      >
        <h3>{heading}</h3>
        <p>{message}</p>
        {state === "error" ? (
          <button className="secondary-button" type="button" onClick={onRetry}>Retry status check</button>
        ) : null}
      </div>
    </section>
  );
}

const AUTOMATION_SCOPE_OPTIONS = [
  { id: "read_structure", label: "Project structure", help: "Read project and context-file metadata, never file bodies." },
  { id: "read_graph", label: "Dependency graph & cleanup", help: "Read the project graph, node relationships, and orphan/duplicate candidates. Structure only — never file bodies." },
  { id: "read_body", label: "Temporary file bodies (trusted local IPC only)", help: "Named-pipe tools can request non-sensitive file bodies inside selected projects. Claude/Cursor/Codex MCP connections never receive this scope or a body tool." },
  { id: "comments_read", label: "Read comments", help: "List the comments on selected projects, folders and files." },
  { id: "comments_write", label: "Write its own comments", help: "Add and edit only its OWN comments. Also needs the global AI write toggle; it can never touch a comment you wrote." },
  { id: "build_plan", label: "Build impact previews", help: "Build read-only OperationPlan and Risk Report previews for selected projects." },
  { id: "execute_plan", label: "Request safe actions", help: "Request verified backup or holding-area moves. A fresh human confirmation token remains mandatory." },
  { id: "history_search", label: "Search project sessions", help: "Run bounded, redacted, on-demand history search for an explicitly selected project." }
] as const;

export function SettingsAutomationView({
  status,
  agents,
  activity,
  credential,
  projects,
  currentFile,
  busy,
  error,
  onRefresh,
  onRegister,
  onRevoke,
  onForget,
  onGrantRead,
  onCopy,
  onClearCredential
}: {
  status: AutomationStatus | null;
  agents: AutomationAgentSummary[];
  activity: AutomationActivityEntry[];
  credential: AutomationCredential | null;
  projects: ProjectSummary[];
  currentFile: { nodeId: number; displayName: string } | null;
  busy: boolean;
  error: string | null;
  onRefresh: () => void;
  onRegister: (name: string, scopes: string[], projectIds: number[]) => void;
  onRevoke: (agentId: number) => void;
  onForget: (agentId: number) => void;
  onGrantRead: (agentId: number, nodeId: number) => void;
  onCopy: (value: string) => void;
  onClearCredential: () => void;
}) {
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<string[]>(["read_structure"]);
  const [projectIds, setProjectIds] = useState<number[]>([]);
  const [grantAgentId, setGrantAgentId] = useState<number | null>(null);

  useEffect(() => {
    const firstEnabled = agents.find((agent) => agent.enabled)?.id ?? null;
    if (grantAgentId === null || !agents.some((agent) => agent.id === grantAgentId && agent.enabled)) {
      setGrantAgentId(firstEnabled);
    }
  }, [agents, grantAgentId]);

  const toggleScope = (scope: string) => {
    setScopes((current) => current.includes(scope) ? current.filter((item) => item !== scope) : [...current, scope]);
  };
  const toggleProject = (projectId: number) => {
    setProjectIds((current) => current.includes(projectId) ? current.filter((item) => item !== projectId) : [...current, projectId]);
  };

  const availability = automationIntegrationAvailabilityState(status, error);
  if (availability !== "ready" || status === null) {
    return (
      <IntegrationAvailabilityPanel
        title="Local automation"
        icon={<Bot size={15} />}
        state={availability === "ready" ? "error" : availability}
        error={error}
        loadingMessage="Reading the local automation capability from this executable."
        notCompiledMessage={status?.message ?? "Local automation is not compiled into this executable."}
        onRetry={onRefresh}
      />
    );
  }

  return (
    <section className="pane-section compact automation-settings">
      <SectionTitle icon={<Bot size={15} />} label="Local automation" trailing={<LocalAutomationHelp />} />
      <p className="muted help-copy">Optional advanced integration for local tools. It uses an authenticated Windows named pipe, never an external network listener. Every agent is limited to explicit projects and scopes.</p>
      <>
          <div className="dashboard-card" data-help="This endpoint is a Windows named pipe restricted to local-machine clients. Every non-status request also needs a registered token.">
            <div className="card-title-row">
              <h3>Local endpoint</h3>
              <button type="button" className="icon-button" aria-label="Copy local endpoint" onClick={() => status.endpoint && onCopy(status.endpoint)} data-help="Copy the local named-pipe endpoint for configuring a trusted local tool."><Copy size={15} /></button>
            </div>
            <code className="path-code">{status.endpoint}</code>
            <p className="muted help-copy">Protocol {status.protocol}. Guest requests see capabilities only; project data always requires authentication.</p>
          </div>

          {credential ? (
            <div className="dashboard-card warning-card" data-help="The raw token is shown once. Code Hangar stores only its hash; closing this card cannot be undone except by registering a new token.">
              <div className="card-title-row"><h3>New credential: {credential.agent.name}</h3><KeyRound size={16} /></div>
              <p>Store this token in the local tool now. It will not be shown again.</p>
              <code className="credential-token">{credential.token}</code>
              <div className="inline-actions">
                <button type="button" onClick={() => onCopy(credential.token)} data-help="Copy the one-time local authentication token."><Copy size={14} /> Copy token</button>
                <button type="button" className="secondary-button" onClick={onClearCredential} data-help="Hide this one-time token. Register a new credential if it is lost.">I stored it</button>
              </div>
            </div>
          ) : null}

          <div className="dashboard-card" data-help="Register a local tool with the smallest scopes and project set it needs. The token is generated locally and displayed once.">
            <h3>Register a local tool</h3>
            <label className="field-label">
              Name
              <input value={name} maxLength={80} onChange={(event) => setName(event.target.value)} placeholder="Example: local indexing helper" />
            </label>
            <div className="automation-choice-list">
              <strong>Allowed capabilities</strong>
              {AUTOMATION_SCOPE_OPTIONS.map((scope) => (
                <label className="toggle-row" key={scope.id} data-help={scope.help}>
                  <input type="checkbox" checked={scopes.includes(scope.id)} onChange={() => toggleScope(scope.id)} />
                  <span><strong>{scope.label}</strong><small>{scope.help}</small></span>
                </label>
              ))}
            </div>
            <div className="automation-choice-list project-scope-list">
              <strong>Allowed projects</strong>
              {projects.map((project) => (
                <label className="toggle-row" key={project.id} data-help={`Allow this credential to address only project ${project.name}.`}>
                  <input type="checkbox" checked={projectIds.includes(project.id)} onChange={() => toggleProject(project.id)} />
                  <span><strong>{project.name}</strong><small>{project.path}</small></span>
                </label>
              ))}
            </div>
            <button type="button" disabled={busy || !name.trim() || scopes.length === 0 || projectIds.length === 0} onClick={() => onRegister(name, scopes, projectIds)} data-help="Generate one local token for exactly these scopes and projects. No remote account is created.">
              <ShieldCheck size={15} /> Register and show token once
            </button>
          </div>

          <div className="dashboard-card" data-help="Revoking a credential immediately blocks its token and revokes all temporary read grants.">
            <div className="card-title-row"><h3>Registered local tools</h3><button type="button" className="secondary-button slim" disabled={busy} onClick={onRefresh}>Refresh</button></div>
            {agents.length === 0 ? <p className="muted">No local tools registered.</p> : agents.map((agent) => (
              <div className="automation-agent-row" key={agent.id}>
                <div><strong>{agent.name}</strong><small>{agent.enabled ? agent.scopes.join(" · ") : "Revoked"}</small></div>
                {agent.enabled ? <button type="button" className="danger-button slim" disabled={busy} onClick={() => onRevoke(agent.id)} data-help={`Revoke ${agent.name}, its token and all temporary file grants.`}>Revoke</button> : <button type="button" className="secondary-button slim" disabled={busy} onClick={() => onForget(agent.id)} data-help={`Remove the revoked ${agent.name} registry entry. Its body-free activity records remain.`}>Remove entry</button>}
              </div>
            ))}
          </div>

          <div className="dashboard-card" data-help="Grant one registered tool temporary body access to the file currently open. Sensitive and Protected Zone policy still overrides this grant.">
            <h3>Temporary access to open file</h3>
            {currentFile ? (
              <div className="automation-grant-row">
                <span><strong>{currentFile.displayName}</strong><small>Expires after 10 minutes and is revoked with the credential.</small></span>
                <select value={grantAgentId ?? ""} onChange={(event) => setGrantAgentId(Number(event.target.value) || null)} aria-label="Local tool">
                  <option value="">Choose local tool</option>
                  {agents.filter((agent) => agent.enabled).map((agent) => <option value={agent.id} key={agent.id}>{agent.name}</option>)}
                </select>
                <button type="button" disabled={busy || grantAgentId === null} onClick={() => grantAgentId !== null && onGrantRead(grantAgentId, currentFile.nodeId)} data-help="Allow this named-pipe trusted tool — never an MCP connector — to request the currently open file body for 10 minutes. Protected policy still applies.">Grant 10 minutes</button>
              </div>
            ) : <p className="muted">Open a file first to create a temporary grant.</p>}
          </div>

          <div className="dashboard-card" data-help="The local activity log records which method was allowed or denied. Response bodies and file content are never stored here.">
            <h3>Local automation activity</h3>
            {activity.length === 0 ? <p className="muted">No local automation requests recorded.</p> : (
              <div className="automation-activity-list">
                {activity.slice(0, 100).map((entry) => (
                  <div key={entry.id}><strong>{entry.method}</strong><span className={entry.status === "allowed" ? "status-good" : "status-warning"}>{entry.status}</span><small>{entry.agentName ?? "Unregistered client"} · {entry.createdAt}</small><p>{entry.detail}</p></div>
                ))}
              </div>
            )}
          </div>
      </>
      {error ? <p className="scan-error">{error}</p> : null}
    </section>
  );
}

// The "AI App Integration" panel: connect Code Hangar to the AI apps it catalogs
// (Claude, Cursor, Codex) over the Model Context Protocol so they can read body-free
// structure/graph/comment metadata — and, only if the user opts in, annotate. Self-contained: it loads
// its own status and toggles and only needs a confirmation callback for the
// strongly-signposted, accountable switches and the per-app config writes.
export function SettingsConnectedAppsView({
  confirm,
  projects
}: {
  confirm: (message: string) => Promise<boolean>;
  projects: ProjectSummary[];
}) {
  const [hosts, setHosts] = useState<ConnectedAppStatus[]>([]);
  const [projectIds, setProjectIds] = useState<number[]>([]);
  const [projectQuery, setProjectQuery] = useState("");
  const [includeHistorySearch, setIncludeHistorySearch] = useState(false);
  const [includeMutationRequests, setIncludeMutationRequests] = useState(false);
  const [writeEnabled, setWriteEnabled] = useState(false);
  const [fullControl, setFullControl] = useState(false);
  const [readOnly, setReadOnly] = useState(false);
  const [requests, setRequests] = useState<AgentActionRequest[]>([]);
  const [approvingId, setApprovingId] = useState<number | null>(null);
  const [backupChecked, setBackupChecked] = useState(true);
  const [backupDir, setBackupDir] = useState<string | null>(null);
  // Extra strengthened-gate state for the mutation request kinds.
  const [holdingDir, setHoldingDir] = useState<string | null>(null);
  const [includeProtected, setIncludeProtected] = useState(false);
  const [liabilityAck, setLiabilityAck] = useState(false);
  const [recommendAck, setRecommendAck] = useState(false);
  const [crossScopeAck, setCrossScopeAck] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const visibleProjects = useMemo(() => {
    const query = projectQuery.trim().toLowerCase();
    return projects.filter((project) => !query || `${project.name} ${project.path}`.toLowerCase().includes(query));
  }, [projectQuery, projects]);
  const selectedProjects = useMemo(
    () => projects.filter((project) => projectIds.includes(project.id)),
    [projectIds, projects]
  );

  useEffect(() => {
    const knownIds = new Set(projects.map((project) => project.id));
    setProjectIds((current) => current.filter((projectId) => knownIds.has(projectId)));
  }, [projects]);

  const toggleProject = (projectId: number) => {
    setProjectIds((current) => current.includes(projectId)
      ? current.filter((item) => item !== projectId)
      : [...current, projectId]);
  };

  const reloadRequests = useCallback(async () => {
    const connectorApi = await requireConnectorApi();
    setRequests(await connectorApi.agentRequestsPending());
  }, []);

  const reload = useCallback(async () => {
    setLoaded(false);
    setLoadError(null);
    try {
      const connectorApi = await requireConnectorApi();
      const [hostList, write, full, frozen, pending] = await Promise.all([
        connectorApi.connectedAppStatus(),
        connectorApi.commentWriteEnabled(),
        connectorApi.mcpFullControlEnabled(),
        connectorApi.mcpReadOnlyMode(),
        connectorApi.agentRequestsPending()
      ]);
      setHosts(hostList);
      setWriteEnabled(write);
      setFullControl(full);
      setReadOnly(frozen);
      setRequests(pending);
      setLoadError(null);
      setError(null);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  // A request appears out-of-band: a connected app files it while this panel is
  // already open. Total control is the only tier that can queue one, so while it is
  // on, poll for pending requests and also refresh the moment the window regains
  // focus (e.g. right after switching back from the AI app that just asked). Without
  // this, a queued request stayed invisible until the panel was remounted.
  useEffect(() => {
    if (!fullControl) return;
    let cancelled = false;
    let refreshing = false;
    let timer: number | null = null;
    const schedule = (delay: number) => {
      if (cancelled) return;
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(refresh, delay);
    };
    const refresh = () => {
      if (cancelled || refreshing) return;
      if (document.hidden) {
        schedule(30_000);
        return;
      }
      refreshing = true;
      void reloadRequests().catch(() => {}).finally(() => {
        refreshing = false;
        schedule(4_000);
      });
    };
    const onVisibilityChange = () => {
      if (!document.hidden) {
        if (timer !== null) window.clearTimeout(timer);
        schedule(100);
      }
    };
    schedule(4_000);
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [fullControl, reloadRequests]);

  const isMutationKind = (kind: string) =>
    kind === "backup_protected" || kind === "move_to_holding" || kind === "final_remove";
  const describeRequest = (request: AgentActionRequest) => {
    const who = request.agentName;
    switch (request.kind) {
      case "comment_edit":
        return `${who} wants to edit a comment`;
      case "comment_delete":
        return `${who} wants to delete a comment`;
      case "read_body":
        return `${who}, a trusted local-IPC agent (not an MCP connector), wants to read a file's contents`;
      case "backup_protected":
        return `${who} wants to back up a folder, INCLUDING protected/sensitive files`;
      case "move_to_holding":
        return `${who} wants to move ${request.detail ?? "a target"} to the holding area`;
      case "final_remove":
        return `${who} recommends reviewing ${request.detail ?? "a held item"} for final removal`;
      default:
        return `${who} filed a request`;
    }
  };

  const approveRequestHelp = (request: AgentActionRequest) => {
    if (request.kind === "read_body") {
      return "Review a trusted local-IPC agent's temporary file-content grant. MCP connectors have no body tool; this separate local agent gets no body text unless you approve.";
    }
    if (request.kind === "final_remove") {
      return "Open the separate local project/batch review in Recovery & cleanup. Approving this connected-app recommendation never deletes anything.";
    }
    if (isMutationKind(request.kind)) {
      return "Review a privileged disk-action request. Code Hangar revalidates the plan, scopes, backups and final-remove gate before doing anything.";
    }
    return "Review a requested comment change. Backing up the comment first is offered as an easy checkbox.";
  };

  // Opening the approve panel is the first deliberate step; clicking "Approve" raises
  // the second confirmation. For mutation kinds the strengthened gate adds the
  // liability waiver, recommendation-against and folder pickers; cross-scope
  // requests add a cross-project authorization. Final removal is never approved
  // here: its button only directs the owner to the separate local batch review.
  const openApprove = (request: AgentActionRequest) => {
    setApprovingId(request.id);
    setBackupChecked(true);
    setBackupDir(null);
    setHoldingDir(null);
    setIncludeProtected(false);
    setLiabilityAck(false);
    setRecommendAck(false);
    setCrossScopeAck(false);
    setError(null);
  };

  const cancelApprove = () => {
    setApprovingId(null);
    setBackupDir(null);
    setHoldingDir(null);
  };

  const chooseHoldingFolder = async () => {
    const dir = await api.pickFolder("Choose a folder to move the target into");
    if (dir) {
      setHoldingDir(dir);
    }
  };

  const chooseBackupFolder = async () => {
    const dir = await api.pickFolder("Choose a safe folder for the comment backup");
    if (dir) {
      setBackupDir(dir);
    }
  };

  const finishApprove = async (request: AgentActionRequest, inputs: ResolveInputs) => {
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.agentRequestResolve(request.id, true, inputs);
      setApprovingId(null);
      setBackupDir(null);
      setHoldingDir(null);
      await reloadRequests();
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const confirmApprove = async (request: AgentActionRequest) => {
    if (request.kind === "final_remove") {
      setError("Open Recovery & cleanup from the sidebar and review the immutable local project/batch preview. This connected-app recommendation cannot delete anything.");
      return;
    }
    // Comment + read-body kinds keep the light gate (optional easy backup).
    if (!isMutationKind(request.kind)) {
      if ((request.kind === "comment_edit" || request.kind === "comment_delete") && backupChecked && !backupDir) {
        setError("Choose a backup folder, or untick the backup option to proceed without one.");
        return;
      }
      const willBackUp = backupChecked && backupDir != null;
      if (request.kind === "read_body") {
        if (!(await confirm(`Grant ${request.agentName} read access to this file for 10 minutes?`))) return;
        await finishApprove(request, {});
        return;
      }
      if (
        !(await confirm(
          `Apply this change to your comment as yourself? Code Hangar will ${request.kind === "comment_delete" ? "delete" : "edit"} it on your behalf${willBackUp ? " after backing it up" : " WITHOUT a backup"}. This cannot be undone.`
        ))
      ) {
        return;
      }
      await finishApprove(request, { backupDir: willBackUp ? backupDir : null });
      return;
    }

    // Mutation kinds: the strengthened gate.
    const needsBackupFolder = request.kind === "backup_protected" || request.kind === "move_to_holding";
    if (needsBackupFolder && !backupDir) {
      setError("Choose a backup folder before approving.");
      return;
    }
    if (request.kind === "move_to_holding" && !holdingDir) {
      setError("Choose a holding folder before approving.");
      return;
    }
    if (request.kind === "backup_protected" && !includeProtected) {
      setError("Tick the protected-files option to back up sensitive files.");
      return;
    }
    if (!liabilityAck || !recommendAck) {
      setError("Accept both acknowledgements to proceed.");
      return;
    }
    if (request.crossScope && !crossScopeAck) {
      setError("Authorize the cross-project action to proceed.");
      return;
    }
    if (
      !(await confirm(
        `Code Hangar recommends AGAINST this. Proceed with this ${request.kind === "move_to_holding" ? "move" : "protected backup"} as yourself? It was requested by a connected app, not by you.`
      ))
    ) {
      return;
    }
    await finishApprove(request, {
      backupDir: needsBackupFolder ? backupDir : null,
      holdingRoot: request.kind === "move_to_holding" ? holdingDir : null,
      includeProtectedOptIn: includeProtected,
      crossScopeAuthorized: crossScopeAck
    });
  };

  const rejectRequest = async (request: AgentActionRequest) => {
    if (!(await confirm(`Reject this request from ${request.agentName}? Nothing will change.`))) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.agentRequestResolve(request.id, false, {});
      await reloadRequests();
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const toggleWrite = async () => {
    const next = !writeEnabled;
    if (
      next &&
      !(await confirm(
        "Allow connected AI apps to write comments?\n\nThey can add and edit only their OWN comments — never the ones you wrote. A less capable model could still add noise you would have to clean up. You are enabling AI to write into your knowledge base. Continue?"
      ))
    ) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.setCommentWriteEnabled(next);
      setWriteEnabled(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const toggleFullControl = async () => {
    const next = !fullControl;
    if (next) {
      if (
        !(await confirm(
          "Give connected AI apps advanced request privileges?\n\nHosts with the standard comments_write scope can file protected comment-change requests. Only hosts explicitly reconnected with Request backup/holding actions can file backup or holding-area requests. Final removal remains a recommendation that opens the separate local Recovery review; it never executes here. Continue?"
        ))
      ) {
        return;
      }
      if (
        !(await confirm(
          "Are you absolutely sure?\n\nEach available request still waits for your review. Code Hangar revalidates that host's effective scopes, projects, protected locations, file locks and backup rules. This toggle grants no new scope by itself. Enable the request tier now?"
        ))
      ) {
        return;
      }
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.setMcpFullControlEnabled(next);
      setFullControl(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const toggleReadOnly = async () => {
    const next = !readOnly;
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      await connectorApi.setMcpReadOnlyMode(next);
      setReadOnly(next);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const register = async (host: ConnectedAppStatus) => {
    if (projectIds.length === 0) {
      setError("Choose at least one project before connecting an AI app.");
      return;
    }
    const projectSummary = selectedProjects.length <= 3
      ? selectedProjects.map((project) => project.name).join(", ")
      : `${selectedProjects.slice(0, 3).map((project) => project.name).join(", ")} and ${selectedProjects.length - 3} more`;
    if (
      !(await confirm(
        `${host.registered ? "Reconnect" : "Add"} Code Hangar ${host.registered ? "in" : "to"} ${host.label}'s configuration for ${projectIds.length} selected project${projectIds.length === 1 ? "" : "s"}?\n\nProjects: ${projectSummary}.\n\nThe standard connection is body-free: curated structure, graph and comments only. It can write only its own comments, and only when AI comment writes are enabled. The file and rollback sidecars are verified before the old credential is replaced.`
      ))
    ) {
      return;
    }
    if (includeHistorySearch || includeMutationRequests) {
      const advanced = [
        includeHistorySearch ? "bounded redacted session-history search" : null,
        includeMutationRequests ? "queue-only backup/holding requests and final-removal review recommendations" : null
      ].filter(Boolean).join(" and ");
      if (!(await confirm(
        `Confirm advanced access for ${host.label}?\n\nThis reconnect adds ${advanced}. It still adds no file-body tool and no direct mutation execution. Requests remain subject to the global request tier and your local approval. Continue with this exact advanced scope?`
      ))) {
        return;
      }
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      const updated = await connectorApi.connectedAppRegister(
        host.host,
        projectIds,
        includeHistorySearch,
        includeMutationRequests
      );
      // The backend returns only after it has re-read and verified the entry. Reflect that
      // postcondition immediately instead of leaving a stale "Not connected" card until remount.
      setHosts((current) => current.map((item) => (item.host === updated.host ? updated : item)));
      setProjectIds([]);
      setProjectQuery("");
      setIncludeHistorySearch(false);
      setIncludeMutationRequests(false);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (host: ConnectedAppStatus) => {
    if (
      !(await confirm(
        `Remove Code Hangar from ${host.label}'s configuration and revoke its token? Your other entries are left untouched.`
      ))
    ) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      const updated = await connectorApi.connectedAppRemove(host.host);
      setHosts((current) => current.map((item) => (item.host === updated.host ? updated : item)));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const revokeOrphan = async (host: ConnectedAppStatus) => {
    if (
      !(await confirm(
        `Revoke ${host.label}'s durable Code Hangar credential now?\n\nThis is a database-only safety action. It does not read, repair or edit ${host.label}'s external configuration. Any process still holding the old token loses access immediately.`
      ))
    ) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      const updated = await connectorApi.connectedAppRevokeOrphan(host.host);
      setHosts((current) => current.map((item) => (item.host === updated.host ? updated : item)));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const forgetOrphan = async (host: ConnectedAppStatus) => {
    if (
      !(await confirm(
        `Forget ${host.label}'s revoked credential record?\n\nThis removes only the already-revoked database row. It does not touch ${host.label}'s external configuration, and the authenticated host/path binding remains available for safe future recovery.`
      ))
    ) {
      return;
    }
    setBusy(true);
    try {
      const connectorApi = await requireConnectorApi();
      const updated = await connectorApi.connectedAppForgetOrphan(host.host);
      setHosts((current) => current.map((item) => (item.host === updated.host ? updated : item)));
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const availability = connectedAppsIntegrationAvailabilityState(loaded, loadError, hosts.length);
  if (availability !== "ready") {
    return (
      <IntegrationAvailabilityPanel
        title="AI app integration"
        icon={<Plug size={15} />}
        state={availability}
        error={loadError}
        loadingMessage="Reading connected-app support and local configuration status."
        notCompiledMessage="Connected-app support is not compiled into this executable. Local project navigation and management remain available."
        onRetry={() => void reload()}
      />
    );
  }

  return (
    <section className="pane-section compact">
      <SectionTitle icon={<Plug size={15} />} label="AI app integration" />
      <p className="muted help-copy">
        Let Claude, Cursor and Codex read body-free, curated project metadata — and, only if you allow it, annotate — over the
        Model Context Protocol. This connector has no file-body tool. Access is local and limited to each host&apos;s effective
        projects and scopes shown below.
      </p>

      {requests.length > 0 ? (
        <div className="dashboard-card warning-card" data-help="A connected app filed a privileged request. Nothing happens until you approve; on approval Code Hangar acts as you and revalidates the relevant safety gate.">
          <h3>Requests awaiting your approval ({requests.length})</h3>
          {requests.map((request) => (
            <div className="automation-agent-row" key={request.id}>
              <div>
                <strong>{describeRequest(request)}</strong>
                {request.currentBody != null ? <small>Now: “{request.currentBody}”</small> : null}
                {request.kind !== "comment_delete" && request.proposedBody ? (
                  <small>Proposed: “{request.proposedBody}”</small>
                ) : null}
              </div>
              {approvingId === request.id ? (
                <div className="request-approve">
                  {request.kind === "final_remove" ? (
                    <p className="muted help-copy">This connected app is recommending a review only. Open the held project/batch in Recovery &amp; cleanup; this panel cannot delete it or approve the local final-removal batch.</p>
                  ) : isMutationKind(request.kind) ? (
                    <>
                      <p className="comment-error"><AlertTriangle size={13} /> Code Hangar recommends against this. A connected app requested it — not you.</p>
                      {request.kind === "backup_protected" || request.kind === "move_to_holding" ? (
                        <button type="button" className="secondary-button slim" disabled={busy} onClick={() => void chooseBackupFolder()} data-help="Choose the folder the verified backup is written to.">
                          {backupDir ? `Backup folder: ${backupDir}` : "Choose backup folder…"}
                        </button>
                      ) : null}
                      {request.kind === "move_to_holding" ? (
                        <button type="button" className="secondary-button slim" disabled={busy} onClick={() => void chooseHoldingFolder()} data-help="Choose the folder the target is moved into.">
                          {holdingDir ? `Move into: ${holdingDir}` : "Choose holding folder…"}
                        </button>
                      ) : null}
                      {request.kind === "backup_protected" || request.kind === "move_to_holding" ? (
                        <label className="toggle-row">
                          <input type="checkbox" checked={includeProtected} disabled={busy} onChange={(event) => setIncludeProtected(event.target.checked)} />
                          <span><strong>Include protected/sensitive files</strong><small>Copies secret bytes into the backup. You must tick this for a protected backup.</small></span>
                        </label>
                      ) : null}
                      {request.crossScope ? (
                        <label className="toggle-row">
                          <input type="checkbox" checked={crossScopeAck} disabled={busy} onChange={(event) => setCrossScopeAck(event.target.checked)} />
                          <span><strong>Authorize cross-project action</strong><small>This app is not scoped to the target&apos;s project.</small></span>
                        </label>
                      ) : null}
                      <label className="toggle-row">
                        <input type="checkbox" checked={liabilityAck} disabled={busy} onChange={(event) => setLiabilityAck(event.target.checked)} />
                        <span><strong>I accept full responsibility</strong><small>I release Code Hangar from liability for any data loss.</small></span>
                      </label>
                      <label className="toggle-row">
                        <input type="checkbox" checked={recommendAck} disabled={busy} onChange={(event) => setRecommendAck(event.target.checked)} />
                        <span><strong>Proceed against the recommendation</strong><small>I understand Code Hangar advises against this and choose to continue.</small></span>
                      </label>
                    </>
                  ) : request.kind === "read_body" ? (
                    <p className="muted help-copy">Grant the separate trusted local-IPC agent {request.agentName} a 10-minute read of this file. MCP connectors have no body tool. No file is changed.</p>
                  ) : (
                    <>
                      <label className="toggle-row" data-help="Recommended, but optional. When ticked, the comment is written to a folder you choose and verified before the change.">
                        <input type="checkbox" checked={backupChecked} disabled={busy} onChange={(event) => setBackupChecked(event.target.checked)} />
                        <span><strong>Back up the comment first</strong><small>Recommended. Untick to proceed without a backup.</small></span>
                      </label>
                      {backupChecked ? (
                        <button type="button" className="secondary-button slim" disabled={busy} onClick={() => void chooseBackupFolder()} data-help="Choose the safe folder the comment backup is written to.">
                          {backupDir ? `Backup folder: ${backupDir}` : "Choose backup folder…"}
                        </button>
                      ) : null}
                    </>
                  )}
                  <div className="inline-actions">
                    <button type="button" className={request.kind === "final_remove" ? "secondary-button slim" : isMutationKind(request.kind) ? "danger-button slim" : "primary-button slim"} disabled={busy} onClick={() => void confirmApprove(request)} data-help={request.kind === "final_remove" ? "Open the separate local project/batch review in Recovery & cleanup. This does not delete or approve anything." : "Perform the action as yourself. You will be asked to confirm once more."}>
                      {request.kind === "final_remove" ? "Review in Recovery" : isMutationKind(request.kind) ? "Approve action" : request.kind === "read_body" ? "Grant access" : "Approve change"}
                    </button>
                    <button type="button" className="secondary-button slim" disabled={busy} onClick={cancelApprove}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : (
                <div className="inline-actions">
                  <button type="button" className="primary-button slim" disabled={busy} onClick={() => openApprove(request)} data-help={approveRequestHelp(request)}>
                    Approve…
                  </button>
                  <button type="button" className="secondary-button slim" disabled={busy} onClick={() => void rejectRequest(request)} data-help="Reject. Nothing changes.">
                    Reject
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>
      ) : null}

      <div className={`dashboard-card ${readOnly ? "warning-card" : ""}`} data-help="A master freeze. When on, connected apps can READ but never write or change anything — overriding the toggles below, including requests already awaiting approval.">
        <label className="toggle-row">
          <input type="checkbox" checked={readOnly} disabled={busy} onChange={() => void toggleReadOnly()} />
          <span>
            <strong>Read-only mode — freeze all AI writes</strong>
            <small>
              Off by default. When on, connected apps can still read your curated knowledge, but every write, comment change
              and action is refused — including any request already waiting for your approval. A one-flip safety brake that
              overrides the two settings below.
            </small>
          </span>
        </label>
      </div>

      <div className={`dashboard-card ${writeEnabled ? "warning-card" : ""}`} data-help="Off by default. Connected apps can add and edit only their own comments; they can never touch a comment you wrote.">
        <label className="toggle-row">
          <input type="checkbox" checked={writeEnabled} disabled={busy} onChange={() => void toggleWrite()} />
          <span>
            <strong>Allow AI apps to write comments</strong>
            <small>
              Off by default. When on, a connected app can add and edit its OWN comments. It can never change a comment you
              wrote. A less capable model could still add noise you would clean up — you are accountable for enabling this.
            </small>
          </span>
        </label>
      </div>

      <div className={`dashboard-card ${fullControl ? "danger-card" : ""}`} data-help="Off by default. Enables request filing only within each host's effective scopes: protected comment requests on the base scope, and backup/holding requests only with execute_plan. It grants no scope by itself.">
        <label className="toggle-row">
          <input type="checkbox" checked={fullControl} disabled={busy} onChange={() => void toggleFullControl()} />
          <span>
            <strong><AlertTriangle size={13} /> Allow advanced AI requests</strong>
            <small>
              Off by default. This grants no capability by itself. Hosts with the base comments scope may request protected
              comment changes; only hosts explicitly connected with Request backup/holding actions may file those disk-action
              requests. A final-removal recommendation only directs you to Recovery&apos;s separate immutable local batch review.
            </small>
          </span>
        </label>
      </div>

      <div className="dashboard-card connected-app-project-scope" data-help="Every new AI app connection is restricted to the projects explicitly selected here. No selection never means every project.">
        <div className="card-title-row">
          <h3>Projects for the next connection</h3>
          <span className={projectIds.length > 0 ? "status-good" : "status-warning"}>{projectIds.length} selected</span>
        </div>
        <p className="muted help-copy">Choose the smallest project set this AI app needs. Each app receives a separate scope when you connect it; reconnect the app to change that scope.</p>
        <div className="automation-choice-list" role="group" aria-label="Advanced capabilities for the next AI app connection">
          <strong>Optional capabilities for the next Connect/Reconnect</strong>
          <label className="toggle-row" data-help="Adds bounded, redacted search snippets from selected projects. It never exposes a full transcript or file body.">
            <input type="checkbox" checked={includeHistorySearch} onChange={(event) => setIncludeHistorySearch(event.target.checked)} />
            <span><strong>Search redacted session history</strong><small>Opt-in history_search scope. Bounded snippets only; never full transcripts or file bodies.</small></span>
          </label>
          <label className="toggle-row" data-help="Adds queue-only backup and holding-area requests plus final-removal review recommendations. The connected app still cannot execute a mutation or approve final removal directly.">
            <input type="checkbox" checked={includeMutationRequests} onChange={(event) => setIncludeMutationRequests(event.target.checked)} />
            <span><strong>Request protected action reviews</strong><small>Opt-in execute_plan scope for backup/holding requests and final-removal review recommendations only. Local human review and every mutation gate remain mandatory.</small></span>
          </label>
          {(includeHistorySearch || includeMutationRequests) ? <p className="warning-inline">Advanced access requires a second confirmation when you Connect/Reconnect.</p> : null}
        </div>
        <label className="field-label">
          Find project
          <input value={projectQuery} onChange={(event) => setProjectQuery(event.target.value)} placeholder="Project name or local path" />
        </label>
        <div className="automation-choice-list project-scope-list" role="group" aria-label="Projects allowed for the next AI app connection">
          {visibleProjects.map((project) => (
            <label className="toggle-row" key={project.id} data-help={`Allow the next connected AI app to address project ${project.name}.`}>
              <input type="checkbox" checked={projectIds.includes(project.id)} onChange={() => toggleProject(project.id)} />
              <span><strong>{project.name}</strong><small>{project.path}</small></span>
            </label>
          ))}
          {visibleProjects.length === 0 ? <p className="muted">No projects match this search.</p> : null}
        </div>
        {projectIds.length === 0 ? <p className="warning-inline">Select at least one project. Connect stays locked until you do.</p> : null}
      </div>

      {hosts.map((host) => (
        <div className="dashboard-card" key={host.host} data-help={`Code Hangar registers itself into ${host.label}'s config. The file is backed up and only our entry is changed.`}>
          <div className="card-title-row">
            <h3>{host.label}</h3>
            <span className={host.credentialActive ? "status-good" : host.credentialOrphaned || host.recoveryRequired || !host.readable ? "status-warning" : "status-muted"}>
              {host.recoveryRequired ? "Recovery required" : host.credentialActive ? "Connected" : host.credentialOrphaned ? "Credential orphaned" : host.durableAgentId != null && !host.durableCredentialEnabled ? "Credential revoked" : host.registered ? "Config present · inactive credential" : !host.readable ? "Config unreadable" : host.configExists ? "Not connected" : "No config yet"}
            </span>
          </div>
          <code className="path-code">{host.configPath}</code>
          {host.durableIdentityId ? (
            <div className="muted help-copy" data-help="Immutable database identity used for authorization. The display name is never an authority boundary.">
              Durable identity: <code>{host.durableIdentityId}</code>
            </div>
          ) : null}
          {host.credentialActive ? (
            <div className="muted help-copy" data-help="These are read from the exact enabled credential whose hash matches this host's current config; global toggles do not infer them.">
              <strong>Effective access</strong>
              <div>Scopes: {(host.effectiveScopes ?? []).join(" · ") || "none"}</div>
              <div>Projects: {(host.effectiveProjectIds ?? []).map((projectId) => projects.find((project) => project.id === projectId)?.name ?? `Project ${projectId}`).join(" · ") || "none"}</div>
            </div>
          ) : null}
          <div className="inline-actions">
            {host.registered ? (
              <>
                <button type="button" className="primary-button" disabled={busy || !host.readable || host.recoveryRequired || projectIds.length === 0} onClick={() => void register(host)} data-help={projectIds.length === 0 ? "Choose at least one project above before reconnecting this AI app." : `Atomically rotate ${host.label} to the selected projects and optional capabilities.`}>
                  <Plug size={14} /> Reconnect with selected access
                </button>
                <button type="button" className="secondary-button" disabled={busy || host.recoveryRequired} onClick={() => void remove(host)} data-help={`Remove Code Hangar from ${host.label} and revoke its token only after the config removal is verified.`}>
                  Disconnect
                </button>
              </>
            ) : (
              <button type="button" className="primary-button" disabled={busy || !host.readable || host.recoveryRequired || projectIds.length === 0} onClick={() => void register(host)} data-help={projectIds.length === 0 ? "Choose at least one project above before connecting this AI app." : `Add Code Hangar to ${host.label} with a fresh per-app token limited to ${projectIds.length} selected project${projectIds.length === 1 ? "" : "s"}.`}>
                <Plug size={14} /> Connect
              </button>
            )}
            {host.credentialOrphaned && host.durableCredentialEnabled ? (
              <button type="button" className="danger-button" disabled={busy || host.recoveryRequired} onClick={() => void revokeOrphan(host)} data-help={`Immediately revoke ${host.label}'s durable token in Code Hangar without reading or editing the external config.`}>
                Revoke credential only
              </button>
            ) : null}
            {host.durableAgentId != null && !host.durableCredentialEnabled && !host.credentialActive ? (
              <button type="button" className="secondary-button" disabled={busy || host.recoveryRequired} onClick={() => void forgetOrphan(host)} data-help={`Forget only ${host.label}'s already-revoked database row. The external config is untouched.`}>
                Forget revoked record
              </button>
            ) : null}
          </div>
          {host.credentialOrphaned ? (
            <p className="comment-error">{host.orphanReason ?? "A durable credential exists but cannot be matched to this app's current external config. Revoke it database-only if the app is no longer trusted."}</p>
          ) : null}
          {!host.readable ? (
            <p className="comment-error">This app&apos;s config could not be parsed, so Code Hangar will not modify it.</p>
          ) : null}
          {host.recoveryRequired ? (
            <p className="comment-error">The config or rollback sidecars do not match the encrypted journal. Code Hangar left them untouched and blocked Connect/Disconnect for this host.</p>
          ) : null}
        </div>
      ))}

      {error ? <p className="scan-error">{error}</p> : null}
    </section>
  );
}
