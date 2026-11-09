//! Thin Rust adapter for the published OpenCode V2 HTTP contract.
//! No native plugin is needed for the first vertical slice.
use crate::{contracts::Report, store::Attempt};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{
    Method, StatusCode, Url,
    blocking::{Client, Response},
    redirect::Policy,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    io::Read,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub directory: PathBuf,
    pub agent: String,
    pub model: Option<Model>,
    pub prompt: String,
    pub permissions: Vec<Value>,
    #[serde(default)]
    pub endpoint: Option<String>,
}

pub trait AgentBackend {
    /// None means still running. Retry using exactly the same attempt and prompt IDs.
    fn execute(&self, attempt: &Attempt, assignment: &Assignment) -> Result<Option<Report>>;

    fn endpoint(&self) -> Option<&str> {
        None
    }

    fn pin(&self, assignment: Assignment) -> Result<Assignment> {
        ensure!(
            assignment.model.is_some(),
            "backend requires an explicit model for durable dispatch"
        );
        Ok(assignment)
    }

    fn execute_for(
        &self,
        attempt: &Attempt,
        assignment: &Assignment,
        _remaining: Duration,
    ) -> Result<Option<Report>> {
        self.execute(attempt, assignment)
    }

    fn inspect(&self, _attempt: &Attempt) -> Result<WorkerState> {
        bail!("backend does not support worker-state reconciliation")
    }

    fn interrupt(&self, _attempt: &Attempt) -> Result<WorkerState> {
        bail!("backend does not support worker cancellation")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOutcome {
    Idle,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkerState {
    Missing,
    Running,
    Stopped { outcome: WorkerOutcome },
}

impl WorkerState {
    pub fn stopped(self) -> bool {
        !matches!(self, Self::Running)
    }
}

pub struct OpenCode {
    client: Client,
    base: Url,
    authorization: Option<String>,
    deadline: Option<Instant>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct EnvelopeWithCursor<T> {
    data: T,
    #[serde(default)]
    cursor: Option<Value>,
}

/// Public session projection used by the orchestrator console.
#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default, rename = "parentID")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub location: Option<Location>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub model: Option<Model>,
    #[serde(default)]
    pub permissions: Option<Vec<Value>>,
    #[serde(default)]
    pub time: Option<SessionTime>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionTime {
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub updated: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Location {
    pub directory: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub finish: Option<String>,
    #[serde(default)]
    pub error: Option<WorkerError>,
    #[serde(default)]
    pub content: Vec<Part>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub time: Option<MessageTime>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageTime {
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub completed: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Part {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub state: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerError {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub status: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub permissions: Option<Vec<Value>>,
}

/// Request shape for `POST /api/session`.
#[derive(Debug, Clone, Serialize)]
pub struct NewSession {
    #[serde(skip_serializing_if = "Option::is_none", rename = "parentID")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<Value>>,
}

/// Mutable session fields accepted by `PATCH /api/session/{id}`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<Value>>,
}

impl OpenCode {
    /// Authorization is an explicit HTTP Authorization value, sourced from the environment.
    pub fn new(base: &str, authorization: Option<String>) -> Result<Self> {
        let base = Url::parse(base)?;
        ensure!(
            base.username().is_empty() && base.password().is_none(),
            "credentials must not be embedded in the server URL"
        );
        ensure!(
            base.path() == "/" && base.query().is_none() && base.fragment().is_none(),
            "OpenCode server URL must name the server root"
        );
        let host = base.host_str().context("server hostname required")?;
        let loopback = host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        ensure!(
            base.scheme() == "https" || (base.scheme() == "http" && loopback),
            "HTTP is permitted only for a loopback server; use HTTPS remotely"
        );
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(Policy::none())
            .no_proxy()
            .build()?;
        Ok(Self {
            client,
            base,
            authorization,
            deadline: None,
        })
    }

    fn request(&self, method: Method, path: &str) -> Result<reqwest::blocking::RequestBuilder> {
        let mut request = self.client.request(method, self.base.join(path)?);
        if let Some(value) = &self.authorization {
            request = request.header(reqwest::header::AUTHORIZATION, value);
        }
        Ok(request.timeout(self.timeout(Duration::from_secs(30))?))
    }

    fn timeout(&self, maximum: Duration) -> Result<Duration> {
        let remaining = self
            .deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(maximum)
            .min(maximum);
        ensure!(!remaining.is_zero(), "worker_deadline_exceeded");
        Ok(remaining)
    }

    fn scoped(&self, budget: Duration) -> Self {
        let deadline = Instant::now() + budget;
        Self {
            client: self.client.clone(),
            base: self.base.clone(),
            authorization: self.authorization.clone(),
            deadline: Some(
                self.deadline
                    .map_or(deadline, |existing| existing.min(deadline)),
            ),
        }
    }

    fn session(&self, attempt: &Attempt) -> Result<Option<Session>> {
        let response = self
            .request(Method::GET, &format!("/api/session/{}", attempt.session_id))?
            .send()?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(decode::<Envelope<Session>>(response)?.data))
    }

    pub fn list_sessions(
        &self,
        parent: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Session>, Option<String>)> {
        let limit = limit.clamp(1, 200).to_string();
        let response = self
            .request(Method::GET, "/api/session")?
            .query(&[("parentID", parent.unwrap_or("null")), ("limit", &limit)])
            .send()?;
        let page: EnvelopeWithCursor<Vec<Session>> = decode_limit(response, 8 * 1024 * 1024)?;
        let next = page
            .cursor
            .as_ref()
            .and_then(|cursor| cursor["next"].as_str().map(str::to_owned));
        Ok((page.data, next))
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let response = self
            .request(Method::GET, &format!("/api/session/{id}"))?
            .send()?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(decode::<Envelope<Session>>(response)?.data))
    }

    pub fn create_session(&self, input: &NewSession) -> Result<Session> {
        Ok(decode::<Envelope<Session>>(
            self.request(Method::POST, "/api/session")?
                .json(input)
                .send()?,
        )?
        .data)
    }

    pub fn update_session(&self, id: &str, input: &SessionPatch) -> Result<()> {
        self.request(Method::PATCH, &format!("/api/session/{id}"))?
            .json(input)
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn move_session(&self, id: &str, directory: &Path) -> Result<()> {
        self.request(Method::POST, &format!("/api/session/{id}/move"))?
            .json(&json!({"directory": directory}))
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn active_sessions(&self) -> Result<std::collections::BTreeSet<String>> {
        let active: Envelope<Value> =
            decode(self.request(Method::GET, "/api/session/active")?.send()?)?;
        let object = active
            .data
            .as_object()
            .context("invalid active-session response")?;
        Ok(object.keys().cloned().collect())
    }

    pub fn prompt(&self, id: &str, prompt_id: &str, text: &str) -> Result<()> {
        let admitted: Envelope<Value> = decode(
            self.request(Method::POST, &format!("/api/session/{id}/prompt"))?
                .json(&json!({"id": prompt_id, "text": text, "delivery": "queue"}))
                .send()?,
        )?;
        ensure!(
            admitted.data["id"].as_str() == Some(prompt_id),
            "server returned a different prompt ID"
        );
        Ok(())
    }

    pub fn synthetic(&self, id: &str, prompt_id: &str, text: &str, resume: bool) -> Result<()> {
        self.request(Method::POST, &format!("/api/session/{id}/synthetic"))?
            .json(&json!({"id": prompt_id, "text": text, "resume": resume}))
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn interrupt_session(&self, id: &str) -> Result<bool> {
        let response = self
            .request(Method::POST, &format!("/api/session/{id}/interrupt"))?
            .query(&[("resume", "false")])
            .send()?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        response.error_for_status()?;
        Ok(true)
    }

    pub fn messages(
        &self,
        id: &str,
        limit: usize,
        order: &str,
        kind: Option<&str>,
    ) -> Result<(Vec<Message>, Option<String>)> {
        let limit = limit.clamp(1, 200).to_string();
        let mut request = self
            .request(Method::GET, &format!("/api/session/{id}/message"))?
            .query(&[("limit", limit.as_str()), ("order", order)]);
        if let Some(kind) = kind {
            request = request.query(&[("type", kind)]);
        }
        let page: EnvelopeWithCursor<Vec<Message>> =
            decode_limit(request.send()?, 8 * 1024 * 1024)?;
        let next = page
            .cursor
            .as_ref()
            .and_then(|cursor| cursor["next"].as_str().map(str::to_owned));
        Ok((page.data, next))
    }

    pub fn get_message(&self, session: &str, message: &str) -> Result<Option<Message>> {
        let response = self
            .request(
                Method::GET,
                &format!("/api/session/{session}/message/{message}"),
            )?
            .send()?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(decode::<Envelope<Message>>(response)?.data))
    }

    pub fn agents(&self, directory: &Path) -> Result<Vec<AgentInfo>> {
        let agents: Envelope<Vec<AgentInfo>> = decode(
            self.request(Method::GET, "/api/agent")?
                .query(&[("location[directory]", directory)])
                .send()?,
        )?;
        Ok(agents.data)
    }
}

/// Narrow synchronous client surface used by the orchestrator console.
pub trait Api: Send + Sync {
    fn list_sessions(
        &self,
        parent: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Session>, Option<String>)>;
    fn get_session(&self, id: &str) -> Result<Option<Session>>;
    fn create_session(&self, input: &NewSession) -> Result<Session>;
    fn update_session(&self, id: &str, input: &SessionPatch) -> Result<()>;
    fn move_session(&self, id: &str, directory: &Path) -> Result<()>;
    fn active_sessions(&self) -> Result<std::collections::BTreeSet<String>>;
    fn prompt(&self, id: &str, prompt_id: &str, text: &str) -> Result<()>;
    fn synthetic(&self, id: &str, prompt_id: &str, text: &str, resume: bool) -> Result<()>;
    fn interrupt_session(&self, id: &str) -> Result<bool>;
    fn messages(
        &self,
        id: &str,
        limit: usize,
        order: &str,
        kind: Option<&str>,
    ) -> Result<(Vec<Message>, Option<String>)>;
    fn get_message(&self, session: &str, message: &str) -> Result<Option<Message>>;
    fn agents(&self, directory: &Path) -> Result<Vec<AgentInfo>>;
}

impl Api for OpenCode {
    fn list_sessions(
        &self,
        parent: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Session>, Option<String>)> {
        OpenCode::list_sessions(self, parent, limit)
    }
    fn get_session(&self, id: &str) -> Result<Option<Session>> {
        OpenCode::get_session(self, id)
    }
    fn create_session(&self, input: &NewSession) -> Result<Session> {
        OpenCode::create_session(self, input)
    }
    fn update_session(&self, id: &str, input: &SessionPatch) -> Result<()> {
        OpenCode::update_session(self, id, input)
    }
    fn move_session(&self, id: &str, directory: &Path) -> Result<()> {
        OpenCode::move_session(self, id, directory)
    }
    fn active_sessions(&self) -> Result<std::collections::BTreeSet<String>> {
        OpenCode::active_sessions(self)
    }
    fn prompt(&self, id: &str, prompt_id: &str, text: &str) -> Result<()> {
        OpenCode::prompt(self, id, prompt_id, text)
    }
    fn synthetic(&self, id: &str, prompt_id: &str, text: &str, resume: bool) -> Result<()> {
        OpenCode::synthetic(self, id, prompt_id, text, resume)
    }
    fn interrupt_session(&self, id: &str) -> Result<bool> {
        OpenCode::interrupt_session(self, id)
    }
    fn messages(
        &self,
        id: &str,
        limit: usize,
        order: &str,
        kind: Option<&str>,
    ) -> Result<(Vec<Message>, Option<String>)> {
        OpenCode::messages(self, id, limit, order, kind)
    }
    fn get_message(&self, session: &str, message: &str) -> Result<Option<Message>> {
        OpenCode::get_message(self, session, message)
    }
    fn agents(&self, directory: &Path) -> Result<Vec<AgentInfo>> {
        OpenCode::agents(self, directory)
    }
}

impl AgentBackend for OpenCode {
    fn endpoint(&self) -> Option<&str> {
        Some(self.base.as_str())
    }

    fn pin(&self, mut assignment: Assignment) -> Result<Assignment> {
        assignment.endpoint = Some(self.base.to_string());
        if assignment.model.is_none() {
            #[derive(Deserialize)]
            struct AgentSelection {
                model: Option<Model>,
            }
            let mut url = self.base.join("/api/agent/")?;
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("invalid server URL"))?
                .pop_if_empty()
                .push(&assignment.agent);
            let selected: Envelope<AgentSelection> = decode(
                self.request(Method::GET, url.as_str())?
                    .query(&[(
                        "location[directory]",
                        assignment.directory.to_string_lossy().as_ref(),
                    )])
                    .send()?,
            )?;
            assignment.model = selected.data.model;
            if assignment.model.is_none() {
                #[derive(Deserialize)]
                struct CatalogModel {
                    id: String,
                    #[serde(rename = "providerID")]
                    provider_id: String,
                }
                let selected: Envelope<CatalogModel> = decode(
                    self.request(Method::GET, "/api/model/default")?
                        .query(&[(
                            "location[directory]",
                            assignment.directory.to_string_lossy().as_ref(),
                        )])
                        .send()?,
                )?;
                assignment.model = Some(Model {
                    provider_id: selected.data.provider_id,
                    id: selected.data.id,
                    variant: None,
                });
            }
        }
        Ok(assignment)
    }

    fn execute_for(
        &self,
        attempt: &Attempt,
        assignment: &Assignment,
        remaining: Duration,
    ) -> Result<Option<Report>> {
        self.scoped(remaining).execute(attempt, assignment)
    }

    fn inspect(&self, attempt: &Attempt) -> Result<WorkerState> {
        let scoped = self.scoped(Duration::from_secs(10));
        let Some(session) = scoped.session(attempt)? else {
            return Ok(WorkerState::Missing);
        };
        validate_owner(&session, attempt)?;
        let active: Envelope<Value> =
            decode(scoped.request(Method::GET, "/api/session/active")?.send()?)?;
        ensure!(active.data.is_object(), "invalid active-session response");
        let inbox: Envelope<Vec<Value>> = decode(
            scoped
                .request(
                    Method::GET,
                    &format!("/api/session/{}/inbox", attempt.session_id),
                )?
                .send()?,
        )?;
        if active.data.get(&attempt.session_id).is_some() || !inbox.data.is_empty() {
            return Ok(WorkerState::Running);
        }
        let outcome = match session.outcome.as_deref() {
            None => WorkerOutcome::Idle,
            Some("succeeded") => WorkerOutcome::Succeeded,
            Some("failed") => WorkerOutcome::Failed,
            Some("interrupted") => WorkerOutcome::Interrupted,
            _ => bail!("unknown worker outcome; termination not confirmed"),
        };
        Ok(WorkerState::Stopped { outcome })
    }

    fn interrupt(&self, attempt: &Attempt) -> Result<WorkerState> {
        let scoped = self.scoped(Duration::from_secs(10));
        let Some(session) = scoped.session(attempt)? else {
            return Ok(WorkerState::Missing);
        };
        validate_owner(&session, attempt)?;
        scoped
            .request(
                Method::DELETE,
                &format!(
                    "/api/session/{}/inbox/{}",
                    attempt.session_id, attempt.prompt_id
                ),
            )?
            .send()?
            .error_for_status()?;
        scoped
            .request(
                Method::POST,
                &format!("/api/session/{}/interrupt", attempt.session_id),
            )?
            .query(&[("resume", "false")])
            .send()?
            .error_for_status()?;
        // A 200 interrupt response is not proof of quiescence. Reconcile both
        // the running drain and durable inbox, without deleting anyone else's input.
        scoped.inspect(attempt)
    }

    fn execute(&self, attempt: &Attempt, assignment: &Assignment) -> Result<Option<Report>> {
        if let Some(expected) = &assignment.endpoint {
            ensure!(
                expected == self.base.as_str(),
                "server endpoint differs from frozen dispatch"
            );
        }
        let session = match self.session(attempt)? {
            Some(session) => session,
            None => {
                let mut input = json!({
                    "id":attempt.session_id, "title":format!("Synarch {} {:?}", attempt.run_id, attempt.role),
                    "agent":assignment.agent, "location":{"directory":assignment.directory},
                    "metadata":{"synarch_attempt":attempt.id}, "permissions":assignment.permissions,
                });
                if let Some(model) = &assignment.model {
                    input["model"] = serde_json::to_value(model)?;
                }
                decode::<Envelope<Session>>(
                    self.request(Method::POST, "/api/session")?
                        .json(&input)
                        .send()?,
                )?
                .data
            }
        };
        validate_session(&session, attempt, assignment)?;
        // The ID is persisted before this side effect. V2 admission deduplicates retries of that ID.
        let admitted: Envelope<Value> = decode(
            self.request(
                Method::POST,
                &format!("/api/session/{}/prompt", attempt.session_id),
            )?
            .json(&json!({"id":attempt.prompt_id,"text":assignment.prompt,"delivery":"queue"}))
            .send()?,
        )?;
        ensure!(
            admitted.data["id"].as_str() == Some(&attempt.prompt_id),
            "server returned a different prompt ID"
        );
        let wait = self
            .request(
                Method::POST,
                &format!("/api/experimental/session/{}/wait", attempt.session_id),
            )?
            .timeout(self.timeout(Duration::from_secs(2))?)
            .send();
        match wait {
            Err(error) if error.is_timeout() => return Ok(None),
            result => {
                result?.error_for_status()?;
            }
        }
        let session = self
            .session(attempt)?
            .context("worker session disappeared")?;
        validate_session(&session, attempt, assignment)?;
        // Admission and execution scheduling are distinct: an idle wait can return
        // before the newly admitted input has produced an execution outcome.
        if session.outcome.is_none() {
            return Ok(None);
        }
        let response = self
            .request(
                Method::GET,
                &format!("/api/session/{}/message", attempt.session_id),
            )?
            .query(&[("type", "assistant"), ("order", "desc"), ("limit", "1")])
            .send()?;
        let messages: Envelope<Vec<Message>> = decode(response)?;
        let message = messages
            .data
            .first()
            .context("worker produced no assistant report")?;
        ensure!(
            message.kind == "assistant"
                && message.agent.as_deref() == Some(assignment.agent.as_str()),
            "worker report is from the wrong agent"
        );
        if let Some(error) = &message.error {
            // Only surface a bounded, inert error code and numeric status. Provider
            // messages/response bodies may contain credentials or untrusted instructions.
            let kind = if error.kind.len() <= 128
                && error
                    .kind
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            {
                error.kind.as_str()
            } else {
                "unclassified-provider-error"
            };
            bail!(
                "worker session {} failed ({kind}, HTTP {}); inspect the session before retrying",
                attempt.session_id,
                error
                    .status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "unknown".into())
            );
        }
        ensure!(
            session.outcome.as_deref() == Some("succeeded")
                && message.finish.as_deref() == Some("stop"),
            "worker session {} did not complete successfully; inspect the session before retrying",
            attempt.session_id
        );
        let text = message
            .content
            .iter()
            .filter(|part| part.kind == "text")
            .filter_map(|part| part.text.as_deref())
            .collect::<Vec<_>>()
            .join("");
        Ok(Some(parse_report(&text)?))
    }
}

fn validate_session(session: &Session, attempt: &Attempt, assignment: &Assignment) -> Result<()> {
    validate_owner(session, attempt)?;
    if let Some(expected) = &assignment.model {
        let actual = session.model.as_ref().context("worker model missing")?;
        ensure!(
            actual.id == expected.id
                && actual.provider_id == expected.provider_id
                && actual.variant.as_deref().unwrap_or("default")
                    == expected.variant.as_deref().unwrap_or("default"),
            "worker model differs from frozen dispatch"
        );
    }
    if !assignment.permissions.is_empty() {
        ensure!(
            session.permissions.as_ref() == Some(&assignment.permissions),
            "worker permissions differ from frozen dispatch"
        );
    }
    ensure!(
        session.agent.as_deref() == Some(assignment.agent.as_str())
            && session
                .location
                .as_ref()
                .is_some_and(|location| location.directory == assignment.directory),
        "worker agent or workspace mismatch"
    );
    Ok(())
}

fn validate_owner(session: &Session, attempt: &Attempt) -> Result<()> {
    ensure!(
        session.id == attempt.session_id
            && session.metadata["synarch_attempt"].as_str() == Some(&attempt.id),
        "session ownership mismatch"
    );
    Ok(())
}

fn decode<T: DeserializeOwned>(response: Response) -> Result<T> {
    decode_limit(response, 1_048_576)
}

fn decode_limit<T: DeserializeOwned>(response: Response, limit: usize) -> Result<T> {
    let response = response.error_for_status()?;
    let mut body = Vec::new();
    response
        .take(u64::try_from(limit)? + 1)
        .read_to_end(&mut body)?;
    ensure!(body.len() <= limit, "OpenCode response exceeded its bound");
    serde_json::from_slice(&body).context("invalid OpenCode V2 response")
}

pub fn parse_report(text: &str) -> Result<Report> {
    ensure!(text.len() <= 262_144, "agent report exceeds 256 KiB");
    let text = text.trim();
    let text = text
        .strip_prefix("```json")
        .and_then(|s| s.trim().strip_suffix("```"))
        .unwrap_or(text)
        .trim();
    serde_json::from_str(text).context("agent must return exactly one schema-valid JSON report")
}
