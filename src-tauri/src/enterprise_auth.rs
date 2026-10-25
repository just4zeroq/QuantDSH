//! Main-process authentication; the opaque member token never enters a renderer or CLI.
//! Mirrors workdsh apps/desktop/src/enterprise-auth.ts.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Collaboration and extension payloads allow 8 MiB; visible-session ingest 2 MiB.
const EXTENSION_BODY_LIMIT: usize = 8 * 1024 * 1024;
const INGEST_BODY_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnterpriseRole {
    Owner,
    Admin,
    Member,
}

impl EnterpriseRole {
    fn as_str(&self) -> &'static str {
        match self {
            EnterpriseRole::Owner => "OWNER",
            EnterpriseRole::Admin => "ADMIN",
            EnterpriseRole::Member => "MEMBER",
        }
    }
}

#[derive(Clone, Debug)]
pub struct EnterpriseMember {
    pub id: String,
    pub organization_id: String,
    pub organization_name: String,
    pub display_name: String,
    pub email: String,
    pub role: EnterpriseRole,
    pub must_change_password: bool,
}

fn member(value: &Value) -> Result<EnterpriseMember, String> {
    if !value.is_object() {
        return Err("企业后台返回了无效的成员信息".to_string());
    }
    let object = value.as_object().unwrap();
    let strings = ["id", "organizationId", "organizationName", "displayName", "email", "role"];
    if strings.iter().any(|key| !object.get(*key).and_then(Value::as_str).is_some_and(|value| !value.is_empty())) {
        return Err("企业后台返回了无效的成员信息".to_string());
    }
    if !object.get("mustChangePassword").and_then(Value::as_bool).is_some() {
        return Err("企业后台返回了无效的成员信息".to_string());
    }
    let role = match object.get("role").and_then(Value::as_str).unwrap_or_default() {
        "OWNER" => EnterpriseRole::Owner,
        "ADMIN" => EnterpriseRole::Admin,
        "MEMBER" => EnterpriseRole::Member,
        _ => return Err("企业后台返回了无效的成员信息".to_string()),
    };
    if object.get("mustChangePassword").and_then(Value::as_bool).unwrap_or(false) {
        return Err("请先在企业后台修改初始密码，再登录 Desktop".to_string());
    }
    Ok(EnterpriseMember {
        id: object.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
        organization_id: object.get("organizationId").and_then(Value::as_str).unwrap_or_default().to_string(),
        organization_name: object.get("organizationName").and_then(Value::as_str).unwrap_or_default().to_string(),
        display_name: object.get("displayName").and_then(Value::as_str).unwrap_or_default().to_string(),
        email: object.get("email").and_then(Value::as_str).unwrap_or_default().to_string(),
        role,
        must_change_password: object.get("mustChangePassword").and_then(Value::as_bool).unwrap_or_default(),
    })
}

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
    member: Option<Value>,
}

pub struct EnterpriseLogin {
    backend_url: String,
    token: Arc<tokio::sync::Mutex<String>>,
    pub actor: EnterpriseMember,
    client: reqwest::Client,
    lifetime: CancellationToken,
}

impl EnterpriseLogin {
    pub async fn login(origin: &str, account: &str, password: &str) -> Result<Self, String> {
        let backend = match crate::connection::enterprise_connection(origin)? {
            crate::connection::DesktopConnection::Enterprise { portal_origin, .. } => portal_origin,
            _ => unreachable!(),
        };
        if account.trim().is_empty()
            || password.is_empty()
            // `account.length > 254` counts UTF-16 units, not bytes.
            || account.encode_utf16().count() > 254
            || password.encode_utf16().count() > 512
        {
            return Err("请输入账号和密码".to_string());
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        let response = client
            .post(format!("{backend}/api/auth/login"))
            .json(&json!({ "email": account.trim(), "password": password }))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        let response = match response {
            Ok(response) if response.status().is_redirection() => {
                return Err("无法连接企业后台，请检查地址、证书和网络".to_string());
            }
            Ok(response) => response,
            Err(_) => return Err("无法连接企业后台，请检查地址、证书和网络".to_string()),
        };
        if !response.status().is_success() {
            let status = response.status();
            return Err(if status == StatusCode::UNAUTHORIZED { "账号或密码不正确".to_string() } else { format!("企业登录失败 ({})", status.as_u16()) });
        }
        let parsed: LoginResponse = response.json().await.map_err(|_| "企业登录响应无效".to_string())?;
        static TOKEN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let token_pattern = TOKEN.get_or_init(|| Regex::new(r"^[A-Za-z0-9_-]{32,256}$").unwrap());
        if !token_pattern.is_match(&parsed.token) {
            return Err("企业登录响应无效".to_string());
        }
        let token = parsed.token;
        match Self::verify_member(&client, &backend, &token).await {
            Ok(actor) => {
                let admitted = parsed.member.as_ref();
                let admitted_id = admitted.and_then(|value| value.get("id")).and_then(Value::as_str);
                let admitted_organization = admitted.and_then(|value| value.get("organizationId")).and_then(Value::as_str);
                if admitted_id != Some(actor.id.as_str()) || admitted_organization != Some(actor.organization_id.as_str()) {
                    return Err("企业账号归属不一致".to_string());
                }
                Ok(Self {
                    backend_url: backend,
                    token: Arc::new(tokio::sync::Mutex::new(token)),
                    actor,
                    client,
                    lifetime: CancellationToken::new(),
                })
            }
            Err(error) => {
                // Login already created a backend session, even though local admission has not succeeded.
                let mut revoked = false;
                if let Ok(response) = client
                    .post(format!("{backend}/api/auth/logout"))
                    .bearer_auth(&token)
                    .timeout(REQUEST_TIMEOUT)
                    .send()
                    .await
                {
                    revoked = response.status().is_success() || response.status() == StatusCode::UNAUTHORIZED;
                }
                if !revoked {
                    return Err(format!("{error}；后台登录撤销尚未确认"));
                }
                Err(error)
            }
        }
    }

    async fn verify_member(client: &reqwest::Client, backend: &str, token: &str) -> Result<EnterpriseMember, String> {
        let response = client
            .get(format!("{backend}/api/auth/me"))
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| "无法核验企业账号，请检查后台连接".to_string())?;
        if response.status().is_redirection() {
            return Err("无法核验企业账号，请检查后台连接".to_string());
        }
        if !response.status().is_success() {
            return Err("企业账号核验失败".to_string());
        }
        let body: Value = response.json().await.map_err(|_| "企业后台返回了无效的成员信息".to_string())?;
        member(&body)
    }

    pub fn backend_url(&self) -> &str {
        &self.backend_url
    }

    pub async fn request(&self, path: &str, method: Method, body: Option<String>) -> Result<reqwest::Response, String> {
        if self.lifetime.is_cancelled() {
            return Err("企业登录已退出".to_string());
        }
        let token = self.token.lock().await.clone();
        let mut request = self.client.request(method, format!("{}{path}", self.backend_url)).bearer_auth(token);
        if let Some(body) = body {
            request = request.header("Content-Type", "application/json").body(body);
        }
        let request = request.timeout(REQUEST_TIMEOUT);
        tokio::select! {
            _ = self.lifetime.cancelled() => Err("企业登录已退出".to_string()),
            response = request.send() => response.map_err(|_| "无法连接企业后台，请检查地址、证书和网络".to_string()),
        }
    }

    pub fn cancel_requests(&self) {
        self.lifetime.cancel();
    }

    pub async fn verify(&self) -> Result<EnterpriseMember, String> {
        let response = self.request("/api/auth/me", Method::GET, None).await?;
        if !response.status().is_success() {
            return Err(format!("企业认证无效 ({})", response.status().as_u16()));
        }
        let body: Value = response.json().await.map_err(|_| "企业后台返回了无效的成员信息".to_string())?;
        let current = member(&body)?;
        if current.id != self.actor.id || current.organization_id != self.actor.organization_id {
            return Err("企业账号归属已改变，请重新登录".to_string());
        }
        Ok(current)
    }

    pub async fn logout(&self) -> bool {
        let token = {
            let mut guard = self.token.lock().await;
            std::mem::take(&mut *guard)
        };
        self.cancel_requests();
        if token.is_empty() {
            return true;
        }
        match self
            .client
            .post(format!("{}/api/auth/logout", self.backend_url))
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
        {
            Ok(response) => response.status().is_success() || response.status() == StatusCode::UNAUTHORIZED,
            Err(_) => false,
        }
    }
}

pub struct Authority {
    pub url: String,
    pub key: String,
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl Authority {
    pub async fn close(self) {
        self.shutdown.send(()).ok();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }
}

/// A capability-protected loopback bridge with a closed set of member-authorized operations.
pub async fn start_enterprise_authority(
    login: Arc<EnterpriseLogin>,
    device_id: String,
    on_logout: Arc<dyn Fn() + Send + Sync>,
    on_invalid: Arc<dyn Fn() + Send + Sync>,
) -> Result<Authority, String> {
    let key = hex::encode({
        let mut bytes = [0u8; 32];
        rand::Rng::fill(&mut rand::thread_rng(), &mut bytes[..]);
        bytes
    });
    let state = AuthorityState {
        expected: format!("Bearer {key}"),
        device_id,
        login,
        closing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        on_logout,
        on_invalid,
    };
    let app = Router::new()
        .fallback(authority_handler)
        // Node reads the body stream itself and cuts at 8 MiB (collaboration) or
        // 2 MiB (ingest); axum's 2 MiB default would reject valid collaboration
        // payloads, so the transport limit is the larger one and ingest re-checks.
        .layer(DefaultBodyLimit::max(EXTENSION_BODY_LIMIT))
        // The 15s deadline mirrors `AbortSignal.timeout(15_000)`; the timeout layer
        // produces a `BoxError`, so the error must be mapped back to a response.
        .layer(
            tower::ServiceBuilder::new()
                .layer(axum::error_handling::HandleErrorLayer::new(
                    |error: tower::BoxError| async move {
                        let _ = error;
                        StatusCode::REQUEST_TIMEOUT
                    },
                ))
                .layer(tower::timeout::TimeoutLayer::new(Duration::from_secs(15))),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_signal.await;
            })
            .await;
    });
    Ok(Authority { url: format!("http://127.0.0.1:{}", address.port()), key, shutdown, task })
}

#[derive(Clone)]
struct AuthorityState {
    /// `Bearer <key>`; the key itself is handed to the provider through the patch,
    /// so the server only needs the expected header value.
    expected: String,
    device_id: String,
    login: Arc<EnterpriseLogin>,
    closing: Arc<std::sync::atomic::AtomicBool>,
    on_logout: Arc<dyn Fn() + Send + Sync>,
    on_invalid: Arc<dyn Fn() + Send + Sync>,
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (status, [("cache-control", "no-store"), ("content-type", "application/json")], axum::Json(body)).into_response()
}

fn reject(status: u16, message: &str) -> Response {
    json_response(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), json!({ "error": message }))
}

/// Constant-time comparison, like timingSafeEqual.
fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (first, second) in left.iter().zip(right.iter()) {
        difference |= first ^ second;
    }
    difference == 0
}

async fn authority_handler(State(state): State<AuthorityState>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let response = json_response(StatusCode::UNAUTHORIZED, json!({ "error": "Desktop authentication required" }));
    if state.closing.load(std::sync::atomic::Ordering::SeqCst) {
        return reject(401, "Desktop is signing out");
    }
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .as_bytes();
    if !constant_time_equal(supplied, state.expected.as_bytes()) {
        return reject(401, "Desktop authentication required");
    }
    if headers.contains_key("origin") {
        return reject(403, "Browser requests are not admitted");
    }
    let path = uri.path();
    let has_query = uri.query().is_some_and(|query| !query.is_empty());

    if path == "/auth/logout" && method == Method::POST && !has_query {
        state.closing.store(true, std::sync::atomic::Ordering::SeqCst);
        state.login.cancel_requests();
        let on_logout = state.on_logout.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            on_logout();
        });
        return json_response(StatusCode::OK, json!({ "local": true }));
    }

    let auth = matches!(path, "/auth/me" | "/api/auth/me") && method == Method::GET && !has_query;
    let ingest = path == "/visible-sessions/ingest" && matches!(method, Method::GET | Method::POST | Method::DELETE);
    let collaboration = !has_query
        && ((method == Method::GET && collaboration_get_pattern().is_match(path))
            || (method == Method::POST && collaboration_post_pattern().is_match(path)));
    let extension = !has_query
        && matches!(method, Method::GET | Method::POST)
        && extension_pattern().is_match(path);
    if !auth && !ingest && !collaboration && !extension {
        return reject(404, "Unknown Desktop operation");
    }
    let current = match state.login.verify().await {
        Ok(current) => current,
        Err(_) => {
            reject(401, "Enterprise authorization unavailable");
            if !state.closing.swap(true, std::sync::atomic::Ordering::SeqCst) {
                state.login.cancel_requests();
                let on_invalid = state.on_invalid.clone();
                tokio::spawn(async move { on_invalid() });
            }
            return response;
        }
    };
    if state.closing.load(std::sync::atomic::Ordering::SeqCst) {
        return reject(401, "Desktop is signing out");
    }
    if auth {
        return json_response(
            StatusCode::OK,
            json!({
                "id": current.id,
                "organizationId": current.organization_id,
                "organizationName": current.organization_name,
                "displayName": current.display_name,
                "email": current.email,
                "role": current.role.as_str(),
                "mustChangePassword": current.must_change_password,
                "deviceId": state.device_id,
                "backendUrl": state.login.backend_url,
            }),
        );
    }
    if collaboration || extension {
        let mut body_text: Option<String> = None;
        if method == Method::POST {
            let content_type = headers
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !content_type.starts_with("application/json") {
                return reject(400, "JSON required");
            }
            body_text = Some(String::from_utf8_lossy(&body).to_string());
            if serde_json::from_str::<Value>(body_text.as_deref().unwrap_or("")).is_err() {
                return reject(502, if extension { "Enterprise extension unavailable" } else { "Enterprise collaboration unavailable" });
            }
        }
        match state.login.request(path, method.clone(), body_text).await {
            Ok(upstream) => {
                if state.closing.load(std::sync::atomic::Ordering::SeqCst) {
                    return response;
                }
                if !upstream.status().is_success() {
                    return reject(upstream.status().as_u16(), if extension { "Enterprise extension rejected" } else { "Enterprise collaboration rejected" });
                }
                match upstream.json::<Value>().await {
                    Ok(value) => json_response(StatusCode::OK, value),
                    Err(_) => reject(502, if extension { "Enterprise extension unavailable" } else { "Enterprise collaboration unavailable" }),
                }
            }
            Err(_) => reject(502, if extension { "Enterprise extension unavailable" } else { "Enterprise collaboration unavailable" }),
        }
    } else {
        // ingest
        let mut body_text: Option<String> = None;
        let mut target = "/api/member/visible-sessions".to_string();
        if method == Method::POST {
            let content_type = headers
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !content_type.starts_with("application/json") {
                return reject(400, "JSON required");
            }
            if body.len() > INGEST_BODY_LIMIT {
                return reject(413, "Body too large");
            }
            body_text = Some(String::from_utf8_lossy(&body).to_string());
            let parsed: Value = match serde_json::from_str(body_text.as_deref().unwrap_or("")) {
                Ok(value) => value,
                Err(_) => return reject(502, "Enterprise body synchronization unavailable"),
            };
            if parsed.get("deviceId").and_then(Value::as_str) != Some(state.device_id.as_str()) {
                return reject(403, "Device mismatch");
            }
        } else {
            // GET/DELETE carry the session id in the query, exactly like the
            // Electron carrier's `else` branch.
            let session = session_id_query(uri.query());
            let Some(session) = session else {
                return reject(400, "Session required");
            };
            target = format!("/api/member/visible-sessions?{}", urlencoded_form(&[("deviceId", state.device_id.as_str()), ("sessionId", session.as_str())]));
        }
        if state.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return reject(401, "Desktop is signing out");
        }
        match state.login.request(&target, method, body_text).await {
            Ok(upstream) => {
                if state.closing.load(std::sync::atomic::Ordering::SeqCst) {
                    return response;
                }
                if !upstream.status().is_success() {
                    return reject(upstream.status().as_u16(), "Enterprise body synchronization rejected");
                }
                match upstream.json::<Value>().await {
                    Ok(value) => json_response(StatusCode::OK, value),
                    Err(_) => reject(502, "Enterprise body synchronization unavailable"),
                }
            }
            Err(_) => reject(502, "Enterprise body synchronization unavailable"),
        }
    }
}

/// Only `sessionId`, exactly once, at most 256 characters.
fn session_id_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    let mut session: Option<String> = None;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=')?;
        if key != "sessionId" {
            return None;
        }
        if session.is_some() {
            return None;
        }
        let decoded = url_decode(value);
        // `session.length > 256` counts UTF-16 units.
        if decoded.encode_utf16().count() > 256 {
            return None;
        }
        session = Some(decoded);
    }
    session
}

fn url_decode(value: &str) -> String {
    let mut result = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    result.push(byte);
                    index += 3;
                    continue;
                }
                result.push(bytes[index]);
                index += 1;
            }
            b'+' => {
                result.push(b' ');
                index += 1;
            }
            byte => {
                result.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&result).to_string()
}

/// application/x-www-form-urlencoded, like URLSearchParams.
fn urlencoded_form(pairs: &[(&str, &str)]) -> String {
    fn encode(value: &str) -> String {
        let mut result = String::new();
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => result.push(byte as char),
                b' ' => result.push('+'),
                byte => result.push_str(&format!("%{byte:02X}")),
            }
        }
        result
    }
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn collaboration_get_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    // `[\w-]` is Unicode-aware in Rust, matching the JavaScript `u` flag.
    PATTERN.get_or_init(|| {
        Regex::new(r"^/api/collaboration/(?:contract|colleagues|inbox|sent|notifications|(?:materials|handoffs)/[\w-]{1,160}|handoffs/[\w-]{1,160}/messages)$").unwrap()
    })
}

fn collaboration_post_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^/api/collaboration/(?:materials|handoffs|notifications/[\w-]{1,160}/read|handoffs/[\w-]{1,160}/(?:messages|complete))$").unwrap()
    })
}

fn extension_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^/api/extensions/[a-z][a-z0-9-]{0,63}/[a-z][a-z0-9-]{0,63}$").unwrap())
}
