use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{
        HeaderMap, HeaderName, HeaderValue, StatusCode, Uri,
        header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, ORIGIN, RETRY_AFTER, SET_COOKIE},
        uri::Authority,
    },
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::agent::source_ip;
use crate::{
    app::AppState,
    auth::{
        SESSION_DURATION_SECONDS, constant_time_token_eq, decode_hex, encode_hex, hash_password,
        random_token, sha256, unix_timestamp, validate_password, verify_password,
    },
    database::{Database, DatabaseError},
};

const JSON_BODY_LIMIT: usize = 4 * 1_024;
const SESSION_COOKIE: &str = "__Host-monitor_session";
const CSRF_COOKIE: &str = "__Host-monitor_csrf";
const CSRF_HEADER: HeaderName = HeaderName::from_static("x-csrf-token");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginRequest {
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    authenticated: bool,
    expires_at: i64,
}

#[derive(Serialize)]
struct AuthMeResponse {
    authenticated: bool,
    expires_at: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordChangeRequest {
    current_password: String,
    new_password: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorObject,
}

#[derive(Serialize)]
struct ErrorObject {
    code: &'static str,
    message: &'static str,
}

pub(super) struct AuthenticatedSession {
    token_hash: [u8; 32],
    expires_at: i64,
}

pub(super) async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Result<Response, ApiError> {
    validate_origin(request.headers())?;
    // The limiter must key on the client, not on the reverse proxy that fronts
    // the default loopback listener; `source_ip` owns that frozen contract.
    let source = source_ip(peer.ip(), request.headers())?;
    if let Some(retry_after) = state.login_limiter.retry_after(source) {
        return Err(ApiError::rate_limited(retry_after));
    }

    let LoginRequest { password } = parse_json(request, JSON_BODY_LIMIT).await?;
    validate_password(&password).map_err(|_| ApiError::invalid_request())?;
    let _hash_permit = state
        .auth_hash_gate
        .acquire()
        .await
        .map_err(|_| ApiError::internal())?;
    if let Some(retry_after) = state.login_limiter.retry_after(source) {
        return Err(ApiError::rate_limited(retry_after));
    }
    let Some(password_hash) = state
        .database
        .load_admin_password_hash()
        .await
        .map_err(ApiError::database)?
    else {
        let _ = hash_password(password)
            .await
            .map_err(|_| ApiError::internal())?;
        tracing::warn!("admin password is not configured");
        return Err(login_failure(&state, source));
    };

    let password_valid = verify_password(password, password_hash.clone())
        .await
        .map_err(|_| ApiError::internal())?;
    if !password_valid {
        return Err(login_failure(&state, source));
    }
    let session_token = random_token().map_err(|_| ApiError::internal())?;
    let csrf_token = random_token().map_err(|_| ApiError::internal())?;
    let now = unix_timestamp().map_err(|_| ApiError::internal())?;
    let expires_at = now
        .checked_add(SESSION_DURATION_SECONDS)
        .ok_or_else(ApiError::internal)?;
    state
        .database
        .delete_expired_sessions(now)
        .await
        .map_err(ApiError::database)?;
    let session_created = state
        .database
        .create_session(password_hash, sha256(&session_token), now, expires_at)
        .await
        .map_err(ApiError::database)?;
    if !session_created {
        return Err(ApiError::invalid_credentials());
    }
    state.login_limiter.clear(source);

    let mut response = json_response(
        StatusCode::OK,
        LoginResponse {
            authenticated: true,
            expires_at,
        },
    );
    append_login_cookies(
        response.headers_mut(),
        &encode_hex(&session_token),
        &encode_hex(&csrf_token),
    );
    Ok(response)
}

pub(super) async fn me(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    let session = authenticate_session(&state.database, request.headers()).await?;
    Ok(json_response(
        StatusCode::OK,
        AuthMeResponse {
            authenticated: true,
            expires_at: session.expires_at,
        },
    ))
}

pub(super) async fn logout(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    let session = authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    state
        .database
        .delete_session(session.token_hash)
        .await
        .map_err(ApiError::database)?;

    let mut response = no_content_response();
    append_cleared_cookies(response.headers_mut());
    Ok(response)
}

pub(super) async fn change_password(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    authenticate_session(&state.database, request.headers()).await?;
    validate_csrf(request.headers())?;
    let PasswordChangeRequest {
        current_password,
        new_password,
    } = parse_json(request, JSON_BODY_LIMIT).await?;
    validate_password(&current_password).map_err(|_| ApiError::invalid_request())?;
    validate_password(&new_password).map_err(|_| ApiError::invalid_request())?;
    if current_password == new_password {
        return Err(ApiError::invalid_request());
    }

    let hash_permit = state
        .auth_hash_gate
        .acquire()
        .await
        .map_err(|_| ApiError::internal())?;
    let Some(current_hash) = state
        .database
        .load_admin_password_hash()
        .await
        .map_err(ApiError::database)?
    else {
        return Err(ApiError::invalid_credentials());
    };
    let password_valid = verify_password(current_password, current_hash.clone())
        .await
        .map_err(|_| ApiError::internal())?;
    if !password_valid {
        return Err(ApiError::invalid_credentials());
    }

    let new_hash = hash_password(new_password)
        .await
        .map_err(|_| ApiError::internal())?;
    drop(hash_permit);
    let updated = state
        .database
        .change_admin_password(
            current_hash,
            new_hash,
            unix_timestamp().map_err(|_| ApiError::internal())?,
        )
        .await
        .map_err(ApiError::database)?;
    if !updated {
        return Err(ApiError::invalid_credentials());
    }

    let mut response = no_content_response();
    append_cleared_cookies(response.headers_mut());
    Ok(response)
}

pub(super) async fn authenticate_session(
    database: &Database,
    headers: &HeaderMap,
) -> Result<AuthenticatedSession, ApiError> {
    let token = cookie_value(headers, SESSION_COOKIE).ok_or_else(ApiError::unauthorized)?;
    let token = decode_hex(&token).ok_or_else(ApiError::unauthorized)?;
    let token_hash = sha256(&token);
    let session = database
        .find_session(token_hash)
        .await
        .map_err(ApiError::database)?
        .ok_or_else(ApiError::unauthorized)?;
    let now = unix_timestamp().map_err(|_| ApiError::internal())?;
    if session.expires_at <= now {
        if let Err(error) = database.delete_session(token_hash).await {
            tracing::warn!(error = %error, "failed to remove expired administrator session");
        }
        return Err(ApiError::unauthorized());
    }

    Ok(AuthenticatedSession {
        token_hash,
        expires_at: session.expires_at,
    })
}

pub(super) fn validate_csrf(headers: &HeaderMap) -> Result<(), ApiError> {
    validate_origin(headers)?;
    let cookie = cookie_value(headers, CSRF_COOKIE).ok_or_else(ApiError::csrf_failed)?;
    let header = single_header(headers, &CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ApiError::csrf_failed)?;
    let cookie = decode_hex(&cookie).ok_or_else(ApiError::csrf_failed)?;
    let header = decode_hex(header).ok_or_else(ApiError::csrf_failed)?;
    if !constant_time_token_eq(&cookie, &header) {
        return Err(ApiError::csrf_failed());
    }
    Ok(())
}

fn validate_origin(headers: &HeaderMap) -> Result<(), ApiError> {
    let host = single_header(headers, &HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Authority::from_str(value).ok())
        .filter(|authority| !authority.as_str().contains('@'))
        .ok_or_else(ApiError::csrf_failed)?;
    let origin = single_header(headers, &ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uri::from_str(value).ok())
        .ok_or_else(ApiError::csrf_failed)?;
    let scheme = origin.scheme_str().ok_or_else(ApiError::csrf_failed)?;
    let authority = origin.authority().ok_or_else(ApiError::csrf_failed)?;
    if origin.path() != "/" || origin.query().is_some() {
        return Err(ApiError::csrf_failed());
    }
    if scheme != "https" && !(scheme == "http" && is_loopback_host(host.host())) {
        return Err(ApiError::csrf_failed());
    }

    let default_port = if scheme == "https" { 443 } else { 80 };
    let hosts_equal = host.host().eq_ignore_ascii_case(authority.host());
    let ports_equal =
        host.port_u16().unwrap_or(default_port) == authority.port_u16().unwrap_or(default_port);
    if !hosts_equal || !ports_equal {
        return Err(ApiError::csrf_failed());
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

pub(super) async fn parse_json<T: DeserializeOwned>(
    request: Request,
    body_limit: usize,
) -> Result<T, ApiError> {
    single_header(request.headers(), &CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| value.eq_ignore_ascii_case("application/json"))
        .ok_or_else(ApiError::unsupported_media_type)?;

    let body = to_bytes(request.into_body(), body_limit)
        .await
        .map_err(|_| ApiError::payload_too_large(body_limit))?;
    serde_json::from_slice(&body).map_err(|_| ApiError::invalid_request())
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(COOKIE) {
        // An unrelated malformed header or segment must not hide a valid
        // session or CSRF cookie, so skip it instead of abandoning the search.
        let Ok(value) = value.to_str() else {
            continue;
        };
        for cookie in value.split(';') {
            let Some((cookie_name, cookie_value)) = cookie.trim().split_once('=') else {
                continue;
            };
            if cookie_name == name {
                // A duplicated target cookie stays ambiguous and is rejected.
                if found.is_some() {
                    return None;
                }
                found = Some(cookie_value.to_owned());
            }
        }
    }
    found
}

fn single_header<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn login_failure(state: &AppState, address: IpAddr) -> ApiError {
    match state.login_limiter.record_failure(address) {
        Some(retry_after) => ApiError::rate_limited(retry_after),
        None => ApiError::invalid_credentials(),
    }
}

fn append_login_cookies(headers: &mut HeaderMap, session: &str, csrf: &str) {
    let max_age = SESSION_DURATION_SECONDS;
    let session = format!(
        "{SESSION_COOKIE}={session}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Strict"
    );
    let csrf = format!("{CSRF_COOKIE}={csrf}; Path=/; Max-Age={max_age}; Secure; SameSite=Strict");
    headers.append(
        SET_COOKIE,
        HeaderValue::from_str(&session).expect("generated session cookie is a valid header"),
    );
    headers.append(
        SET_COOKIE,
        HeaderValue::from_str(&csrf).expect("generated CSRF cookie is a valid header"),
    );
}

fn append_cleared_cookies(headers: &mut HeaderMap) {
    headers.append(
        SET_COOKIE,
        HeaderValue::from_static(
            "__Host-monitor_session=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict",
        ),
    );
    headers.append(
        SET_COOKIE,
        HeaderValue::from_static(
            "__Host-monitor_csrf=; Path=/; Max-Age=0; Secure; SameSite=Strict",
        ),
    );
}

pub(super) fn json_response(status: StatusCode, value: impl Serialize) -> Response {
    let mut response = (status, Json(value)).into_response();
    set_no_store(response.headers_mut());
    response
}

pub(super) fn no_content_response() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    set_no_store(response.headers_mut());
    response
}

fn set_no_store(headers: &mut HeaderMap) {
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

pub(super) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retry_after: Option<u64>,
}

impl ApiError {
    pub(super) fn invalid_request() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid request",
        )
    }

    pub(super) fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "unauthorized")
    }

    fn invalid_credentials() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "invalid credentials",
        )
    }

    fn csrf_failed() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "CSRF validation failed",
        )
    }

    pub(super) fn payload_too_large(body_limit: usize) -> Self {
        let message = match body_limit {
            4_096 => "request body exceeds 4096 bytes",
            8_192 => "request body exceeds 8192 bytes",
            _ => "request body exceeds limit",
        };
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", message)
    }

    pub(super) fn unsupported_media_type() -> Self {
        Self::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type must be application/json",
        )
    }

    fn rate_limited(retry_after: u64) -> Self {
        let mut error = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many login attempts",
        );
        error.retry_after = Some(retry_after);
        error
    }

    pub(super) fn database(error: DatabaseError) -> Self {
        tracing::error!(error = %error, "API database operation failed");
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "service unavailable",
        )
    }

    pub(super) fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal server error",
        )
    }

    pub(super) fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }

    pub(super) fn conflict() -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", "conflict")
    }

    const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            retry_after: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = json_response(
            self.status,
            ErrorResponse {
                error: ErrorObject {
                    code: self.code,
                    message: self.message,
                },
            },
        );
        if let Some(seconds) = self.retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(RETRY_AFTER, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::{SocketAddr, TcpStream},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use rusqlite::Connection;

    use super::*;
    use crate::{
        app::AppState,
        database::{Database, hydrate_startup},
        http::router,
    };

    static TEST_SERVER_ID: AtomicU64 = AtomicU64::new(0);
    const OLD_PASSWORD: &str = "correct horse battery staple";
    const NEW_PASSWORD: &str = "a different secure passphrase";
    const ORIGIN_HEADER: (&str, &str) = ("Origin", "https://monitor.test");
    const JSON_HEADER: (&str, &str) = ("Content-Type", "application/json");

    struct TestServer {
        address: SocketAddr,
        path: PathBuf,
        state: AppState,
        task: tokio::task::JoinHandle<()>,
    }

    struct TestResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl TestServer {
        async fn start(password: Option<&str>) -> Self {
            let id = TEST_SERVER_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("monitor-auth-http-{}-{id}.db", std::process::id()));
            remove_database_files(&path);
            let database = Database::open(&path).expect("open test database");
            if let Some(password) = password {
                database
                    .set_admin_password(
                        hash_password(password.to_owned())
                            .await
                            .expect("hash test password"),
                        unix_timestamp().expect("test timestamp"),
                    )
                    .await
                    .expect("set test administrator password");
            }
            let hydration = hydrate_startup(&database)
                .await
                .expect("hydrate test state");
            let state = AppState::new(database, hydration);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind test server");
            let address = listener.local_addr().expect("test server address");
            let service = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
            let task = tokio::spawn(async move {
                axum::serve(listener, service)
                    .await
                    .expect("serve test requests");
            });
            Self {
                address,
                path,
                state,
                task,
            }
        }

        async fn request(
            &self,
            method: &str,
            path: &str,
            headers: &[(&str, &str)],
            body: &str,
        ) -> TestResponse {
            let mut request = format!(
                "{method} {path} HTTP/1.1\r\nHost: monitor.test\r\nConnection: close\r\nContent-Length: {}\r\n",
                body.len()
            );
            for (name, value) in headers {
                request.push_str(name);
                request.push_str(": ");
                request.push_str(value);
                request.push_str("\r\n");
            }
            request.push_str("\r\n");
            request.push_str(body);
            let address = self.address;

            let raw = tokio::task::spawn_blocking(move || send_http_request(address, &request))
                .await
                .expect("HTTP client task")
                .expect("HTTP request");
            TestResponse::parse(&raw)
        }

        async fn login(&self, password: &str) -> TestResponse {
            let body = serde_json::json!({ "password": password }).to_string();
            self.request(
                "POST",
                "/api/auth/login",
                &[ORIGIN_HEADER, JSON_HEADER],
                &body,
            )
            .await
        }

        /// Logs in the way a reverse proxy fronting the loopback listener does.
        async fn login_from(&self, client: &str, password: &str) -> TestResponse {
            let body = serde_json::json!({ "password": password }).to_string();
            self.request(
                "POST",
                "/api/auth/login",
                &[ORIGIN_HEADER, JSON_HEADER, ("X-Real-IP", client)],
                &body,
            )
            .await
        }

        async fn finish(self) {
            self.task.abort();
            let _ = self.task.await;
            self.state
                .database
                .shutdown()
                .await
                .expect("shutdown test database");
            remove_database_files(&self.path);
        }
    }

    impl TestResponse {
        fn parse(raw: &str) -> Self {
            let (head, body) = raw.split_once("\r\n\r\n").expect("HTTP response head");
            let mut lines = head.lines();
            let status = lines
                .next()
                .expect("HTTP status line")
                .split_whitespace()
                .nth(1)
                .expect("HTTP status")
                .parse()
                .expect("numeric HTTP status");
            let headers = lines
                .map(|line| {
                    let (name, value) = line.split_once(':').expect("HTTP response header");
                    (name.to_ascii_lowercase(), value.trim().to_owned())
                })
                .collect();
            Self {
                status,
                headers,
                body: body.to_owned(),
            }
        }

        fn header_values(&self, name: &str) -> Vec<&str> {
            self.headers
                .iter()
                .filter_map(|(header, value)| (header == name).then_some(value.as_str()))
                .collect()
        }

        fn error_code(&self) -> String {
            let body: serde_json::Value =
                serde_json::from_str(&self.body).expect("JSON error response");
            body["error"]["code"]
                .as_str()
                .expect("error code")
                .to_owned()
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrong_password_unknown_field_body_limit_and_content_type_are_enforced() {
        let server = TestServer::start(Some(OLD_PASSWORD)).await;

        let wrong = server.login("wrong password").await;
        assert_eq!(wrong.status, 401);
        assert_eq!(wrong.error_code(), "invalid_credentials");
        assert!(wrong.header_values("set-cookie").is_empty());
        assert_eq!(
            server
                .state
                .database
                .delete_all_sessions()
                .await
                .expect("count sessions by deleting"),
            0
        );

        let unknown = server
            .request(
                "POST",
                "/api/auth/login",
                &[ORIGIN_HEADER, JSON_HEADER],
                r#"{"password":"x","username":"admin"}"#,
            )
            .await;
        assert_eq!(unknown.status, 400);
        assert_eq!(unknown.error_code(), "invalid_request");

        let unsupported = server
            .request(
                "POST",
                "/api/auth/login",
                &[ORIGIN_HEADER, ("Content-Type", "text/plain")],
                r#"{"password":"x"}"#,
            )
            .await;
        assert_eq!(unsupported.status, 415);
        assert_eq!(unsupported.error_code(), "unsupported_media_type");

        let oversized_body = format!(r#"{{"password":"{}"}}"#, "x".repeat(4_100));
        let oversized = server
            .request(
                "POST",
                "/api/auth/login",
                &[ORIGIN_HEADER, JSON_HEADER],
                &oversized_body,
            )
            .await;
        assert_eq!(oversized.status, 413);
        assert_eq!(oversized.error_code(), "payload_too_large");

        server.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_admin_and_wrong_password_share_the_same_http_error() {
        let configured = TestServer::start(Some(OLD_PASSWORD)).await;
        let wrong_password = configured.login("wrong password").await;
        let wrong_status = wrong_password.status;
        let wrong_body = wrong_password.body.clone();
        configured.finish().await;

        let unconfigured = TestServer::start(None).await;
        let missing_admin = unconfigured.login("wrong password").await;
        assert_eq!(missing_admin.status, wrong_status);
        assert_eq!(missing_admin.body, wrong_body);
        unconfigured.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn login_session_cookies_database_and_me_are_correct() {
        let server = TestServer::start(Some(OLD_PASSWORD)).await;
        let login = server.login(OLD_PASSWORD).await;
        assert_eq!(login.status, 200);
        let cookies = login.header_values("set-cookie");
        assert_eq!(cookies.len(), 2);
        let session_cookie = cookie_line(cookies.as_slice(), SESSION_COOKIE);
        let csrf_cookie = cookie_line(cookies.as_slice(), CSRF_COOKIE);
        assert!(session_cookie.contains("HttpOnly"));
        assert!(session_cookie.contains("Secure"));
        assert!(session_cookie.contains("SameSite=Strict"));
        assert!(session_cookie.contains("Path=/"));
        assert!(!session_cookie.contains("Domain="));
        assert!(!csrf_cookie.contains("HttpOnly"));
        assert!(csrf_cookie.contains("Secure"));
        assert!(csrf_cookie.contains("SameSite=Strict"));
        assert!(csrf_cookie.contains("Path=/"));
        assert!(!csrf_cookie.contains("Domain="));

        let session_pair = cookie_pair(session_cookie);
        let session_value = session_pair.split_once('=').expect("session value").1;
        let session_raw = decode_hex(session_value).expect("session token format");
        let token_hash = sha256(&session_raw);
        assert!(
            server
                .state
                .database
                .find_session(token_hash)
                .await
                .expect("find login session")
                .is_some()
        );
        let connection = Connection::open(&server.path).expect("inspect session database");
        let (storage_type, stored_length): (String, i64) = connection
            .query_row(
                "SELECT typeof(token_hash), length(token_hash) FROM sessions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("inspect stored session token");
        assert_eq!(storage_type, "blob");
        assert_eq!(stored_length, 32);
        drop(connection);

        let me = server
            .request("GET", "/api/auth/me", &[("Cookie", &session_pair)], "")
            .await;
        assert_eq!(me.status, 200);
        let missing = server.request("GET", "/api/auth/me", &[], "").await;
        assert_eq!(missing.status, 401);
        let wrong = server
            .request(
                "GET",
                "/api/auth/me",
                &[(
                    "Cookie",
                    "__Host-monitor_session=0000000000000000000000000000000000000000000000000000000000000000",
                )],
                "",
            )
            .await;
        assert_eq!(wrong.status, 401);

        let expired_raw = [9_u8; 32];
        let now = unix_timestamp().expect("test timestamp");
        let password_hash = server
            .state
            .database
            .load_admin_password_hash()
            .await
            .expect("load administrator hash")
            .expect("administrator exists");
        assert!(
            server
                .state
                .database
                .create_session(password_hash, sha256(&expired_raw), now - 2, now - 1)
                .await
                .expect("create expired session")
        );
        let expired_cookie = format!("{SESSION_COOKIE}={}", encode_hex(&expired_raw));
        let expired = server
            .request("GET", "/api/auth/me", &[("Cookie", &expired_cookie)], "")
            .await;
        assert_eq!(expired.status, 401);
        assert_eq!(
            server
                .state
                .database
                .find_session(sha256(&expired_raw))
                .await
                .expect("find expired session"),
            None
        );

        server.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn csrf_failures_are_rejected_and_logout_deletes_session() {
        let server = TestServer::start(Some(OLD_PASSWORD)).await;
        let login = server.login(OLD_PASSWORD).await;
        let cookies = login.header_values("set-cookie");
        let session_pair = cookie_pair(cookie_line(cookies.as_slice(), SESSION_COOKIE));
        let csrf_pair = cookie_pair(cookie_line(cookies.as_slice(), CSRF_COOKIE));
        let csrf_value = csrf_pair.split_once('=').expect("CSRF value").1;
        let both_cookies = format!("{session_pair}; {csrf_pair}");

        let cases = [
            vec![
                ORIGIN_HEADER,
                ("Cookie", session_pair.as_str()),
                ("X-CSRF-Token", csrf_value),
            ],
            vec![ORIGIN_HEADER, ("Cookie", both_cookies.as_str())],
            vec![
                ORIGIN_HEADER,
                ("Cookie", both_cookies.as_str()),
                (
                    "X-CSRF-Token",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                ),
            ],
            vec![
                ORIGIN_HEADER,
                ("Cookie", both_cookies.as_str()),
                ("X-CSRF-Token", "INVALID"),
            ],
            vec![
                ("Cookie", both_cookies.as_str()),
                ("X-CSRF-Token", csrf_value),
            ],
            vec![
                ("Origin", "https://evil.example"),
                ("Cookie", both_cookies.as_str()),
                ("X-CSRF-Token", csrf_value),
            ],
        ];
        for headers in cases {
            let response = server
                .request("POST", "/api/auth/logout", &headers, "")
                .await;
            assert_eq!(response.status, 403);
            assert_eq!(response.error_code(), "csrf_failed");
        }

        let logout = server
            .request(
                "POST",
                "/api/auth/logout",
                &[
                    ORIGIN_HEADER,
                    ("Cookie", both_cookies.as_str()),
                    ("X-CSRF-Token", csrf_value),
                ],
                "",
            )
            .await;
        assert_eq!(logout.status, 204);
        let cleared = logout.header_values("set-cookie");
        assert_eq!(cleared.len(), 2);
        assert!(cleared.iter().all(|cookie| cookie.contains("Max-Age=0")));
        let me = server
            .request(
                "GET",
                "/api/auth/me",
                &[("Cookie", session_pair.as_str())],
                "",
            )
            .await;
        assert_eq!(me.status, 401);

        server.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn password_change_is_atomic_invalidates_sessions_and_replaces_password() {
        let server = TestServer::start(Some(OLD_PASSWORD)).await;
        let first_login = server.login(OLD_PASSWORD).await;
        let second_login = server.login(OLD_PASSWORD).await;
        let first_cookies = first_login.header_values("set-cookie");
        let first_session = cookie_pair(cookie_line(first_cookies.as_slice(), SESSION_COOKIE));
        let first_csrf = cookie_pair(cookie_line(first_cookies.as_slice(), CSRF_COOKIE));
        let first_csrf_value = first_csrf.split_once('=').expect("CSRF value").1;
        let request_cookies = format!("{first_session}; {first_csrf}");
        let second_session = cookie_pair(cookie_line(
            second_login.header_values("set-cookie").as_slice(),
            SESSION_COOKIE,
        ));
        let wrong_current_body = serde_json::json!({
            "current_password": "wrong current password",
            "new_password": NEW_PASSWORD,
        })
        .to_string();
        let wrong_current = server
            .request(
                "PATCH",
                "/api/admin/password",
                &[
                    ORIGIN_HEADER,
                    JSON_HEADER,
                    ("Cookie", request_cookies.as_str()),
                    ("X-CSRF-Token", first_csrf_value),
                ],
                &wrong_current_body,
            )
            .await;
        assert_eq!(wrong_current.status, 401);
        assert_eq!(wrong_current.error_code(), "invalid_credentials");

        let body = serde_json::json!({
            "current_password": OLD_PASSWORD,
            "new_password": NEW_PASSWORD,
        })
        .to_string();

        let changed = server
            .request(
                "PATCH",
                "/api/admin/password",
                &[
                    ORIGIN_HEADER,
                    JSON_HEADER,
                    ("Cookie", request_cookies.as_str()),
                    ("X-CSRF-Token", first_csrf_value),
                ],
                &body,
            )
            .await;
        assert_eq!(changed.status, 204);
        assert!(
            changed
                .header_values("set-cookie")
                .iter()
                .all(|cookie| cookie.contains("Max-Age=0"))
        );
        for session in [first_session, second_session] {
            let me = server
                .request("GET", "/api/auth/me", &[("Cookie", &session)], "")
                .await;
            assert_eq!(me.status, 401);
        }

        let old_login = server.login(OLD_PASSWORD).await;
        assert_eq!(old_login.status, 401);
        let new_login = server.login(NEW_PASSWORD).await;
        assert_eq!(new_login.status, 200);

        server.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn login_origin_and_rate_limit_are_enforced_and_success_clears_failures() {
        let server = TestServer::start(Some(OLD_PASSWORD)).await;
        let body = serde_json::json!({ "password": OLD_PASSWORD }).to_string();
        let missing_origin = server
            .request("POST", "/api/auth/login", &[JSON_HEADER], &body)
            .await;
        assert_eq!(missing_origin.status, 403);
        let wrong_origin = server
            .request(
                "POST",
                "/api/auth/login",
                &[
                    ("Origin", "https://evil.example"),
                    ("X-Forwarded-Host", "evil.example"),
                    ("X-Forwarded-Proto", "https"),
                    JSON_HEADER,
                ],
                &body,
            )
            .await;
        assert_eq!(wrong_origin.status, 403);

        for _ in 0..4 {
            assert_eq!(server.login("wrong password").await.status, 401);
        }
        assert_eq!(server.login(OLD_PASSWORD).await.status, 200);
        for _ in 0..4 {
            assert_eq!(server.login("wrong password").await.status, 401);
        }
        let limited = server.login("wrong password").await;
        assert_eq!(limited.status, 429);
        assert_eq!(limited.error_code(), "rate_limited");
        assert_eq!(limited.header_values("retry-after").len(), 1);

        server.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_login_burst_rechecks_limit_after_hash_gate() {
        let server = TestServer::start(Some(OLD_PASSWORD)).await;
        let gate = server
            .state
            .auth_hash_gate
            .clone()
            .acquire_owned()
            .await
            .expect("acquire test hash gate");
        let responses = {
            let burst = async {
                tokio::join!(
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                    server.login("wrong password"),
                )
            };
            tokio::pin!(burst);
            tokio::select! {
                _ = &mut burst => panic!("login burst completed while the hash gate was held"),
                () = async {
                    for _ in 0..100 {
                        tokio::task::yield_now().await;
                    }
                } => {}
            }
            drop(gate);
            burst.await
        };
        let responses = [
            responses.0,
            responses.1,
            responses.2,
            responses.3,
            responses.4,
            responses.5,
            responses.6,
            responses.7,
            responses.8,
            responses.9,
            responses.10,
            responses.11,
        ];
        assert_eq!(
            responses
                .iter()
                .filter(|response| response.status == 401)
                .count(),
            4
        );
        assert_eq!(
            responses
                .iter()
                .filter(|response| response.status == 429)
                .count(),
            8
        );
        assert!(responses.iter().all(|response| {
            matches!(
                response.error_code().as_str(),
                "invalid_credentials" | "rate_limited"
            )
        }));

        server.finish().await;
    }

    #[tokio::test]
    async fn login_limiter_keys_on_the_proxied_client_not_the_reverse_proxy() {
        // Every request reaches the loopback listener from the proxy, so keying
        // on the socket peer would let one client lock out every other one.
        let server = TestServer::start(Some(OLD_PASSWORD)).await;
        const NOISY: &str = "198.51.100.10";
        const QUIET: &str = "198.51.100.20";

        for _ in 0..4 {
            assert_eq!(server.login_from(NOISY, "wrong password").await.status, 401);
        }
        let limited = server.login_from(NOISY, "wrong password").await;
        assert_eq!(limited.status, 429);
        assert_eq!(limited.error_code(), "rate_limited");

        // The blocked client stays blocked even with the correct password.
        assert_eq!(server.login_from(NOISY, OLD_PASSWORD).await.status, 429);
        // A different client behind the same proxy is untouched.
        assert_eq!(server.login_from(QUIET, OLD_PASSWORD).await.status, 200);
        // ... and so is a client that sends no proxy header at all.
        assert_eq!(server.login(OLD_PASSWORD).await.status, 200);

        server.finish().await;
    }

    #[tokio::test]
    async fn login_rejects_duplicate_and_malformed_proxy_headers() {
        let server = TestServer::start(Some(OLD_PASSWORD)).await;
        let body = serde_json::json!({ "password": OLD_PASSWORD }).to_string();

        let malformed = server
            .request(
                "POST",
                "/api/auth/login",
                &[ORIGIN_HEADER, JSON_HEADER, ("X-Real-IP", "not-an-ip")],
                &body,
            )
            .await;
        assert_eq!(malformed.status, 400);
        assert_eq!(malformed.error_code(), "invalid_request");

        let duplicated = server
            .request(
                "POST",
                "/api/auth/login",
                &[
                    ORIGIN_HEADER,
                    JSON_HEADER,
                    ("X-Real-IP", "198.51.100.1"),
                    ("X-Real-IP", "198.51.100.2"),
                ],
                &body,
            )
            .await;
        assert_eq!(duplicated.status, 400);
        assert_eq!(duplicated.error_code(), "invalid_request");

        // A rejected proxy header must not consume a limiter attempt either.
        assert_eq!(server.login(OLD_PASSWORD).await.status, 200);
        server.finish().await;
    }

    #[tokio::test]
    async fn unknown_api_path_uses_json_not_found_without_spa_fallback() {
        let server = TestServer::start(None).await;

        let api = server.request("GET", "/api/not-real", &[], "").await;
        assert_eq!(api.status, 404);
        assert_eq!(api.header_values("content-type"), vec!["application/json"]);
        assert_eq!(api.header_values("cache-control"), vec!["no-store"]);
        assert_eq!(api.error_code(), "not_found");
        assert!(!api.body.contains("<!doctype html>"));

        let spa = server
            .request("GET", "/not-a-real-spa-route", &[], "")
            .await;
        assert_eq!(spa.status, 200);
        assert!(spa.body.contains("<!doctype html>"));

        server.finish().await;
    }

    #[test]
    fn cookie_lookup_skips_unrelated_malformed_segments() {
        let session = "a".repeat(64);
        let valid = format!("{SESSION_COOKIE}={session}");

        for header in [
            valid.clone(),
            format!("bare; {valid}"),
            format!("{valid}; bare"),
            format!("foo=bar; {valid}"),
            format!("bare; foo=bar; {valid}; trailing"),
            format!("=leading-equals; {valid}"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(COOKIE, HeaderValue::from_str(&header).expect("cookie"));
            assert_eq!(
                cookie_value(&headers, SESSION_COOKIE).as_deref(),
                Some(session.as_str()),
                "header: {header}"
            );
        }

        // A malformed value in one header must not hide the cookie in another.
        let mut split = HeaderMap::new();
        split.append(COOKIE, HeaderValue::from_static("bare"));
        split.append(COOKIE, HeaderValue::from_str(&valid).expect("cookie"));
        assert_eq!(
            cookie_value(&split, SESSION_COOKIE).as_deref(),
            Some(session.as_str())
        );
    }

    #[test]
    fn cookie_lookup_rejects_duplicates_and_reports_absence() {
        let mut duplicated = HeaderMap::new();
        duplicated.insert(
            COOKIE,
            HeaderValue::from_static("__Host-monitor_session=a; __Host-monitor_session=b"),
        );
        assert_eq!(cookie_value(&duplicated, SESSION_COOKIE), None);

        let mut across_headers = HeaderMap::new();
        across_headers.append(COOKIE, HeaderValue::from_static("__Host-monitor_session=a"));
        across_headers.append(COOKIE, HeaderValue::from_static("__Host-monitor_session=b"));
        assert_eq!(cookie_value(&across_headers, SESSION_COOKIE), None);

        let mut absent = HeaderMap::new();
        absent.insert(COOKIE, HeaderValue::from_static("foo=bar; bare"));
        assert_eq!(cookie_value(&absent, SESSION_COOKIE), None);
        assert_eq!(cookie_value(&HeaderMap::new(), SESSION_COOKIE), None);
    }

    fn send_http_request(address: SocketAddr, request: &str) -> std::io::Result<String> {
        let mut stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.write_all(request.as_bytes())?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        String::from_utf8(response)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    fn cookie_line<'a>(cookies: &'a [&str], name: &str) -> &'a str {
        cookies
            .iter()
            .copied()
            .find(|cookie| cookie.starts_with(&format!("{name}=")))
            .expect("named cookie")
    }

    fn cookie_pair(cookie: &str) -> String {
        cookie.split(';').next().expect("cookie pair").to_owned()
    }

    fn remove_database_files(path: &Path) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove test database: {error}"),
            }
        }
    }
}
