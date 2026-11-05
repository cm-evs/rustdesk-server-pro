//! Account & API service for hbbs (added on top of the open-source server).
//!
//! Provides what the OSS server lacks and the official client expects from a
//! Pro-style deployment, all backed by SQLite:
//! - user accounts (first registered user becomes admin)
//! - web console: user management, device list, integration API tokens,
//!   server settings, and pre-bound client generation with download links
//! - client login: OIDC-style flow (`/api/oidc/auth` + browser login page +
//!   `/api/oidc/auth-query`), legacy password login (`/api/login`),
//!   `/api/login-options`
//! - device binding via `/api/heartbeat` and `/api/sysinfo`
//! - strategy push through heartbeat (e.g. hide-server-settings)
//!
//! Started from main.rs on `--api-listen` (default 0.0.0.0:21114).

use crate::api_pages;
use axum::{
    body::Body,
    extract::{ConnectInfo, Extension, Form, Path, Query},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use hbb_common::{anyhow, log, tokio, ResultType};
use serde_json::{json, Value};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    ConnectOptions as _, Row, SqlitePool,
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};

pub struct ApiState {
    pool: SqlitePool,
    clients_dir: PathBuf,
    templates_dir: PathBuf,
    peer_db_path: String,
    key_override: String,
    tpl_job: std::sync::Mutex<TplJob>,
    guard: std::sync::Mutex<AuthGuard>,
}

/// Progress state of the background official-client template download.
#[derive(Default)]
pub struct TplJob {
    running: bool,
    phase: String, // idle | downloading | done | error
    version: String,
    total: u64,
    source: String,
    error: String,
}

// ------------------------------ auth guard ------------------------------
// Brute-force protection for every password entry point (web console
// login, client password login, OIDC browser login, registration).
// Failures are counted per account and per source IP in a sliding
// window; hitting the limit locks that account (or IP) for a cooldown
// that doubles on repeat offences. State is in-memory on purpose: a
// restart clears it, and the [auth] audit log keeps the durable record.
// Thresholds can be tuned via RD_AUTH_* env vars (also used by tests).

fn auth_cfg(key: &str, dflt: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(dflt)
}

#[derive(Default)]
struct FailState {
    fails: Vec<i64>,
    locked_until: i64,
    level: u32,
    level_expires: i64,
}

/// A lock that just engaged (scope, key, level, lock seconds).
struct LockHit {
    scope: &'static str,
    key: String,
    level: u32,
    secs: i64,
}

#[derive(Default)]
pub struct AuthGuard {
    acct: HashMap<String, FailState>,
    ip: HashMap<String, FailState>,
    reg: HashMap<String, Vec<i64>>,
}

impl AuthGuard {
    fn prune_state(st: &mut FailState, t: i64, window: i64) {
        st.fails.retain(|f| t - *f < window);
        if st.level_expires <= t {
            st.level = 0;
        }
    }

    /// Seconds of lock remaining for this (account, ip) pair; 0 = allowed.
    fn locked_remaining(&mut self, user: &str, ip: &str, t: i64) -> i64 {
        let window = auth_cfg("RD_AUTH_WINDOW_SECS", 600);
        let mut remain = 0i64;
        if let Some(st) = self.acct.get_mut(user) {
            Self::prune_state(st, t, window);
            remain = remain.max(st.locked_until - t);
        }
        if let Some(st) = self.ip.get_mut(ip) {
            Self::prune_state(st, t, window);
            remain = remain.max(st.locked_until - t);
        }
        remain.max(0)
    }

    fn note_failure_in(
        map: &mut HashMap<String, FailState>,
        key: &str,
        scope: &'static str,
        max: i64,
        t: i64,
        hits: &mut Vec<LockHit>,
    ) {
        let window = auth_cfg("RD_AUTH_WINDOW_SECS", 600);
        let base = auth_cfg("RD_AUTH_LOCK_SECS", 900);
        let st = map.entry(key.to_string()).or_default();
        Self::prune_state(st, t, window);
        if st.locked_until > t {
            return; // already locked: do not extend or re-report
        }
        st.fails.push(t);
        if st.fails.len() as i64 >= max {
            st.level = st.level.saturating_add(1);
            let mult = 1i64 << (st.level - 1).min(6);
            let secs = base.saturating_mul(mult).min(43_200);
            st.locked_until = t + secs;
            st.level_expires = t + 86_400;
            st.fails.clear();
            hits.push(LockHit {
                scope,
                key: key.to_string(),
                level: st.level,
                secs,
            });
        }
    }

    /// Record one failed password attempt; returns any locks it engaged.
    fn record_failure(&mut self, user: &str, ip: &str, t: i64) -> Vec<LockHit> {
        let mut hits = Vec::new();
        if !user.is_empty() {
            Self::note_failure_in(
                &mut self.acct,
                user,
                "account",
                auth_cfg("RD_AUTH_FAIL_MAX", 5),
                t,
                &mut hits,
            );
        }
        if !ip.is_empty() {
            Self::note_failure_in(
                &mut self.ip,
                ip,
                "ip",
                auth_cfg("RD_AUTH_IP_FAIL_MAX", 30),
                t,
                &mut hits,
            );
        }
        hits
    }

    /// A successful login clears the account's failure counter. An
    /// active lock is never cleared here: locked attempts are rejected
    /// before the password check, so they cannot reach this point.
    fn record_success(&mut self, user: &str) {
        if let Some(st) = self.acct.get_mut(user) {
            st.fails.clear();
        }
    }

    /// Registration throttle: at most RD_AUTH_REG_MAX (default 10)
    /// attempts per IP per RD_AUTH_REG_WINDOW_SECS (default 1 h).
    fn reg_allowed(&mut self, ip: &str, t: i64) -> bool {
        let window = auth_cfg("RD_AUTH_REG_WINDOW_SECS", 3600);
        let max = auth_cfg("RD_AUTH_REG_MAX", 10);
        let list = self.reg.entry(ip.to_string()).or_default();
        list.retain(|f| t - *f < window);
        (list.len() as i64) < max
    }

    fn reg_record(&mut self, ip: &str, t: i64) {
        self.reg.entry(ip.to_string()).or_default().push(t);
    }
}

/// Keep attacker-controlled strings from breaking the log's structure.
fn log_safe(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if out.is_empty() {
        out.push('-');
    }
    out
}

/// One structured audit line per auth event; lands in logs/hbbs.log.
fn auth_log(event: &str, kind: &str, user: &str, ip: &str, detail: &str) {
    let line = format!(
        "[auth] {event} kind={kind} user={} ip={} {detail}",
        log_safe(user),
        log_safe(ip)
    );
    if event == "login-ok" || event == "register-ok" {
        log::info!("{line}");
    } else {
        log::warn!("{line}");
    }
}

fn guard_locked(st: &ApiState, user: &str, ip: &str) -> i64 {
    if let Ok(mut g) = st.guard.lock() {
        g.locked_remaining(user, ip, now())
    } else {
        0
    }
}

fn guard_fail(st: &ApiState, user: &str, ip: &str, kind: &str) {
    if let Ok(mut g) = st.guard.lock() {
        for hit in g.record_failure(user, ip, now()) {
            auth_log(
                "lockout",
                kind,
                user,
                ip,
                &format!(
                    "scope={} key={} level={} lock_secs={}",
                    hit.scope,
                    log_safe(&hit.key),
                    hit.level,
                    hit.secs
                ),
            );
        }
    }
}

fn guard_success(st: &ApiState, user: &str) {
    if let Ok(mut g) = st.guard.lock() {
        g.record_success(user);
    }
}

struct AuthUser {
    username: String,
    is_admin: bool,
}

const SYSINFO_VER: &str = "1";

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn rand_hex(nbytes: usize) -> String {
    sodiumoxide::randombytes::randombytes(nbytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn sha256_hex(data: &[u8]) -> String {
    sodiumoxide::crypto::hash::sha256::hash(data)
        .0
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn jresp(status: StatusCode, v: Value) -> Response {
    (status, axum::Json(v)).into_response()
}

fn err_resp(status: StatusCode, msg: &str) -> Response {
    jresp(status, json!({ "ok": false, "error": msg }))
}

fn client_err(msg: &str) -> Response {
    jresp(StatusCode::OK, json!({ "error": msg }))
}

// ------------------------------ database ------------------------------

async fn init_db(path: &str) -> Result<SqlitePool, String> {
    let mut opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    opts.log_statements(log::LevelFilter::Off);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .map_err(|e| e.to_string())?;
    let stmts = [
        "create table if not exists users(id integer primary key autoincrement, username text unique not null, password_hash text not null, is_admin integer not null default 0, disabled integer not null default 0, created_at integer not null)",
        "create table if not exists web_sessions(token text primary key, username text not null, expires_at integer not null)",
        "create table if not exists client_tokens(token text primary key, username text not null, device_id text not null default '', uuid text not null default '', created_at integer not null)",
        "create table if not exists oidc_codes(code text primary key, device_id text not null, uuid text not null, username text, created_at integer not null)",
        "create table if not exists api_tokens(id integer primary key autoincrement, username text not null, name text not null, token_hash text not null, created_at integer not null, last_used_at integer)",
        "create table if not exists devices(device_id text not null, uuid text not null, username text, hostname text not null default '', os text not null default '', os_username text not null default '', version text not null default '', last_seen integer not null, sysinfo text not null default '', primary key(device_id, uuid))",
        "create table if not exists settings(key text primary key, value text not null)",
        "create table if not exists gen_clients(id integer primary key autoincrement, filename text not null, token text not null, label text not null default '', created_by text not null, created_at integer not null)",
        "create table if not exists ab_store(username text primary key, data text not null)",
        "create table if not exists captcha_codes(id text primary key, code_hash text not null, expires_at integer not null)",
    ];
    for s in stmts {
        sqlx::query(s)
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(pool)
}

async fn get_setting(pool: &SqlitePool, key: &str) -> String {
    let row = sqlx::query("select value from settings where key=?")
        .bind(key)
        .fetch_optional(pool)
        .await;
    match row {
        Ok(Some(r)) => r.try_get::<String, _>("value").unwrap_or_default(),
        _ => String::new(),
    }
}

async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<(), String> {
    sqlx::query(
        "insert into settings(key, value) values(?, ?) on conflict(key) do update set value=excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn valid_username(u: &str) -> bool {
    (3..=32).contains(&u.len())
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

async fn create_user(pool: &SqlitePool, username: &str, password: &str) -> Result<(), String> {
    if !valid_username(username) {
        return Err("用户名需为 3-32 位字母、数字、_-.".into());
    }
    if password.len() < 6 {
        return Err("密码至少 6 位".into());
    }
    let count: i64 = sqlx::query("select count(*) as c from users")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?
        .try_get("c")
        .map_err(|e| e.to_string())?;
    let hash = bcrypt::hash(password, 10).map_err(|e| e.to_string())?;
    let r = sqlx::query(
        "insert into users(username, password_hash, is_admin, created_at) values(?, ?, ?, ?)",
    )
    .bind(username)
    .bind(&hash)
    .bind(if count == 0 { 1 } else { 0 })
    .bind(now())
    .execute(pool)
    .await;
    match r {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(db)) if db.message().contains("UNIQUE") => {
            Err("用户名已存在".into())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Ok(Some(user)) on good credentials, Ok(None) on bad credentials,
/// Err(message) when the account exists but may not log in.
async fn verify_user(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<Option<AuthUser>, String> {
    let row = sqlx::query(
        "select password_hash, is_admin, disabled from users where username=?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    let row = match row {
        Some(r) => r,
        None => return Ok(None),
    };
    let hash: String = row.try_get("password_hash").map_err(|e| e.to_string())?;
    let disabled: i64 = row.try_get("disabled").map_err(|e| e.to_string())?;
    if disabled != 0 {
        return Err("账号已被禁用".into());
    }
    let ok = bcrypt::verify(password, &hash).map_err(|e| e.to_string())?;
    if !ok {
        return Ok(None);
    }
    let is_admin: i64 = row.try_get("is_admin").map_err(|e| e.to_string())?;
    Ok(Some(AuthUser {
        username: username.to_string(),
        is_admin: is_admin != 0,
    }))
}

async fn load_user(pool: &SqlitePool, username: &str) -> Option<AuthUser> {
    let row = sqlx::query("select is_admin, disabled from users where username=?")
        .bind(username)
        .fetch_optional(pool)
        .await
        .ok()?;
    let row = row?;
    let disabled: i64 = row.try_get("disabled").ok()?;
    if disabled != 0 {
        return None;
    }
    let is_admin: i64 = row.try_get("is_admin").ok()?;
    Some(AuthUser {
        username: username.to_string(),
        is_admin: is_admin != 0,
    })
}

// ------------------------------ auth ------------------------------

fn cookie_val(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{name}=");
    for part in raw.split(';') {
        if let Some(v) = part.trim().strip_prefix(prefix.as_str()) {
            return Some(v.to_string());
        }
    }
    None
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ").map(|s| s.to_string())
}

async fn resolve_user(st: &ApiState, headers: &HeaderMap) -> Option<AuthUser> {
    if let Some(tok) = bearer(headers) {
        if tok.starts_with("rdt_") {
            let h = sha256_hex(tok.as_bytes());
            let row = sqlx::query("select id, username from api_tokens where token_hash=?")
                .bind(&h)
                .fetch_optional(&st.pool)
                .await
                .ok()?;
            let row = row?;
            let id: i64 = row.try_get("id").ok()?;
            let username: String = row.try_get("username").ok()?;
            let _ = sqlx::query("update api_tokens set last_used_at=? where id=?")
                .bind(now())
                .bind(id)
                .execute(&st.pool)
                .await;
            return load_user(&st.pool, &username).await;
        }
    }
    let sid = cookie_val(headers, "rdp_sid")?;
    let row = sqlx::query("select username, expires_at from web_sessions where token=?")
        .bind(&sid)
        .fetch_optional(&st.pool)
        .await
        .ok()?;
    let row = row?;
    let expires: i64 = row.try_get("expires_at").ok()?;
    if expires < now() {
        return None;
    }
    let username: String = row.try_get("username").ok()?;
    load_user(&st.pool, &username).await
}

async fn resolve_client_user(st: &ApiState, headers: &HeaderMap) -> Option<String> {
    let tok = bearer(headers)?;
    let row = sqlx::query("select username from client_tokens where token=?")
        .bind(&tok)
        .fetch_optional(&st.pool)
        .await
        .ok()?;
    let row = row?;
    row.try_get("username").ok()
}

async fn username_for_device(st: &ApiState, device_id: &str, uuid: &str) -> Option<String> {
    let row = sqlx::query(
        "select username from client_tokens where device_id=? and uuid=? order by created_at desc limit 1",
    )
    .bind(device_id)
    .bind(uuid)
    .fetch_optional(&st.pool)
    .await
    .ok()?;
    let row = row?;
    row.try_get("username").ok()
}

async fn new_web_session(st: &ApiState, username: &str) -> Result<String, String> {
    let token = rand_hex(32);
    sqlx::query("insert into web_sessions(token, username, expires_at) values(?, ?, ?)")
        .bind(&token)
        .bind(username)
        .bind(now() + 86400)
        .execute(&st.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(token)
}

fn session_response(token: &str, body: Value) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(v) = format!("rdp_sid={token}; Path=/; HttpOnly; Max-Age=86400; SameSite=Lax")
        .parse::<header::HeaderValue>()
    {
        headers.insert(header::SET_COOKIE, v);
    }
    (StatusCode::OK, headers, axum::Json(body)).into_response()
}

async fn issue_client_token(
    st: &ApiState,
    username: &str,
    device_id: &str,
    uuid: &str,
) -> Result<String, String> {
    let token = rand_hex(32);
    sqlx::query(
        "insert into client_tokens(token, username, device_id, uuid, created_at) values(?, ?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(username)
    .bind(device_id)
    .bind(uuid)
    .bind(now())
    .execute(&st.pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(token)
}

fn auth_body(username: &str, is_admin: bool, token: &str) -> Value {
    json!({
        "access_token": token,
        "type": "access_token",
        "user": {
            "name": username,
            "email": "",
            "note": "",
            "status": 1,
            "is_admin": is_admin,
            "info": {}
        }
    })
}

fn parse_body(body: &str) -> Value {
    serde_json::from_str::<Value>(body).unwrap_or(Value::Null)
}

fn sfield(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

// ------------------------------ client login ------------------------------

async fn login_options() -> Response {
    let common = serde_json::to_string(&json!([{ "name": "account" }])).unwrap_or_default();
    jresp(
        StatusCode::OK,
        json!([format!("common-oidc/{common}"), "oidc/account"]),
    )
}

#[derive(serde::Deserialize)]
struct OidcLoginForm {
    code: String,
    username: String,
    password: String,
    #[serde(default)]
    captcha: String,
}

async fn oidc_auth(Extension(st): Extension<Arc<ApiState>>, body: String) -> Response {
    let v = parse_body(&body);
    let code = rand_hex(16);
    let device_id = sfield(&v, "id");
    let uuid = sfield(&v, "uuid");
    if let Err(e) = sqlx::query(
        "insert into oidc_codes(code, device_id, uuid, created_at) values(?, ?, ?, ?)",
    )
    .bind(&code)
    .bind(&device_id)
    .bind(&uuid)
    .bind(now())
    .execute(&st.pool)
    .await
    {
        return client_err(&e.to_string());
    }
    // The URL the client opens in a browser. Prefer the apiDomain the client
    // itself used to reach us; it is guaranteed to be reachable by the user.
    let api_domain = sfield(&v, "apiDomain");
    let base = if api_domain.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", api_domain.trim_end_matches('/'))
    };
    jresp(
        StatusCode::OK,
        json!({ "code": code, "url": format!("{base}oidc/login?code={code}") }),
    )
}

async fn oidc_login_get(Query(q): Query<HashMap<String, String>>) -> Response {
    let code = q.get("code").cloned().unwrap_or_default();
    Html(api_pages::oidc_login_page(&code, "")).into_response()
}

async fn oidc_login_post(
    Extension(st): Extension<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<OidcLoginForm>,
) -> Response {
    let exists = sqlx::query("select code from oidc_codes where code=?")
        .bind(&f.code)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten()
        .is_some();
    if !exists {
        return Html(api_pages::oidc_login_page(&f.code, "登录链接已失效，请回客户端重试"))
            .into_response();
    }
    // This page is reachable from the client-facing listener (it is the
    // client's account-login path), so it gets the same captcha as the
    // console login — it previously had none (user report 2026-10-31).
    if let Err(e) = verify_captcha(&st, &headers, &f.captcha).await {
        return Html(api_pages::oidc_login_page(&f.code, &e)).into_response();
    }
    let ip = addr.ip().to_string();
    let remain = guard_locked(&st, &f.username, &ip);
    if remain > 0 {
        auth_log(
            "login-locked",
            "oidc",
            &f.username,
            &ip,
            &format!("remain_secs={remain}"),
        );
        return Html(api_pages::oidc_login_page(
            &f.code,
            &format!("登录尝试过于频繁，已临时锁定，请约 {} 分钟后再试", (remain + 59) / 60),
        ))
        .into_response();
    }
    match verify_user(&st.pool, &f.username, &f.password).await {
        Ok(Some(_)) => {
            guard_success(&st, &f.username);
            auth_log("login-ok", "oidc", &f.username, &ip, "");
            let _ = sqlx::query("update oidc_codes set username=? where code=?")
                .bind(&f.username)
                .bind(&f.code)
                .execute(&st.pool)
                .await;
            Html(api_pages::oidc_success_page()).into_response()
        }
        Ok(None) => {
            guard_fail(&st, &f.username, &ip, "oidc");
            auth_log(
                "login-fail",
                "oidc",
                &f.username,
                &ip,
                "reason=bad-credentials",
            );
            Html(api_pages::oidc_login_page(&f.code, "用户名或密码错误")).into_response()
        }
        Err(e) => {
            auth_log("login-fail", "oidc", &f.username, &ip, "reason=rejected");
            Html(api_pages::oidc_login_page(&f.code, &e)).into_response()
        }
    }
}

async fn auth_query(
    Extension(st): Extension<Arc<ApiState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let code = q.get("code").cloned().unwrap_or_default();
    let row = sqlx::query("select device_id, uuid, username from oidc_codes where code=?")
        .bind(&code)
        .fetch_optional(&st.pool)
        .await;
    let row = match row {
        Ok(Some(r)) => r,
        _ => return client_err("No authed oidc is found"),
    };
    let username: Option<String> = row.try_get("username").ok().flatten();
    let username = match username {
        Some(u) if !u.is_empty() => u,
        _ => return client_err("No authed oidc is found"),
    };
    let device_id: String = row.try_get("device_id").unwrap_or_default();
    let uuid: String = row.try_get("uuid").unwrap_or_default();
    let _ = sqlx::query("delete from oidc_codes where code=?")
        .bind(&code)
        .execute(&st.pool)
        .await;
    let user = match load_user(&st.pool, &username).await {
        Some(u) => u,
        None => return client_err("User is disabled"),
    };
    match issue_client_token(&st, &username, &device_id, &uuid).await {
        Ok(token) => jresp(
            StatusCode::OK,
            auth_body(&username, user.is_admin, &token),
        ),
        Err(e) => client_err(&e),
    }
}

async fn password_login(
    Extension(st): Extension<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: String,
) -> Response {
    let v = parse_body(&body);
    let username = sfield(&v, "username");
    let password = sfield(&v, "password");
    if username.is_empty() || password.is_empty() {
        return client_err("Username or password is empty");
    }
    let ip = addr.ip().to_string();
    let remain = guard_locked(&st, &username, &ip);
    if remain > 0 {
        auth_log(
            "login-locked",
            "client",
            &username,
            &ip,
            &format!("remain_secs={remain}"),
        );
        return client_err(&format!(
            "Too many failed attempts, temporarily locked. Try again in about {} min",
            (remain + 59) / 60
        ));
    }
    match verify_user(&st.pool, &username, &password).await {
        Ok(Some(user)) => {
            guard_success(&st, &username);
            auth_log("login-ok", "client", &username, &ip, "");
            let device_id = sfield(&v, "id");
            let uuid = sfield(&v, "uuid");
            match issue_client_token(&st, &username, &device_id, &uuid).await {
                Ok(token) => jresp(
                    StatusCode::OK,
                    auth_body(&username, user.is_admin, &token),
                ),
                Err(e) => client_err(&e),
            }
        }
        Ok(None) => {
            guard_fail(&st, &username, &ip, "client");
            auth_log(
                "login-fail",
                "client",
                &username,
                &ip,
                "reason=bad-credentials",
            );
            client_err("Username or password is incorrect")
        }
        Err(e) => {
            auth_log("login-fail", "client", &username, &ip, "reason=rejected");
            client_err(&e)
        }
    }
}

async fn client_logout(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    if let Some(tok) = bearer(&headers) {
        let _ = sqlx::query("delete from client_tokens where token=?")
            .bind(&tok)
            .execute(&st.pool)
            .await;
    }
    jresp(StatusCode::OK, json!({}))
}

// ------------------------------ device sync ------------------------------

async fn upsert_device(
    st: &ApiState,
    device_id: &str,
    uuid: &str,
    username: Option<String>,
    hostname: &str,
    os: &str,
    os_username: &str,
    version: &str,
    sysinfo: &str,
) {
    let _ = sqlx::query(
        "insert into devices(device_id, uuid, username, hostname, os, os_username, version, last_seen, sysinfo)
         values(?, ?, ?, ?, ?, ?, ?, ?, ?)
         on conflict(device_id, uuid) do update set
           username=coalesce(excluded.username, devices.username),
           hostname=case when excluded.hostname='' then devices.hostname else excluded.hostname end,
           os=case when excluded.os='' then devices.os else excluded.os end,
           os_username=case when excluded.os_username='' then devices.os_username else excluded.os_username end,
           version=case when excluded.version='' then devices.version else excluded.version end,
           sysinfo=case when excluded.sysinfo='' then devices.sysinfo else excluded.sysinfo end,
           last_seen=excluded.last_seen",
    )
    .bind(device_id)
    .bind(uuid)
    .bind(username)
    .bind(hostname)
    .bind(os)
    .bind(os_username)
    .bind(version)
    .bind(now())
    .bind(sysinfo)
    .execute(&st.pool)
    .await;
}

async fn bind_username(st: &ApiState, headers: &HeaderMap, device_id: &str, uuid: &str) -> Option<String> {
    if let Some(u) = resolve_client_user(st, headers).await {
        return Some(u);
    }
    username_for_device(st, device_id, uuid).await
}

async fn heartbeat(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let v = parse_body(&body);
    let device_id = sfield(&v, "id");
    let uuid = sfield(&v, "uuid");
    if !device_id.is_empty() {
        let username = bind_username(&st, &headers, &device_id, &uuid).await;
        let ver = v
            .get("ver")
            .map(|x| x.to_string())
            .unwrap_or_default();
        upsert_device(&st, &device_id, &uuid, username, "", "", "", &ver, "").await;
    }
    // Strategy push: keep the pushed client options in sync with the
    // console settings (hide-server-settings, allow-remote-config).
    let hide = get_setting(&st.pool, "hide_server_settings").await;
    let allow_rc = get_setting(&st.pool, "allow_remote_config").await;
    let ts: i64 = get_setting(&st.pool, "strategy_ts")
        .await
        .parse()
        .unwrap_or(1);
    let mut rsp = json!({ "modified_at": ts });
    rsp["strategy"] = json!({
        "config_options": {
            "hide-server-settings": if hide == "0" { "N" } else { "Y" },
            "allow-remote-config-modification": if allow_rc == "1" { "Y" } else { "N" },
        }
    });
    jresp(StatusCode::OK, rsp)
}

async fn sysinfo_ver() -> Response {
    (StatusCode::OK, SYSINFO_VER).into_response()
}

async fn sysinfo(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let v = parse_body(&body);
    let device_id = sfield(&v, "id");
    let uuid = sfield(&v, "uuid");
    if !device_id.is_empty() {
        let username = bind_username(&st, &headers, &device_id, &uuid).await;
        let os = match v.get("os") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        upsert_device(
            &st,
            &device_id,
            &uuid,
            username,
            &sfield(&v, "hostname"),
            &os,
            &sfield(&v, "username"),
            &sfield(&v, "version"),
            &body,
        )
        .await;
    }
    jresp(StatusCode::OK, json!({}))
}

// --------------------- client group / address book APIs ---------------------
// Endpoints the official client's device-group tab and (legacy) address
// book call with the login access token. They must always answer with a
// JSON body: an empty body makes the client throw a FormatException.

fn client_unauthorized() -> Response {
    jresp(
        StatusCode::UNAUTHORIZED,
        json!({ "error": "Please login" }),
    )
}

async fn client_caller(st: &ApiState, headers: &HeaderMap) -> Option<AuthUser> {
    let username = resolve_client_user(st, headers).await?;
    load_user(&st.pool, &username).await
}

fn page_params(q: &HashMap<String, String>) -> (usize, usize) {
    let cur: usize = q
        .get("current")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let size: usize = q
        .get("pageSize")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100)
        .clamp(1, 1000);
    (cur, size)
}

fn paged_json(items: Vec<Value>, cur: usize, size: usize) -> Value {
    let total = items.len();
    let data: Vec<Value> = items
        .into_iter()
        .skip((cur - 1) * size)
        .take(size)
        .collect();
    json!({ "total": total, "data": data })
}

async fn device_group_accessible(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if client_caller(&st, &headers).await.is_none() {
        return client_unauthorized();
    }
    let (cur, size) = page_params(&q);
    jresp(StatusCode::OK, paged_json(Vec::new(), cur, size))
}

async fn client_users(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let caller = match client_caller(&st, &headers).await {
        Some(u) => u,
        None => return client_unauthorized(),
    };
    let rows = sqlx::query(
        "select username, is_admin, disabled from users where disabled=0 order by created_at",
    )
    .fetch_all(&st.pool)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return client_err(&e.to_string()),
    };
    // v9: same scoping as client_peers — the client sidebar lists only the
    // login account, otherwise it would offer users whose devices can
    // never appear in the scoped peer list.
    let mut items: Vec<Value> = Vec::new();
    for r in &rows {
        let name = r.try_get::<String, _>("username").unwrap_or_default();
        if name != caller.username {
            continue;
        }
        items.push(json!({
            "name": name,
            "display_name": "",
            "avatar": "",
            "email": "",
            "note": "",
            "status": 1,
            "is_admin": r.try_get::<i64, _>("is_admin").unwrap_or(0) != 0,
        }));
    }
    let (cur, size) = page_params(&q);
    jresp(StatusCode::OK, paged_json(items, cur, size))
}

// The official client's home page refreshes the logged-in user on every
// start via POST /api/currentUser and decodes the body with an unguarded
// json.decode (only 401/400 are checked first). A missing route answers
// 404 with an empty body, which the client surfaces as the bare
// "FormatException: Unexpected end of input" under its retry widget, so
// this endpoint must exist and always return a JSON body.
async fn current_user(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
) -> Response {
    let caller = match client_caller(&st, &headers).await {
        Some(u) => u,
        None => return client_unauthorized(),
    };
    jresp(
        StatusCode::OK,
        json!({
            "name": caller.username,
            "display_name": "",
            "avatar": "",
            "email": "",
            "note": "",
            "status": 1,
            "is_admin": caller.is_admin,
        }),
    )
}

async fn client_peers(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let caller = match client_caller(&st, &headers).await {
        Some(u) => u,
        None => return client_unauthorized(),
    };
    let rows = sqlx::query(
        "select device_id, username, hostname, os, os_username from devices order by last_seen desc",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    // v9: the client device list is scoped to the login account for every
    // caller, including admins. Other users' devices and unbound devices
    // must not appear here; the web console's /api/devices remains the
    // admin's full view.
    let mut items: Vec<Value> = Vec::new();
    for r in &rows {
        let id = r.try_get::<String, _>("device_id").unwrap_or_default();
        let owner = r
            .try_get::<Option<String>, _>("username")
            .ok()
            .flatten()
            .unwrap_or_default();
        if owner != caller.username {
            continue;
        }
        items.push(json!({
            "id": id,
            "user": owner,
            "user_name": owner,
            "note": "",
            "device_group_name": "",
            "status": 1,
            "info": {
                "device_name": r.try_get::<String, _>("hostname").unwrap_or_default(),
                "os": r.try_get::<String, _>("os").unwrap_or_default(),
                "username": r.try_get::<String, _>("os_username").unwrap_or_default(),
            },
        }));
    }
    let (cur, size) = page_params(&q);
    jresp(StatusCode::OK, paged_json(items, cur, size))
}

async fn ab_get(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let caller = match client_caller(&st, &headers).await {
        Some(u) => u,
        None => return client_unauthorized(),
    };
    let row = sqlx::query("select data from ab_store where username=?")
        .bind(&caller.username)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    match row {
        Some(r) => {
            let data: String = r.try_get("data").unwrap_or_default();
            jresp(
                StatusCode::OK,
                json!({ "data": data, "licensed_devices": 0 }),
            )
        }
        // The client treats the literal body "null" as an empty address book.
        None => (StatusCode::OK, "null").into_response(),
    }
}

async fn ab_post(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let caller = match client_caller(&st, &headers).await {
        Some(u) => u,
        None => return client_unauthorized(),
    };
    let v = parse_body(&body);
    let data = sfield(&v, "data");
    let r = sqlx::query(
        "insert into ab_store(username, data) values(?, ?) on conflict(username) do update set data=excluded.data",
    )
    .bind(&caller.username)
    .bind(&data)
    .execute(&st.pool)
    .await;
    match r {
        Ok(_) => (StatusCode::OK, "").into_response(),
        Err(e) => client_err(&e.to_string()),
    }
}

// ------------------------------ captcha ------------------------------
// Image captcha for the web console login/register pages. The code lives
// only server-side (sha256, 5-minute expiry, one-time use); the browser
// only carries the challenge id in a cookie, so reading page source or
// cookies never reveals the answer. This is a basic bot check against
// automated credential stuffing, not a hardened CAPTCHA: it will not stop
// a dedicated OCR solver. The official client's own login (/api/login)
// is deliberately NOT covered: the client has no captcha UI, and adding
// one there would break client logins.

const CAPTCHA_LEN: usize = 5;
const CAPTCHA_TTL: i64 = 300;
const CAPTCHA_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

fn captcha_rand_byte() -> u8 {
    sodiumoxide::randombytes::randombytes(1)
        .first()
        .copied()
        .unwrap_or(0)
}

fn captcha_code() -> String {
    let mut s = String::with_capacity(CAPTCHA_LEN);
    for _ in 0..CAPTCHA_LEN {
        let idx = captcha_rand_byte() as usize % CAPTCHA_CHARS.len();
        s.push(CAPTCHA_CHARS[idx] as char);
    }
    s
}

fn captcha_svg(code: &str) -> String {
    let mut s = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"140\" height=\"48\" viewBox=\"0 0 140 48\"><rect width=\"140\" height=\"48\" fill=\"#e8ecf1\"/>");
    let grays = ["#9aa4b2", "#b0b8c5", "#8b95a5"];
    for _ in 0..4 {
        let x1 = captcha_rand_byte() as u16 % 140;
        let y1 = captcha_rand_byte() as u16 % 48;
        let x2 = captcha_rand_byte() as u16 % 140;
        let y2 = captcha_rand_byte() as u16 % 48;
        let g = grays[captcha_rand_byte() as usize % grays.len()];
        s.push_str(&format!(
            "<line x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" stroke=\"{g}\" stroke-width=\"1\"/>"
        ));
    }
    for _ in 0..20 {
        let cx = captcha_rand_byte() as u16 % 140;
        let cy = captcha_rand_byte() as u16 % 48;
        let g = grays[captcha_rand_byte() as usize % grays.len()];
        s.push_str(&format!("<circle cx=\"{cx}\" cy=\"{cy}\" r=\"1\" fill=\"{g}\"/>"));
    }
    let colors = ["#1f2937", "#7c3aed", "#0369a1", "#b45309", "#0f766e"];
    for (i, ch) in code.chars().enumerate() {
        let x = 14 + (i as u16) * 26;
        let y = 31 + (captcha_rand_byte() as i16 % 11) - 5;
        let rot = (captcha_rand_byte() as i16 % 31) - 15;
        let c = colors[captcha_rand_byte() as usize % colors.len()];
        s.push_str(&format!("<text x=\"{x}\" y=\"{y}\" font-family=\"Arial,sans-serif\" font-size=\"26\" font-weight=\"bold\" fill=\"{c}\" transform=\"rotate({rot} {x} {y})\">{ch}</text>"));
    }
    s.push_str("</svg>");
    s
}

async fn captcha_begin(st: &ApiState, answer: &str) -> Result<String, Response> {
    let _ = sqlx::query("delete from captcha_codes where expires_at<=?")
        .bind(now())
        .execute(&st.pool)
        .await;
    let id = rand_hex(16);
    let hash = sha256_hex(answer.to_uppercase().as_bytes());
    let ins = sqlx::query(
        "insert into captcha_codes(id, code_hash, expires_at) values(?, ?, ?)",
    )
    .bind(&id)
    .bind(&hash)
    .bind(now() + CAPTCHA_TTL)
    .execute(&st.pool)
    .await;
    if ins.is_err() {
        return Err(err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "验证码生成失败",
        ));
    }
    Ok(id)
}

fn captcha_headers(id: &str, content_type: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(v) = format!("rdp_captcha={id}; Path=/; HttpOnly; Max-Age={CAPTCHA_TTL}; SameSite=Lax")
        .parse::<header::HeaderValue>()
    {
        headers.insert(header::SET_COOKIE, v);
    }
    if let Ok(v) = content_type.parse::<header::HeaderValue>() {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers
}

fn captcha_question() -> (String, String) {
    let a = 2 + (captcha_rand_byte() % 8) as i64;
    let b = 2 + (captcha_rand_byte() % 8) as i64;
    match captcha_rand_byte() % 3 {
        0 => (format!("{a} 加 {b} 等于多少？"), (a + b).to_string()),
        1 => {
            let (x, y) = if a >= b { (a, b) } else { (b, a) };
            (format!("{x} 减 {y} 等于多少？"), (x - y).to_string())
        }
        _ => (format!("{a} 乘 {b} 等于多少？"), (a * b).to_string()),
    }
}

async fn captcha_image(
    Extension(st): Extension<Arc<ApiState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if q.get("mode").map(|s| s.as_str()) == Some("text") {
        // Accessible alternative to the image challenge: a plain-text
        // arithmetic question, stored and verified through the exact same
        // one-time code path (verify_captcha). For users who cannot read
        // the image: screen readers, or the console viewed through a
        // heavily compressed remote-desktop session.
        let (question, answer) = captcha_question();
        let id = match captcha_begin(&st, &answer).await {
            Ok(id) => id,
            Err(r) => return r,
        };
        let headers = captcha_headers(&id, "application/json");
        return (
            StatusCode::OK,
            headers,
            json!({ "ok": true, "question": question }).to_string(),
        )
            .into_response();
    }
    let code = captcha_code();
    let id = match captcha_begin(&st, &code).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    let headers = captcha_headers(&id, "image/svg+xml");
    (StatusCode::OK, headers, captcha_svg(&code)).into_response()
}

async fn verify_captcha(
    st: &ApiState,
    headers: &HeaderMap,
    code: &str,
) -> Result<(), String> {
    let id = cookie_val(headers, "rdp_captcha")
        .ok_or_else(|| "请先获取验证码".to_string())?;
    let row = sqlx::query("select code_hash, expires_at from captcha_codes where id=?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    // One-time use: consume the challenge whatever the outcome, so a
    // solved code cannot be replayed and a failed attempt forces refresh.
    let _ = sqlx::query("delete from captcha_codes where id=?")
        .bind(&id)
        .execute(&st.pool)
        .await;
    let row = match row {
        Some(r) => r,
        None => return Err("验证码已过期，请点击图片刷新".to_string()),
    };
    let exp: i64 = row.try_get("expires_at").unwrap_or(0);
    if exp <= now() {
        return Err("验证码已过期，请点击图片刷新".to_string());
    }
    let want: String = row.try_get("code_hash").unwrap_or_default();
    let code = code.trim();
    if code.is_empty() || sha256_hex(code.to_uppercase().as_bytes()) != want {
        return Err("验证码错误".to_string());
    }
    Ok(())
}

// ------------------------------ web auth ------------------------------

#[derive(serde::Deserialize)]
struct CredForm {
    username: String,
    password: String,
}

async fn web_register(
    Extension(st): Extension<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let v = parse_body(&body);
    if let Err(e) = verify_captcha(&st, &headers, &sfield(&v, "captcha")).await {
        return err_resp(StatusCode::BAD_REQUEST, &e);
    }
    let f = CredForm {
        username: sfield(&v, "username"),
        password: sfield(&v, "password"),
    };
    let ip = addr.ip().to_string();
    // Per-IP registration throttle (account-creation spam).
    let reg_ok = if let Ok(mut g) = st.guard.lock() {
        let allowed = g.reg_allowed(&ip, now());
        if allowed {
            g.reg_record(&ip, now());
        }
        allowed
    } else {
        true
    };
    if !reg_ok {
        auth_log("register-blocked", "web", &f.username, &ip, "reason=rate-limit");
        return err_resp(StatusCode::TOO_MANY_REQUESTS, "注册过于频繁，请稍后再试");
    }
    // Open registration can be switched off in the server settings. It
    // never applies to the very first account (bootstrap), and an
    // admin who is logged in can still create accounts while it is off.
    let user_count: i64 = sqlx::query("select count(*) as c from users")
        .fetch_one(&st.pool)
        .await
        .ok()
        .and_then(|r| r.try_get("c").ok())
        .unwrap_or(0);
    if user_count > 0 && get_setting(&st.pool, "allow_register").await == "0" {
        let caller_admin = resolve_user(&st, &headers)
            .await
            .map(|u| u.is_admin)
            .unwrap_or(false);
        if !caller_admin {
            auth_log("register-blocked", "web", &f.username, &ip, "reason=closed");
            return err_resp(StatusCode::FORBIDDEN, "注册已关闭，请联系管理员开通账号");
        }
    }
    if let Err(e) = create_user(&st.pool, &f.username, &f.password).await {
        return err_resp(StatusCode::BAD_REQUEST, &e);
    }
    auth_log("register-ok", "web", &f.username, &ip, "");
    match new_web_session(&st, &f.username).await {
        Ok(token) => session_response(&token, json!({ "ok": true })),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn web_login(
    Extension(st): Extension<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let v = parse_body(&body);
    if let Err(e) = verify_captcha(&st, &headers, &sfield(&v, "captcha")).await {
        return err_resp(StatusCode::BAD_REQUEST, &e);
    }
    let username = sfield(&v, "username");
    let password = sfield(&v, "password");
    let ip = addr.ip().to_string();
    let remain = guard_locked(&st, &username, &ip);
    if remain > 0 {
        auth_log(
            "login-locked",
            "web",
            &username,
            &ip,
            &format!("remain_secs={remain}"),
        );
        return err_resp(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("登录尝试过于频繁，已临时锁定，请约 {} 分钟后再试", (remain + 59) / 60),
        );
    }
    match verify_user(&st.pool, &username, &password).await {
        Ok(Some(_)) => {
            guard_success(&st, &username);
            auth_log("login-ok", "web", &username, &ip, "");
            match new_web_session(&st, &username).await {
                Ok(token) => session_response(&token, json!({ "ok": true })),
                Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e),
            }
        }
        Ok(None) => {
            guard_fail(&st, &username, &ip, "web");
            auth_log("login-fail", "web", &username, &ip, "reason=bad-credentials");
            err_resp(StatusCode::UNAUTHORIZED, "用户名或密码错误")
        }
        Err(e) => {
            auth_log("login-fail", "web", &username, &ip, "reason=rejected");
            err_resp(StatusCode::FORBIDDEN, &e)
        }
    }
}

async fn web_logout(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    if let Some(sid) = cookie_val(&headers, "rdp_sid") {
        let _ = sqlx::query("delete from web_sessions where token=?")
            .bind(&sid)
            .execute(&st.pool)
            .await;
    }
    let mut headers = HeaderMap::new();
    if let Ok(v) = "rdp_sid=; Path=/; HttpOnly; Max-Age=0".parse::<header::HeaderValue>() {
        headers.insert(header::SET_COOKIE, v);
    }
    (
        StatusCode::OK,
        headers,
        axum::Json(json!({ "ok": true })),
    )
        .into_response()
}

async fn me(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    match resolve_user(&st, &headers).await {
        Some(u) => jresp(
            StatusCode::OK,
            json!({ "ok": true, "username": u.username, "is_admin": u.is_admin }),
        ),
        None => err_resp(StatusCode::UNAUTHORIZED, "未登录"),
    }
}

// ------------------------------ console APIs ------------------------------

fn unauthorized() -> Response {
    err_resp(StatusCode::UNAUTHORIZED, "未登录")
}

fn forbidden() -> Response {
    err_resp(StatusCode::FORBIDDEN, "需要管理员权限")
}

async fn peer_count(path: &str) -> i64 {
    if !std::path::Path::new(path).exists() {
        return 0;
    }
    let mut opts = SqliteConnectOptions::new().filename(path).read_only(true);
    opts.log_statements(log::LevelFilter::Off);
    let pool = match SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
    {
        Ok(p) => p,
        Err(_) => return 0,
    };
    let n = sqlx::query("select count(*) as c from peer")
        .fetch_one(&pool)
        .await
        .ok()
        .and_then(|r| r.try_get::<i64, _>("c").ok())
        .unwrap_or(0);
    pool.close().await;
    n
}

async fn overview(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    if !u.is_admin {
        return forbidden();
    }
    let users: i64 = sqlx::query("select count(*) as c from users")
        .fetch_one(&st.pool)
        .await
        .ok()
        .and_then(|r| r.try_get("c").ok())
        .unwrap_or(0);
    let devices: i64 = sqlx::query("select count(*) as c from devices")
        .fetch_one(&st.pool)
        .await
        .ok()
        .and_then(|r| r.try_get("c").ok())
        .unwrap_or(0);
    let peers = peer_count(&st.peer_db_path).await;
    jresp(
        StatusCode::OK,
        json!({ "ok": true, "users": users, "devices": devices, "peers": peers }),
    )
}

async fn admin_users(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    if !u.is_admin {
        return forbidden();
    }
    let rows = sqlx::query(
        "select username, is_admin, disabled, created_at from users order by created_at",
    )
    .fetch_all(&st.pool)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let users: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "username": r.try_get::<String, _>("username").unwrap_or_default(),
                "is_admin": r.try_get::<i64, _>("is_admin").unwrap_or(0) != 0,
                "disabled": r.try_get::<i64, _>("disabled").unwrap_or(0) != 0,
                "created_at": r.try_get::<i64, _>("created_at").unwrap_or(0),
            })
        })
        .collect();
    jresp(StatusCode::OK, json!({ "ok": true, "users": users }))
}

async fn admin_user_action(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path((name, action)): Path<(String, String)>,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    if !u.is_admin {
        return forbidden();
    }
    if name == u.username && matches!(action.as_str(), "delete" | "disable" | "unadmin") {
        return err_resp(StatusCode::BAD_REQUEST, "不能对自己执行此操作");
    }
    let r = match action.as_str() {
        "disable" => {
            sqlx::query("update users set disabled=1 where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await
        }
        "enable" => {
            sqlx::query("update users set disabled=0 where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await
        }
        "admin" => {
            sqlx::query("update users set is_admin=1 where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await
        }
        "unadmin" => {
            sqlx::query("update users set is_admin=0 where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await
        }
        "delete" => {
            let _ = sqlx::query("delete from web_sessions where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await;
            let _ = sqlx::query("delete from client_tokens where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await;
            let _ = sqlx::query("delete from api_tokens where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await;
            let _ = sqlx::query("update devices set username=null where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await;
            sqlx::query("delete from users where username=?")
                .bind(&name)
                .execute(&st.pool)
                .await
        }
        _ => return err_resp(StatusCode::BAD_REQUEST, "未知操作"),
    };
    match r {
        Ok(res) if res.rows_affected() > 0 => jresp(StatusCode::OK, json!({ "ok": true })),
        Ok(_) => err_resp(StatusCode::NOT_FOUND, "用户不存在"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn admin_user_password(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: String,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    if !u.is_admin {
        return forbidden();
    }
    let v = parse_body(&body);
    let password = sfield(&v, "password");
    if password.len() < 6 {
        return err_resp(StatusCode::BAD_REQUEST, "密码至少 6 位");
    }
    let hash = match bcrypt::hash(&password, 10) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    match sqlx::query("update users set password_hash=? where username=?")
        .bind(&hash)
        .bind(&name)
        .execute(&st.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => jresp(StatusCode::OK, json!({ "ok": true })),
        Ok(_) => err_resp(StatusCode::NOT_FOUND, "用户不存在"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn devices_list(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let rows = if u.is_admin {
        sqlx::query(
            "select device_id, uuid, username, hostname, os, version, last_seen from devices order by last_seen desc",
        )
        .fetch_all(&st.pool)
        .await
    } else {
        sqlx::query(
            "select device_id, uuid, username, hostname, os, version, last_seen from devices where username=? order by last_seen desc",
        )
        .bind(&u.username)
        .fetch_all(&st.pool)
        .await
    };
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let devices: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "device_id": r.try_get::<String, _>("device_id").unwrap_or_default(),
                "uuid": r.try_get::<String, _>("uuid").unwrap_or_default(),
                "username": r.try_get::<Option<String>, _>("username").ok().flatten(),
                "hostname": r.try_get::<String, _>("hostname").unwrap_or_default(),
                "os": r.try_get::<String, _>("os").unwrap_or_default(),
                "version": r.try_get::<String, _>("version").unwrap_or_default(),
                "last_seen": r.try_get::<i64, _>("last_seen").unwrap_or(0),
            })
        })
        .collect();
    jresp(StatusCode::OK, json!({ "ok": true, "now": now(), "devices": devices }))
}

async fn tokens_list(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let rows = sqlx::query(
        "select id, name, created_at, last_used_at from api_tokens where username=? order by id desc",
    )
    .bind(&u.username)
    .fetch_all(&st.pool)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let tokens: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "name": r.try_get::<String, _>("name").unwrap_or_default(),
                "created_at": r.try_get::<i64, _>("created_at").unwrap_or(0),
                "last_used_at": r.try_get::<Option<i64>, _>("last_used_at").ok().flatten(),
            })
        })
        .collect();
    jresp(StatusCode::OK, json!({ "ok": true, "tokens": tokens }))
}

async fn tokens_create(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let v = parse_body(&body);
    let name = sfield(&v, "name");
    let name = if name.is_empty() { "default" } else { &name };
    let token = format!("rdt_{}", rand_hex(32));
    let hash = sha256_hex(token.as_bytes());
    match sqlx::query(
        "insert into api_tokens(username, name, token_hash, created_at) values(?, ?, ?, ?)",
    )
    .bind(&u.username)
    .bind(name)
    .bind(&hash)
    .bind(now())
    .execute(&st.pool)
    .await
    {
        Ok(_) => jresp(StatusCode::OK, json!({ "ok": true, "token": token })),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn tokens_delete(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    match sqlx::query("delete from api_tokens where id=? and username=?")
        .bind(id)
        .bind(&u.username)
        .execute(&st.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => jresp(StatusCode::OK, json!({ "ok": true })),
        Ok(_) => err_resp(StatusCode::NOT_FOUND, "Token 不存在"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn settings_get(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    if !u.is_admin {
        return forbidden();
    }
    let tpl_ready = st.templates_dir.join("rustdesk-windows.exe").exists();
    jresp(
        StatusCode::OK,
        json!({
            "ok": true,
            "settings": {
                "public_host": get_setting(&st.pool, "public_host").await,
                "api_url_override": get_setting(&st.pool, "api_url_override").await,
                "github_mirror": get_setting(&st.pool, "github_mirror").await,
                "hide_server_settings": get_setting(&st.pool, "hide_server_settings").await != "0",
                "allow_register": get_setting(&st.pool, "allow_register").await != "0",
                "allow_remote_config": get_setting(&st.pool, "allow_remote_config").await == "1",
            },
            "template_ready": tpl_ready,
            "template_version": get_setting(&st.pool, "tpl_version").await,
            "ports": effective_ports(),
        }),
    )
}

async fn settings_post(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    if !u.is_admin {
        return forbidden();
    }
    let v = parse_body(&body);
    let host = sfield(&v, "public_host");
    let host = host
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string();
    if let Err(e) = set_setting(&st.pool, "public_host", &host).await {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    if let Err(e) = set_setting(&st.pool, "api_url_override", &sfield(&v, "api_url_override")).await {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    if let Err(e) = set_setting(&st.pool, "github_mirror", &sfield(&v, "github_mirror")).await {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    let hide = v
        .get("hide_server_settings")
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    let old_hide = get_setting(&st.pool, "hide_server_settings").await;
    let new_hide = if hide { "1" } else { "0" };
    if old_hide != new_hide && !(old_hide.is_empty() && hide) {
        // bump the strategy timestamp so clients pick up the change
        let _ = set_setting(&st.pool, "strategy_ts", &now().to_string()).await;
    }
    if let Err(e) = set_setting(&st.pool, "hide_server_settings", new_hide).await {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    let allow_reg = v
        .get("allow_register")
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    if let Err(e) = set_setting(
        &st.pool,
        "allow_register",
        if allow_reg { "1" } else { "0" },
    )
    .await
    {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    let allow_rc = v
        .get("allow_remote_config")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let old_rc = get_setting(&st.pool, "allow_remote_config").await;
    let new_rc = if allow_rc { "1" } else { "0" };
    if old_rc != new_rc && !(old_rc.is_empty() && !allow_rc) {
        // bump the strategy timestamp so clients pick up the change
        let _ = set_setting(&st.pool, "strategy_ts", &now().to_string()).await;
    }
    if let Err(e) = set_setting(&st.pool, "allow_remote_config", new_rc).await {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    jresp(StatusCode::OK, json!({ "ok": true }))
}

// ------------------------------ client generation ------------------------------

// ------------------------- built-in HTTP downloader -------------------------
// The template download pipeline is self-contained: it uses the reqwest
// copy already linked into hbbs instead of shelling out to a system
// curl, which is not guaranteed to exist — on Windows without curl.exe
// in PATH every download source used to fail with "program not found".

/// Client for the download pipeline: fast failure on dead mirrors
/// (10 s connect timeout), `hbbs-api` user agent, redirects followed,
/// proxy taken from the environment.
fn http_client(total_timeout: Option<std::time::Duration>) -> Result<reqwest::Client, String> {
    let mut b = reqwest::Client::builder()
        .user_agent("hbbs-api")
        .connect_timeout(std::time::Duration::from_secs(10));
    if let Some(t) = total_timeout {
        b = b.timeout(t);
    }
    b.build().map_err(|e| format!("初始化下载组件失败：{e}"))
}

/// Small GET (release-metadata JSON) through the built-in downloader.
fn fetch_bytes(url: &str, timeout_secs: u64) -> Result<Vec<u8>, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("初始化下载组件失败：{e}"))?;
    rt.block_on(async {
        let client = http_client(Some(std::time::Duration::from_secs(timeout_secs)))?;
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("下载失败：{e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("下载失败：HTTP {status}"));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("下载失败：{e}"))
    })
}

/// Built-in GitHub acceleration mirrors (prefix style: mirror + full URL).
/// They are tried in order before a direct connection; a slow or dead
/// mirror is abandoned automatically via the downloader's stall detection.
const GH_MIRRORS: [&str; 5] = [
    "https://gh-proxy.com/",
    "https://ghproxy.net/",
    "https://gh-proxy.ygxz.in/",
    "https://ghfast.top/",
    "https://ghps.cc/",
];

fn tpl_part_path(st: &ApiState) -> PathBuf {
    st.templates_dir.join("rustdesk-windows.exe.part")
}

fn tpl_final_path(st: &ApiState) -> PathBuf {
    st.templates_dir.join("rustdesk-windows.exe")
}

/// Resolve the latest official client version and the installer size from
/// the GitHub API (small JSON). Mirrors are tried before a direct
/// connection, because direct GitHub API access is the part most likely to
/// be slow or blocked on networks that need acceleration; direct is the
/// last resort.
fn resolve_latest(custom: &str) -> Result<(String, u64), String> {
    let api_url = "https://api.github.com/repos/rustdesk/rustdesk/releases/latest";
    let mut urls: Vec<String> = Vec::new();
    if !custom.is_empty() {
        urls.push(format!("{custom}/{api_url}"));
    }
    for m in GH_MIRRORS {
        urls.push(format!("{m}{api_url}"));
    }
    urls.push(api_url.to_string());
    let mut last_err = String::new();
    for u in urls {
        match fetch_bytes(&u, 20) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                let version = v
                    .get("tag_name")
                    .and_then(|t| t.as_str())
                    .map(|t| t.trim_start_matches('v').to_string())
                    .ok_or_else(|| "无法解析官方最新版本".to_string())?;
                let asset = format!("rustdesk-{version}-x86_64.exe");
                let mut size = 0u64;
                if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
                    for a in assets {
                        if a.get("name").and_then(|n| n.as_str()) == Some(asset.as_str()) {
                            size = a.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                        }
                    }
                }
                return Ok((version, size));
            }
            Err(e) => last_err = e,
        }
    }
    Err(format!(
        "无法获取官方版本信息（GitHub API 不可达）：{last_err}。可在上方手动填写版本号后重试"
    ))
}

/// Stream `url` to `dest` with the built-in downloader, aborting when
/// the transfer stalls (no data for 20 s, or below ~10 KB/s over a 15 s
/// window) or exceeds `max_time` overall, so the caller can fail over
/// to the next mirror. The body is written incrementally, so the status
/// endpoint keeps reporting progress from the partial file's size.
fn download_to_file(url: &str, dest: &std::path::Path, max_time: u64) -> Result<(), String> {
    use std::io::Write;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("初始化下载组件失败：{e}"))?;
    rt.block_on(async {
        let client = http_client(None)?;
        let mut resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("下载失败：{e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("下载失败：HTTP {status}"));
        }
        let mut file =
            std::fs::File::create(dest).map_err(|e| format!("无法创建下载文件：{e}"))?;
        let started = std::time::Instant::now();
        // (timestamp, total bytes at that moment) samples bracketing the
        // stall-detection window.
        let mut samples: std::collections::VecDeque<(std::time::Instant, u64)> =
            std::collections::VecDeque::new();
        samples.push_back((started, 0));
        let mut total: u64 = 0;
        loop {
            let chunk = match tokio::time::timeout(
                std::time::Duration::from_secs(20),
                resp.chunk(),
            )
            .await
            {
                Ok(c) => c.map_err(|e| format!("下载失败：{e}"))?,
                Err(_) => {
                    return Err("下载停滞（20 秒没有收到数据），已中止".to_string());
                }
            };
            let bytes = match chunk {
                Some(b) => b,
                None => break,
            };
            file.write_all(&bytes)
                .map_err(|e| format!("写入下载文件失败：{e}"))?;
            total += bytes.len() as u64;
            let now = std::time::Instant::now();
            if now.duration_since(started).as_secs() > max_time {
                return Err(format!("下载超时（超过 {max_time} 秒）"));
            }
            samples.push_back((now, total));
            // Keep the oldest sample that still brackets the 15 s window.
            while samples.len() > 2 {
                match samples.get(1) {
                    Some((t, _)) if now.duration_since(*t).as_secs() >= 15 => {
                        samples.pop_front();
                    }
                    _ => break,
                }
            }
            if let Some((t0, b0)) = samples.front() {
                let span = now.duration_since(*t0).as_secs_f64();
                if span >= 15.0 && (total - b0) as f64 / span < 10240.0 {
                    return Err("下载过慢（持续 15 秒低于 10 KB/s），已中止".to_string());
                }
            }
        }
        Ok(())
    })
}

fn file_starts_with_mz(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 2];
    match std::fs::File::open(path) {
        Ok(mut f) => f.read_exact(&mut buf).is_ok() && &buf == b"MZ",
        Err(_) => false,
    }
}

fn set_job(st: &ApiState, f: impl FnOnce(&mut TplJob)) {
    if let Ok(mut j) = st.tpl_job.lock() {
        f(&mut j);
    }
}

/// Background template download with GitHub mirror failover. Runs on a
/// plain thread; progress is read from the .part file size by the status
/// endpoint, and the winning source is recorded in the job state.
fn run_template_job(st: Arc<ApiState>, version_req: String) {
    let dest = tpl_final_path(&st);
    let part = tpl_part_path(&st);
    let finish_err = |st: &ApiState, msg: String| {
        set_job(st, |j| {
            j.running = false;
            j.phase = "error".into();
            j.error = msg;
        });
    };
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            finish_err(&st, e.to_string());
            return;
        }
    }
    let custom_mirror = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match rt {
            Ok(rt) => rt.block_on(get_setting(&st.pool, "github_mirror")),
            Err(_) => String::new(),
        }
    };
    let custom: String = custom_mirror.trim().trim_end_matches('/').to_string();
    let (version, total) = if !version_req.trim().is_empty() {
        (
            version_req.trim().trim_start_matches('v').to_string(),
            0u64,
        )
    } else {
        match resolve_latest(&custom) {
            Ok(vs) => vs,
            Err(e) => {
                finish_err(&st, e);
                return;
            }
        }
    };
    set_job(&st, |j| {
        j.version = version.clone();
        j.total = total;
    });
    let direct =
        format!("https://github.com/rustdesk/rustdesk/releases/download/{version}/rustdesk-{version}-x86_64.exe");
    let mut candidates: Vec<(String, String)> = Vec::new();
    if !custom.is_empty() {
        candidates.push((
            format!("自定义镜像 {custom}"),
            format!("{custom}/{direct}"),
        ));
    }
    for m in GH_MIRRORS {
        candidates.push((m.to_string(), format!("{m}{direct}")));
    }
    candidates.push(("直连 GitHub".to_string(), direct));
    let mut last_err = String::new();
    for (label, url) in candidates {
        set_job(&st, |j| j.source = label.clone());
        let _ = std::fs::remove_file(&part);
        match download_to_file(&url, &part, 1800) {
            Ok(()) => {
                let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                let size_ok = len >= 1_000_000 && (total == 0 || len == total);
                if size_ok && file_starts_with_mz(&part) {
                    match std::fs::rename(&part, &dest) {
                        Ok(()) => {
                            let rt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build();
                            if let Ok(rt) = rt {
                                rt.block_on(set_setting(&st.pool, "tpl_version", &version));
                            }
                            set_job(&st, |j| {
                                j.running = false;
                                j.phase = "done".into();
                                j.error = String::new();
                            });
                            return;
                        }
                        Err(e) => last_err = e.to_string(),
                    }
                } else {
                    last_err = format!("{label} 返回的文件不完整或不是有效的安装程序（{len} 字节）");
                }
            }
            Err(e) => last_err = format!("{label}：{e}"),
        }
    }
    let _ = std::fs::remove_file(&part);
    finish_err(
        &st,
        format!("所有下载源都失败了。最后错误：{last_err}"),
    );
}

async fn template_download(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    if !u.is_admin {
        return forbidden();
    }
    let v = parse_body(&body);
    let version_req = sfield(&v, "version");
    let already = {
        match st.tpl_job.lock() {
            Ok(mut j) => {
                if j.running {
                    true
                } else {
                    *j = TplJob {
                        running: true,
                        phase: "downloading".into(),
                        version: String::new(),
                        total: 0,
                        source: String::new(),
                        error: String::new(),
                    };
                    false
                }
            }
            Err(_) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "状态锁异常"),
        }
    };
    if !already {
        let st2 = st.clone();
        std::thread::spawn(move || run_template_job(st2, version_req));
    }
    jresp(StatusCode::OK, json!({ "ok": true, "started": !already }))
}

async fn template_status(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    if !u.is_admin {
        return forbidden();
    }
    let (running, phase, version, total, source, error) = match st.tpl_job.lock() {
        Ok(j) => (
            j.running,
            j.phase.clone(),
            j.version.clone(),
            j.total,
            j.source.clone(),
            j.error.clone(),
        ),
        Err(_) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "状态锁异常"),
    };
    let downloaded = std::fs::metadata(tpl_part_path(&st))
        .map(|m| m.len())
        .unwrap_or(0);
    let tpl_ready = tpl_final_path(&st).exists();
    jresp(
        StatusCode::OK,
        json!({
            "ok": true,
            "state": if phase.is_empty() { "idle".to_string() } else { phase },
            "running": running,
            "version": version,
            "total": total,
            "downloaded": downloaded,
            "source": source,
            "error": error,
            "template_ready": tpl_ready,
            "template_version": get_setting(&st.pool, "tpl_version").await,
        }),
    )
}

/// Build the generated client file name exactly the way the official client
/// parses it (src/custom_server.rs): normally the config JSON is embedded as
/// reverse(base64url_no_pad(json)) in a `--` separated segment, so the file
/// name reveals neither the server address nor the key. If the payload would
/// contain a "--" separator, fall back to the plain host=/key= form.
// v10: listening ports are startup arguments (start.sh / server.env),
// not database settings: the API thread cannot rebind them, and a
// generated client must carry the exact ports the server listens on.
// These helpers parse the raw argument strings; every fallback is the
// standard RustDesk port, so default deployments behave exactly as before.
const DEFAULT_RENDEZVOUS_PORT: u16 = 21116;
const DEFAULT_RELAY_PORT: u16 = 21117;
const DEFAULT_API_PORT: u16 = 21114;

/// Port suffix of a `host` or `host:port` value. A bare IPv6 address
/// (more than one ':' without brackets) is treated as having no port.
fn port_suffix(value: &str) -> Option<u16> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.matches(':').count() > 1 && !value.starts_with('[') {
        return None;
    }
    let (_, port) = value.rsplit_once(':')?;
    port.parse::<u16>().ok()
}

/// Host part of a `host` or `host:port` value (brackets kept for IPv6).
fn host_part(value: &str) -> &str {
    let value = value.trim();
    match port_suffix(value) {
        Some(_) => value.rsplit_once(':').map(|(h, _)| h).unwrap_or(value),
        None => value,
    }
}

fn rendezvous_port(arg: &str) -> u16 {
    arg.trim().parse::<u16>().unwrap_or(DEFAULT_RENDEZVOUS_PORT)
}

fn api_port(api_listen: &str) -> u16 {
    port_suffix(api_listen).unwrap_or(DEFAULT_API_PORT)
}

/// Relay port from the first `--relay-servers` entry, or the default.
/// The relay port must be changed together with hbbr's `-p` anyway, or
/// rendezvous-mediated relay breaks, so this is the same source of truth.
fn relay_port(relay_servers: &str) -> u16 {
    relay_servers
        .split(',')
        .next()
        .and_then(port_suffix)
        .unwrap_or(DEFAULT_RELAY_PORT)
}

/// (rendezvous host, api url, relay host) embedded in a generated client.
/// Ports always come from the server's startup arguments; a port already
/// present in the public-host setting is dropped first so it cannot
/// disagree with the ports the server actually listens on.
fn client_addrs(
    public_host: &str,
    port_arg: &str,
    api_listen: &str,
    relay_servers: &str,
) -> (String, String, String) {
    let host = host_part(public_host);
    let r = rendezvous_port(port_arg);
    let a = api_port(api_listen);
    let y = relay_port(relay_servers);
    (
        format!("{host}:{r}"),
        format!("http://{host}:{a}"),
        format!("{host}:{y}"),
    )
}

fn effective_ports() -> Value {
    let r = rendezvous_port(&crate::common::get_arg("port")) as u32;
    let y = relay_port(&crate::common::get_arg("relay-servers")) as u32;
    let a = api_port(&crate::common::get_arg("api-listen")) as u32;
    let console = crate::common::get_arg("console-listen");
    json!({
        "rendezvous": r,
        "nat_test": r.saturating_sub(1),
        "ws_rendezvous": r.saturating_add(2),
        "relay": y,
        "ws_relay": y.saturating_add(2),
        "api": a,
        "console_listen": console,
        "console": if console.is_empty() {
            Value::Null
        } else {
            json!(api_port(&console) as u32)
        },
        "tls": !crate::common::get_arg("tls-cert").is_empty(),
    })
}

fn generated_filename(host: &str, key: &str, api: &str, relay: &str) -> String {
    let cfg = json!({ "key": key, "host": host, "api": api, "relay": relay });
    let raw = serde_json::to_string(&cfg).unwrap_or_default();
    let b64 = base64::encode_config(raw.as_bytes(), base64::URL_SAFE_NO_PAD);
    let payload: String = b64.chars().rev().collect();
    if payload.is_empty() || payload.contains("--") {
        format!("rustdesk-host={host},key={key},relay={relay}.exe")
    } else {
        format!("rustdesk--{payload}.exe")
    }
}

async fn server_pubkey(st: &ApiState) -> Result<String, String> {
    if !st.key_override.is_empty() && st.key_override != "-" {
        return Ok(st.key_override.clone());
    }
    let text = tokio::fs::read_to_string("id_ed25519.pub")
        .await
        .map_err(|e| format!("读取服务器公钥失败（id_ed25519.pub）：{e}"))?;
    Ok(text.trim().to_string())
}

/// The client configuration for this server — (ID/rendezvous host,
/// API URL, relay host, key) — exactly as embedded into a generated
/// client. Shared by client generation and the console's client-config
/// display, so values an admin copies into a stock client by hand can
/// never drift from what a generated client would have used.
async fn client_config(st: &ApiState) -> Result<(String, String, String, String), String> {
    let public = get_setting(&st.pool, "public_host").await;
    if public.is_empty() {
        return Err("请先在「服务器设置」里填写公网地址".into());
    }
    let key = server_pubkey(st).await?;
    let (host, default_api, relay) = client_addrs(
        &public,
        &crate::common::get_arg("port"),
        &crate::common::get_arg("api-listen"),
        &crate::common::get_arg("relay-servers"),
    );
    let api_override = get_setting(&st.pool, "api_url_override").await;
    let api = if !api_override.is_empty() {
        api_override
    } else if !crate::common::get_arg("tls-cert").is_empty() {
        // HTTPS is enabled on the API listener: generated and manually
        // configured clients must be pointed at the https address.
        default_api.replacen("http://", "https://", 1)
    } else {
        default_api
    };
    Ok((host, api, relay, key))
}

async fn do_generate(
    st: &ApiState,
    created_by: &str,
    label: &str,
) -> Result<(String, String), String> {
    let (host, api, relay, key) = client_config(st).await?;
    let tpl = st.templates_dir.join("rustdesk-windows.exe");
    if !tpl.exists() {
        return Err("请先在「服务器设置」里下载官方客户端模板".into());
    }
    let filename = generated_filename(&host, &key, &api, &relay);
    tokio::fs::create_dir_all(&st.clients_dir)
        .await
        .map_err(|e| e.to_string())?;
    let dest = st.clients_dir.join(&filename);
    tokio::fs::copy(&tpl, &dest)
        .await
        .map_err(|e| e.to_string())?;
    let token = rand_hex(16);
    sqlx::query(
        "insert into gen_clients(filename, token, label, created_by, created_at) values(?, ?, ?, ?, ?)",
    )
    .bind(&filename)
    .bind(&token)
    .bind(label)
    .bind(created_by)
    .bind(now())
    .execute(&st.pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok((filename, token))
}

fn host_base(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1:21114");
    format!("http://{host}")
}

/// Base URL for generated-client download links. The links are served
/// by the API listener (/dl/), so they must be built from the API's
/// public identity — NOT from the address the console was opened with.
/// Since v23 the console can live on its own listener; building the
/// link from the console request's Host produced URLs pointing at the
/// console address, where /dl/ does not exist (user report 2026-10-31).
/// Falls back to the request Host when no public address is configured.
async fn download_base(st: &ApiState, headers: &HeaderMap) -> String {
    let api_override = get_setting(&st.pool, "api_url_override").await;
    if !api_override.is_empty() {
        return api_override.trim_end_matches('/').to_string();
    }
    let public = get_setting(&st.pool, "public_host").await;
    if !public.is_empty() {
        let scheme = if crate::common::get_arg("tls-cert").is_empty() {
            "http"
        } else {
            "https"
        };
        let host = host_part(&public);
        let port = api_port(&crate::common::get_arg("api-listen"));
        return format!("{scheme}://{host}:{port}");
    }
    host_base(headers)
}

async fn gen_client(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    // Any logged-in user may generate: the record is stamped with the
    // caller's name, and listing/deletion below are scoped to it. Server
    // settings and template downloads stay admin-only; generation reads
    // the admin-configured public address, it does not expose settings.
    let v = parse_body(&body);
    let label = sfield(&v, "label");
    match do_generate(&st, &u.username, &label).await {
        Ok((filename, token)) => jresp(
            StatusCode::OK,
            json!({
                "ok": true,
                "filename": filename,
                "download_url": format!("{}/dl/{token}/{filename}", download_base(&st, &headers).await),
            }),
        ),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, &e),
    }
}

/// The client configuration, for display in the console: a user who
/// already has the stock RustDesk client can type these values into
/// its network settings instead of installing a generated client.
/// Available to any logged-in user — the same values are embedded in
/// every generated client, whose download links are shareable by
/// design, so this exposes nothing new.
async fn client_config_view(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
) -> Response {
    if resolve_user(&st, &headers).await.is_none() {
        return unauthorized();
    }
    if get_setting(&st.pool, "public_host").await.is_empty() {
        return jresp(StatusCode::OK, json!({ "ok": true, "configured": false }));
    }
    match client_config(&st).await {
        Ok((host, api, relay, key)) => jresp(
            StatusCode::OK,
            json!({ "ok": true, "configured": true, "host": host, "api": api, "relay": relay, "key": key }),
        ),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn gen_list(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let rows = if u.is_admin {
        sqlx::query(
            "select id, filename, token, label, created_by, created_at from gen_clients order by id desc",
        )
        .fetch_all(&st.pool)
        .await
    } else {
        sqlx::query(
            "select id, filename, token, label, created_by, created_at from gen_clients where created_by=? order by id desc",
        )
        .bind(&u.username)
        .fetch_all(&st.pool)
        .await
    };
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let base = download_base(&st, &headers).await;
    let clients: Vec<Value> = rows
        .iter()
        .map(|r| {
            let filename = r.try_get::<String, _>("filename").unwrap_or_default();
            let token = r.try_get::<String, _>("token").unwrap_or_default();
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "filename": filename,
                "label": r.try_get::<String, _>("label").unwrap_or_default(),
                "created_by": r.try_get::<String, _>("created_by").unwrap_or_default(),
                "created_at": r.try_get::<i64, _>("created_at").unwrap_or(0),
                "download_url": format!("{base}/dl/{token}/{filename}"),
            })
        })
        .collect();
    jresp(StatusCode::OK, json!({ "ok": true, "clients": clients }))
}

async fn gen_delete(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let row = sqlx::query("select filename, created_by from gen_clients where id=?")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    let row = match row {
        Some(r) => r,
        None => return err_resp(StatusCode::NOT_FOUND, "记录不存在"),
    };
    let owner: String = row.try_get("created_by").unwrap_or_default();
    if !u.is_admin && owner != u.username {
        return forbidden();
    }
    if let Ok(f) = row.try_get::<String, _>("filename") {
        let _ = tokio::fs::remove_file(st.clients_dir.join(f)).await;
    }
    match sqlx::query("delete from gen_clients where id=?")
        .bind(id)
        .execute(&st.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => jresp(StatusCode::OK, json!({ "ok": true })),
        Ok(_) => err_resp(StatusCode::NOT_FOUND, "记录不存在"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn pct_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

async fn download(
    Extension(st): Extension<Arc<ApiState>>,
    Path((token, filename)): Path<(String, String)>,
) -> Response {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return err_resp(StatusCode::NOT_FOUND, "文件不存在");
    }
    let row = sqlx::query("select id from gen_clients where token=? and filename=?")
        .bind(&token)
        .bind(&filename)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    if row.is_none() {
        return err_resp(StatusCode::NOT_FOUND, "下载链接无效或已删除");
    }
    let path = st.clients_dir.join(&filename);
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return err_resp(StatusCode::NOT_FOUND, "文件不存在"),
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let stream = hbb_common::tokio_util::io::ReaderStream::new(file);
    let mut headers = HeaderMap::new();
    if let Ok(v) = "application/octet-stream".parse::<header::HeaderValue>() {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = len.to_string().parse::<header::HeaderValue>() {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    if let Ok(v) = format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        filename.replace('"', ""),
        pct_encode(&filename)
    )
    .parse::<header::HeaderValue>()
    {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    let mut resp = Response::new(Body::wrap_stream(stream));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().extend(headers);
    resp.into_response()
}

// ------------------------------ pages & router ------------------------------

async fn root() -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(v) = "/console".parse::<header::HeaderValue>() {
        headers.insert(header::LOCATION, v);
    }
    (StatusCode::FOUND, headers).into_response()
}

async fn login_pg() -> Response {
    Html(api_pages::login_page()).into_response()
}

async fn register_pg() -> Response {
    Html(api_pages::register_page()).into_response()
}

async fn console_home(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
) -> Response {
    match resolve_user(&st, &headers).await {
        Some(_) => Html(api_pages::console_page()).into_response(),
        None => {
            let mut headers = HeaderMap::new();
            if let Ok(v) = "/console/login".parse::<header::HeaderValue>() {
                headers.insert(header::LOCATION, v);
            }
            (StatusCode::FOUND, headers).into_response()
        }
    }
}

/// Which plane a router serves. With `--console-listen` set, the API
/// listener serves only the client-facing endpoints (Public) and the
/// web console moves to its own address (Console), so the management
/// plane does not have to be exposed on the public interface at all.
#[derive(Clone, Copy, PartialEq)]
enum RouterPart {
    All,
    Public,
    Console,
}

pub fn build_router(state: Arc<ApiState>) -> Router {
    build_router_part(state, RouterPart::All)
}

fn build_router_part(state: Arc<ApiState>, part: RouterPart) -> Router {
    let mut r = Router::new().route("/", get(root));
    // Captcha challenges are needed on BOTH planes (the console login
    // page and the client-facing OIDC login page), so register the
    // route once, outside the plane blocks — registering it in both
    // blocks panics the merged All router with a duplicate route.
    r = r.route("/api/captcha", get(captcha_image));
    if part != RouterPart::Console {
        // official client endpoints
        r = r
            .route("/api/login-options", get(login_options))
            .route("/api/oidc/auth", post(oidc_auth))
            .route("/api/oidc/auth-query", get(auth_query))
            .route("/oidc/login", get(oidc_login_get).post(oidc_login_post))
            .route("/api/login", post(password_login))
            .route("/api/logout", post(client_logout))
            .route("/api/heartbeat", post(heartbeat))
            .route("/api/sysinfo", post(sysinfo))
            .route("/api/sysinfo_ver", post(sysinfo_ver).get(sysinfo_ver))
            .route("/api/device-group/accessible", get(device_group_accessible))
            .route("/api/users", get(client_users))
            .route("/api/peers", get(client_peers))
            .route("/api/currentUser", post(current_user))
            .route("/api/ab", get(ab_get).post(ab_post))
            .route("/dl/:token/:filename", get(download));
    }
    if part != RouterPart::Public {
        // web console (management plane)
        r = r
            .route("/console", get(console_home))
            .route("/console/login", get(login_pg))
            .route("/console/register", get(register_pg))
            .route("/api/auth/register", post(web_register))
            .route("/api/auth/login", post(web_login))
            .route("/api/auth/logout", post(web_logout))
            .route("/api/me", get(me))
            .route("/api/client-config", get(client_config_view))
            .route("/api/admin/overview", get(overview))
            .route("/api/admin/users", get(admin_users))
            .route("/api/admin/users/:name/password", post(admin_user_password))
            .route("/api/admin/users/:name/:action", post(admin_user_action))
            .route("/api/devices", get(devices_list))
            .route("/api/tokens", get(tokens_list).post(tokens_create))
            .route("/api/tokens/:id", delete(tokens_delete))
            .route("/api/admin/settings", get(settings_get).post(settings_post))
            .route("/api/admin/template/download", post(template_download))
            .route("/api/admin/template/status", get(template_status))
            .route("/api/admin/gen-client", post(gen_client))
            .route("/api/admin/gen-clients", get(gen_list))
            .route("/api/admin/gen-clients/:id", delete(gen_delete));
    }
    r.layer(Extension(state))
}

/// A TLS-terminated connection that still knows its peer address, so
/// the ConnectInfo extractor keeps working over HTTPS.
struct TlsConn {
    stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    remote: SocketAddr,
}

impl tokio::io::AsyncRead for TlsConn {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for TlsConn {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        data: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, data)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl axum::extract::connect_info::Connected<&TlsConn> for SocketAddr {
    fn connect_info(target: &TlsConn) -> Self {
        target.remote
    }
}

/// hyper Accept impl fed by the TLS accept loop below.
struct TlsAccept {
    rx: tokio::sync::mpsc::Receiver<std::io::Result<TlsConn>>,
}

impl hyper::server::accept::Accept for TlsAccept {
    type Conn = TlsConn;
    type Error = std::io::Error;

    fn poll_accept(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Self::Conn, Self::Error>>> {
        self.rx.poll_recv(cx)
    }
}

/// Extract the DER bodies of all PEM blocks with the given label.
fn pem_blocks(pem: &str, label: &str) -> Vec<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in pem.lines() {
        let line = line.trim();
        if line == begin {
            cur = Some(String::new());
        } else if line == end {
            if let Some(b64) = cur.take() {
                if let Ok(der) = base64::decode(b64.as_bytes()) {
                    out.push(der);
                }
            }
        } else if let Some(buf) = cur.as_mut() {
            buf.push_str(line);
        }
    }
    out
}

fn load_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<Arc<tokio_rustls::rustls::ServerConfig>, anyhow::Error> {
    let cert_pem = std::fs::read_to_string(cert_path)
        .map_err(|e| anyhow::anyhow!("cannot read tls cert {cert_path}: {e}"))?;
    let key_pem = std::fs::read_to_string(key_path)
        .map_err(|e| anyhow::anyhow!("cannot read tls key {key_path}: {e}"))?;
    let certs: Vec<tokio_rustls::rustls::Certificate> = pem_blocks(&cert_pem, "CERTIFICATE")
        .into_iter()
        .map(tokio_rustls::rustls::Certificate)
        .collect();
    if certs.is_empty() {
        return Err(anyhow::anyhow!("no certificate found in {cert_path}"));
    }
    let mut keys = pem_blocks(&key_pem, "PRIVATE KEY");
    keys.extend(pem_blocks(&key_pem, "RSA PRIVATE KEY"));
    keys.extend(pem_blocks(&key_pem, "EC PRIVATE KEY"));
    let key = keys
        .into_iter()
        .next()
        .map(tokio_rustls::rustls::PrivateKey)
        .ok_or_else(|| anyhow::anyhow!("no private key found in {key_path}"))?;
    let cfg = tokio_rustls::rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| anyhow::anyhow!("invalid tls cert/key pair: {e}"))?;
    Ok(Arc::new(cfg))
}

async fn serve_router(
    router: Router,
    addr: SocketAddr,
    tls: Option<Arc<tokio_rustls::rustls::ServerConfig>>,
) -> Result<(), anyhow::Error> {
    let make = router.into_make_service_with_connect_info::<SocketAddr>();
    match tls {
        None => {
            axum::Server::bind(&addr).serve(make).await?;
        }
        Some(cfg) => {
            let acceptor = tokio_rustls::TlsAcceptor::from(cfg);
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| anyhow::anyhow!("bind {addr} failed: {e}"))?;
            let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<TlsConn>>(64);
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((sock, peer)) => {
                            let acc = acceptor.clone();
                            let tx = tx.clone();
                            tokio::spawn(async move {
                                match acc.accept(sock).await {
                                    Ok(stream) => {
                                        let _ = tx
                                            .send(Ok(TlsConn {
                                                stream,
                                                remote: peer,
                                            }))
                                            .await;
                                    }
                                    Err(e) => {
                                        log::debug!("tls handshake failed from {peer}: {e}");
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            log::warn!("api tls accept error: {e}");
                            break;
                        }
                    }
                }
            });
            axum::Server::builder(TlsAccept { rx }).serve(make).await?;
        }
    }
    Ok(())
}

/// Entry point called from main.rs; runs the API service on its own runtime
/// inside a dedicated thread so the rendezvous server is untouched.
///
/// `console_listen` (from `--console-listen`) optionally moves the web
/// console to its own address; the main listener then serves only the
/// client-facing endpoints. `tls_cert`/`tls_key` (from `--tls-cert` /
/// `--tls-key`) optionally enable HTTPS on both listeners.
pub fn run(
    listen: &str,
    db_path: &str,
    key_override: &str,
    console_listen: &str,
    tls_cert: &str,
    tls_key: &str,
) -> ResultType<()> {
    let _ = sodiumoxide::init();
    let listen = listen.to_string();
    let db_path = db_path.to_string();
    let key_override = key_override.to_string();
    let console_listen = console_listen.to_string();
    let tls_cert = tls_cert.to_string();
    let tls_key = tls_key.to_string();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let pool = init_db(&db_path)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let cwd = std::env::current_dir()?;
        let state = Arc::new(ApiState {
            pool,
            clients_dir: cwd.join("clients"),
            templates_dir: cwd.join("client-templates"),
            peer_db_path: "db_v2.sqlite3".to_string(),
            key_override,
            tpl_job: std::sync::Mutex::new(TplJob::default()),
            guard: std::sync::Mutex::new(AuthGuard::default()),
        });
        let tls = if tls_cert.is_empty() && tls_key.is_empty() {
            None
        } else if tls_cert.is_empty() || tls_key.is_empty() {
            return Err(anyhow::anyhow!(
                "--tls-cert and --tls-key must be given together"
            ));
        } else {
            let cfg = load_tls_config(&tls_cert, &tls_key)?;
            log::info!("https enabled on the api/console listeners");
            Some(cfg)
        };
        let addr: SocketAddr = listen
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid api listen address: {e}"))?;
        if console_listen.is_empty() {
            log::info!("api/web console service listening on {addr}");
            serve_router(build_router(state), addr, tls).await?;
        } else {
            let console_addr: SocketAddr = console_listen
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid console listen address: {e}"))?;
            log::info!(
                "api service listening on {addr} (client endpoints only); web console listening on {console_addr}"
            );
            let api = serve_router(
                build_router_part(state.clone(), RouterPart::Public),
                addr,
                tls.clone(),
            );
            let con = serve_router(
                build_router_part(state, RouterPart::Console),
                console_addr,
                tls,
            );
            tokio::try_join!(api, con)?;
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_locks_after_max_failures_and_expires() {
        let mut g = AuthGuard::default();
        let t = 10_000;
        for _ in 0..4 {
            assert!(g.record_failure("admin", "1.2.3.4", t).is_empty());
        }
        assert_eq!(g.locked_remaining("admin", "1.2.3.4", t), 0);
        let hits = g.record_failure("admin", "1.2.3.4", t);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].scope, "account");
        assert_eq!(hits[0].secs, 900);
        assert_eq!(g.locked_remaining("admin", "1.2.3.4", t), 900);
        assert_eq!(g.locked_remaining("admin", "1.2.3.4", t + 901), 0);
    }

    #[test]
    fn guard_lock_doubles_on_repeat() {
        let mut g = AuthGuard::default();
        let t = 20_000;
        for _ in 0..5 {
            g.record_failure("admin", "1.2.3.4", t);
        }
        let t2 = t + 901; // first lock expired
        let mut hits = Vec::new();
        for _ in 0..5 {
            hits = g.record_failure("admin", "1.2.3.4", t2);
        }
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].level, 2);
        assert_eq!(hits[0].secs, 1800);
    }

    #[test]
    fn guard_success_clears_counter() {
        let mut g = AuthGuard::default();
        let t = 30_000;
        for _ in 0..4 {
            g.record_failure("admin", "1.2.3.4", t);
        }
        g.record_success("admin");
        for _ in 0..4 {
            assert!(g.record_failure("admin", "1.2.3.4", t).is_empty());
        }
        assert_eq!(g.locked_remaining("admin", "1.2.3.4", t), 0);
    }

    #[test]
    fn guard_ip_lock_covers_other_accounts() {
        let mut g = AuthGuard::default();
        let t = 40_000;
        for i in 0..30 {
            g.record_failure(&format!("user{i}"), "9.9.9.9", t);
        }
        // No single account hit its own limit, but the IP is locked, so
        // even a fresh account with the right password is turned away.
        assert!(g.locked_remaining("brand-new", "9.9.9.9", t) > 0);
        assert_eq!(g.locked_remaining("brand-new", "8.8.8.8", t), 0);
    }

    #[test]
    fn guard_registration_throttle() {
        let mut g = AuthGuard::default();
        let t = 50_000;
        for _ in 0..10 {
            assert!(g.reg_allowed("1.2.3.4", t));
            g.reg_record("1.2.3.4", t);
        }
        assert!(!g.reg_allowed("1.2.3.4", t));
        assert!(g.reg_allowed("1.2.3.4", t + 3601));
    }

    /// Replica of the official client's decoder (src/custom_server.rs):
    /// reverse the segment, base64url-decode, parse the JSON.
    fn client_parse(filename: &str) -> Option<(String, String, String, String)> {
        let s = filename.strip_suffix(".exe").unwrap_or(filename);
        if s.to_lowercase().contains("host=") {
            return None;
        }
        let s = s
            .replace("-licensed---", "--")
            .replace("-licensed--", "--")
            .replace("-licensed-", "--");
        for seg in s.split("--") {
            let rev: String = seg.trim().chars().rev().collect();
            if let Ok(data) = base64::decode_config(&rev, base64::URL_SAFE_NO_PAD) {
                if let Ok(v) = serde_json::from_slice::<Value>(&data) {
                    return Some((
                        v["host"].as_str().unwrap_or_default().to_string(),
                        v["key"].as_str().unwrap_or_default().to_string(),
                        v["api"].as_str().unwrap_or_default().to_string(),
                        v["relay"].as_str().unwrap_or_default().to_string(),
                    ));
                }
            }
        }
        None
    }

    #[test]
    fn generated_filename_roundtrip() {
        let name = generated_filename(
            "rd.example.com",
            "AbCdEf0123456789+/xyz=",
            "http://rd.example.com:21114",
            "rd.example.com",
        );
        let parsed = client_parse(&name);
        assert_eq!(
            parsed,
            Some((
                "rd.example.com".to_string(),
                "AbCdEf0123456789+/xyz=".to_string(),
                "http://rd.example.com:21114".to_string(),
                "rd.example.com".to_string(),
            )),
            "filename was: {name}"
        );
    }

    #[test]
    fn port_helpers() {
        assert_eq!(rendezvous_port(""), 21116);
        assert_eq!(rendezvous_port("31116"), 31116);
        assert_eq!(rendezvous_port("abc"), 21116);
        assert_eq!(api_port("0.0.0.0:31114"), 31114);
        assert_eq!(api_port("off"), 21114);
        assert_eq!(api_port(""), 21114);
        assert_eq!(relay_port("203.0.113.10:31117"), 31117);
        assert_eq!(relay_port("203.0.113.10"), 21117);
        assert_eq!(relay_port(""), 21117);
        assert_eq!(relay_port("a:1,b:2"), 1);
        assert_eq!(port_suffix("::1"), None);
        assert_eq!(port_suffix("[::1]:31117"), Some(31117));
        assert_eq!(host_part("rd.example.com:31116"), "rd.example.com");
        assert_eq!(host_part("rd.example.com"), "rd.example.com");
        let (h, a, r) = client_addrs(
            "rd.example.com",
            "31116",
            "0.0.0.0:31114",
            "203.0.113.10:31117",
        );
        assert_eq!(h, "rd.example.com:31116");
        assert_eq!(a, "http://rd.example.com:31114");
        assert_eq!(r, "rd.example.com:31117");
        let (h, a, r) = client_addrs("rd.example.com", "", "", "");
        assert_eq!(h, "rd.example.com:21116");
        assert_eq!(a, "http://rd.example.com:21114");
        assert_eq!(r, "rd.example.com:21117");
        let (h, _, _) = client_addrs("rd.example.com:9999", "", "", "");
        assert_eq!(h, "rd.example.com:21116");
    }
}
