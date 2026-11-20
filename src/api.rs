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
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use axum::{
    body::Body,
    extract::{ConnectInfo, Extension, Form, Path, Query},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use hbb_common::{anyhow, log, tokio, ResultType};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
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
    /// Server directory (the exe directory; anchor_to_exe_dir runs at
    /// startup, so this is the process working directory). Staged
    /// update binaries land here, never anywhere else.
    exe_dir: PathBuf,
    peer_db_path: String,
    key_override: String,
    tpl_job: std::sync::Mutex<TplJob>,
    upd_job: std::sync::Mutex<UpdJob>,
    guard: std::sync::Mutex<AuthGuard>,
    /// Cached latest-release data for /api/client-downloads:
    /// (fetched_at, version, platforms). The GitHub API allows only 60
    /// unauthenticated requests per hour per shared IP, and the console
    /// fetches this on every visit to the generate-client page, so the
    /// upstream data is cached for one hour. Mirrors are NOT cached:
    /// they are computed per request from the current github_mirror
    /// setting, so an admin's mirror change takes effect immediately.
    dl_cache: std::sync::Mutex<Option<(std::time::Instant, String, Vec<ClientDownload>)>>,
    /// Outstanding relay registration challenges: nonce_hex ->
    /// (token_id, expires_at_unix). A challenge is consumed on its
    /// first verification attempt and expires after 5 minutes.
    reg_challenges: std::sync::Mutex<HashMap<String, (String, i64)>>,
    /// Recently seen relay heartbeat nonces for replay rejection:
    /// (node_id, nonce_hex) -> seen_at_unix. Entries older than 10
    /// minutes are pruned on each heartbeat.
    hb_replay: std::sync::Mutex<HashMap<(i64, String), i64>>,
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

/// Fork release number ("32" displays as v32 in the console and is
/// compared against our GitHub release tags for one-click updates).
/// Bump once per release, together with dist-pro/VERSION.
/// NOTE: this deliberately does NOT live in version.rs — build.rs
/// regenerates that file on every build (see hbb_common::gen_version)
/// and would wipe a hand-written constant.
const PRO_VERSION: &str = "43";

/// Progress state of the background one-click server update download.
#[derive(Default)]
pub struct UpdJob {
    running: bool,
    phase: String, // idle | downloading | extracting | done | error
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
        "create table if not exists devices(device_id text not null, uuid text not null, username text, hostname text not null default '', os text not null default '', os_username text not null default '', version text not null default '', last_seen integer not null, sysinfo text not null default '', last_relay text not null default '', last_relay_at integer not null default 0, conns text not null default '[]', conns_updated_at integer not null default 0, primary key(device_id, uuid))",
        "create table if not exists settings(key text primary key, value text not null)",
        "create table if not exists gen_clients(id integer primary key autoincrement, filename text not null, token text not null, label text not null default '', created_by text not null, created_at integer not null)",
        "create table if not exists ab_store(username text primary key, data text not null)",
        "create table if not exists captcha_codes(id text primary key, code_hash text not null, expires_at integer not null)",
        "create table if not exists relay_reg_tokens(token_id text primary key, link_key text not null, label text not null default '', created_by text not null, created_at integer not null, expires_at integer not null, used_at integer, used_by integer)",
        "create table if not exists relay_nodes(id integer primary key autoincrement, name text not null, address text not null default '', last_ip text not null default '', link_key text not null, key_delivered integer not null default 0, registered_at integer not null, last_heartbeat integer not null default 0, agent_version text not null default '', hbbr_version text not null default '', hbbr_running integer not null default 0, hbbr_started_at integer not null default 0, listen_port integer not null default 0, sessions integer not null default 0, total_kbps integer not null default 0, config_json text not null default '{}', config_version integer not null default 1, applied_version integer not null default 0, pending_command text not null default '', created_by text not null default '')",
        // v40: connection-audit records posted by the controlled side
        // (POST /api/audit/conn). nonce is UNIQUE: the client retries
        // a post for up to 120 s, and the insert is insert-or-ignore,
        // so a retry never stores a second row. Deliberate deviation
        // from the official server's 5-minute nonce window: permanent
        // dedup is a superset and needs no background expiry.
        "create table if not exists audit_conn(id integer primary key autoincrement, nonce text not null unique, device_id text not null, device_uuid text not null default '', conn_id integer not null, session_id text not null default '', action text not null, ip text not null default '', peer_id text not null default '', peer_name text not null default '', conn_type text not null default '', primary_auth integer, two_factor integer, conn_audit_ref text not null default '', raw text not null default '', received_at integer not null)",
        "create index if not exists idx_audit_conn_session on audit_conn(device_id, conn_id, received_at)",
        "create index if not exists idx_audit_conn_received on audit_conn(received_at)",
        // v43: ends of sessions whose "close" never arrived, recorded
        // by the heartbeat reaper (issue #11). audit_conn keeps the
        // raw truth: no close rows are fabricated there.
        "create table if not exists audit_session_close(device_id text not null, conn_id integer not null, ended_at integer not null, reason text not null default 'stale', primary key(device_id, conn_id))",
    ];
    for s in stmts {
        sqlx::query(s)
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;
    }
    // v38: the relay a device's most recent relayed session actually
    // used (recorded from RelayResponse / RequestRelay; empty until
    // the device has had a relayed session). ALTER TABLE fails with
    // "duplicate column name" on databases that already have the
    // columns; that error is expected and ignored, any other error
    // fails startup so a broken migration is never silent.
    for s in [
        "alter table devices add column last_relay text not null default ''",
        "alter table devices add column last_relay_at integer not null default 0",
    ] {
        if let Err(e) = sqlx::query(s).execute(&pool).await {
            let msg = e.to_string();
            if !msg.contains("duplicate column name") {
                return Err(msg);
            }
        }
    }
    // v43: the live-connection list from the device's latest
    // heartbeat (issue #11 reaping). Same duplicate-column tolerance
    // as the v38 migration above.
    for s in [
        "alter table devices add column conns text not null default '[]'",
        "alter table devices add column conns_updated_at integer not null default 0",
    ] {
        if let Err(e) = sqlx::query(s).execute(&pool).await {
            let msg = e.to_string();
            if !msg.contains("duplicate column name") {
                return Err(msg);
            }
        }
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

/// Live connection IDs from a heartbeat body: the `conns` array the
/// official client sends (src/hbbs_http/sync.rs). The field is
/// OMITTED when the list is empty, so a missing field means no live
/// connections; non-number entries are ignored. Same i32 ID
/// namespace as the audit conn_id (inner.id in post_conn_audit).
fn heartbeat_conns(v: &Value) -> Vec<i64> {
    match v.get("conns") {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_i64()).collect(),
        _ => Vec::new(),
    }
}

/// Store a device's latest live-connection list (`at` = heartbeat
/// receipt time, unix seconds). Called after upsert_device, so the
/// row exists.
async fn store_heartbeat_conns(
    pool: &SqlitePool,
    device_id: &str,
    uuid: &str,
    conns: &[i64],
    at: i64,
) {
    let json = serde_json::to_string(conns).unwrap_or_else(|_| "[]".to_string());
    let _ = sqlx::query(
        "update devices set conns=?, conns_updated_at=? where device_id=? and uuid=?",
    )
    .bind(&json)
    .bind(at)
    .bind(device_id)
    .bind(uuid)
    .execute(pool)
    .await;
}

/// Grace period before a session missing from the heartbeat's live
/// list is reaped: covers the race where a session just started and
/// its audit "new" record landed before the next heartbeat. A close
/// posted late still wins, because reaping only touches groups with
/// no close row.
const AUDIT_REAP_GRACE_SECS: i64 = 60;

/// Reap dead sessions for one device (v43, issue #11): the
/// controlled client posts "close" exactly once at connection-loop
/// exit, so a crash/restart/reboot (or exhausted close retries)
/// leaves the session open forever. The heartbeat's live-connection
/// list is the ground truth instead: an open session whose last
/// event is older than the grace period and whose conn_id is absent
/// from the just-received list is dead — record its end in
/// audit_session_close (never fabricate audit_conn rows). A conn_id
/// that reappears in the live list has any stale record deleted:
/// conn_id restarts when the client restarts, so the id may be
/// reused by a new session.
async fn reap_audit_sessions(
    pool: &SqlitePool,
    device_id: &str,
    live_conns: &[i64],
    at: i64,
) {
    // Reappearance hygiene first: a live conn_id is not dead.
    for conn_id in live_conns {
        let _ = sqlx::query("delete from audit_session_close where device_id=? and conn_id=?")
            .bind(device_id)
            .bind(conn_id)
            .execute(pool)
            .await;
    }
    let rows = sqlx::query(
        "select a.conn_id as conn_id from audit_conn a \
         where a.device_id=? \
         and not exists (select 1 from audit_conn c where c.device_id=a.device_id and c.conn_id=a.conn_id and c.action='close') \
         and not exists (select 1 from audit_session_close s where s.device_id=a.device_id and s.conn_id=a.conn_id) \
         group by a.conn_id having max(a.received_at) < ?",
    )
    .bind(device_id)
    .bind(at - AUDIT_REAP_GRACE_SECS)
    .fetch_all(pool)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(_) => return,
    };
    for r in &rows {
        let conn_id = r.try_get::<i64, _>("conn_id").unwrap_or(0);
        if live_conns.contains(&conn_id) {
            continue;
        }
        let _ = sqlx::query(
            "insert or ignore into audit_session_close(device_id, conn_id, ended_at, reason) values(?, ?, ?, 'stale')",
        )
        .bind(device_id)
        .bind(conn_id)
        .bind(at)
        .execute(pool)
        .await;
    }
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
        // v43: record the live-connection list and reap sessions the
        // client no longer reports (issue #11).
        let conns = heartbeat_conns(&v);
        let at = now();
        store_heartbeat_conns(&st.pool, &device_id, &uuid, &conns, at).await;
        reap_audit_sessions(&st.pool, &device_id, &conns, at).await;
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

// ------------------------------ address book ------------------------------
// v40.1 (issue #8): auto-add the caller's bound devices to the address
// book. The stored blob is opaque (ab_store keeps the client's `data`
// string verbatim; the server never parses peer fields), so every peer
// field the client knows — forceAlwaysRelay, rdpPort, rdpUsername,
// loginName, same_server, password, and the rest — already round-trips
// untouched; the only server-side work is the merge below.
//
// Merge-on-GET / strip-on-POST:
// - ab_get merges the caller's bound devices (devices.username == caller)
//   into the returned peers with the server-managed tag 登录设备. A
//   device the user already has an entry for keeps the user's entry
//   as-is (no tag added, no field touched).
// - ab_post strips peers tagged 登录设备 before storing: they are
//   re-added on GET from live binding state, so an unbind self-heals
//   on the next GET. A user who manually tags a peer 登录设备 sees it
//   stripped on the next sync; the tag is server-managed.
// - No stored blob + bound devices: synthesize {"tags", "peers"}.
//   No blob + no devices: keep the literal "null" contract.
// - A malformed stored blob is returned verbatim (fail open).
// Merged blobs are re-serialized through serde_json::Value, which
// normalizes object key order (alphabetical); the JSON is semantically
// identical and the client does not depend on key order. When nothing
// is merged or stripped, the stored string is returned verbatim.
const AB_LOGIN_DEVICES_TAG: &str = "登录设备";

struct BoundDevice {
    device_id: String,
    hostname: String,
    os: String,
    os_username: String,
}

async fn bound_devices(st: &ApiState, username: &str) -> Vec<BoundDevice> {
    let rows = sqlx::query(
        "select device_id, hostname, os, os_username from devices where username=? order by device_id",
    )
    .bind(username)
    .fetch_all(&st.pool)
    .await;
    match rows {
        Ok(rows) => rows
            .iter()
            .map(|r| BoundDevice {
                device_id: r.try_get("device_id").unwrap_or_default(),
                hostname: r.try_get("hostname").unwrap_or_default(),
                os: r.try_get("os").unwrap_or_default(),
                os_username: r.try_get("os_username").unwrap_or_default(),
            })
            .filter(|d| !d.device_id.is_empty())
            .collect(),
        // DB failure must never break the address book: fail open with
        // no merge (the stored blob is returned as-is).
        Err(_) => Vec::new(),
    }
}

/// Map a sysinfo `os` string to the client's platform strings
/// (official client 1.5.0 src/common.rs PLATFORM_WINDOWS/LINUX/MACOS/
/// ANDROID; iOS is "iOS"). The client reports os as
/// "{distribution_id} / {long_os_version}", e.g.
/// "windows / Windows 10 Pro", "ubuntu / Ubuntu 22.04".
/// Unknown values pass through unchanged.
fn ab_platform_from_os(os: &str) -> String {
    let lower = os.to_lowercase();
    if lower.contains("windows") {
        "Windows".to_string()
    } else if lower.contains("android") {
        "Android".to_string()
    } else if lower.contains("ios") {
        "iOS".to_string()
    } else if lower.contains("mac") || lower.contains("darwin") || lower.contains("apple") {
        "Mac OS".to_string()
    } else if lower.contains("linux")
        || [
            "ubuntu", "debian", "fedora", "arch", "centos", "opensuse", "gentoo", "alpine",
            "raspbian", "manjaro", "mint", "redhat", "suse", "nixos",
        ]
        .iter()
        .any(|d| lower.contains(d))
    {
        "Linux".to_string()
    } else {
        os.to_string()
    }
}

/// The peer object the server auto-adds for a bound device. No password
/// or hash: the user sets those in the client.
fn ab_peer_for_device(d: &BoundDevice) -> Value {
    json!({
        "id": d.device_id,
        "hash": "",
        "password": "",
        "username": d.os_username,
        "hostname": d.hostname,
        "platform": ab_platform_from_os(&d.os),
        "alias": "",
        "tags": [AB_LOGIN_DEVICES_TAG],
        "forceAlwaysRelay": "false",
        "rdpPort": "",
        "rdpUsername": "",
        "loginName": "",
        "same_server": null,
    })
}

/// A peer's id as a string (the blob normally stores string ids; accept
/// a numeric id too so dedup never misses on type alone).
fn ab_peer_id(p: &Value) -> Option<String> {
    match p.get("id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// Merge bound devices into a stored address-book blob. Returns None
/// when the blob is not a parseable JSON object (caller returns it
/// verbatim). With no devices, the blob is returned unchanged.
fn ab_merge_bound_devices(blob: &str, devices: &[BoundDevice]) -> Option<String> {
    if devices.is_empty() {
        return Some(blob.to_string());
    }
    let mut v: Value = serde_json::from_str(blob).ok()?;
    let obj = v.as_object_mut()?;
    let tags = obj
        .entry("tags")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(tags) = tags {
        if !tags
            .iter()
            .any(|t| t.as_str() == Some(AB_LOGIN_DEVICES_TAG))
        {
            tags.push(Value::String(AB_LOGIN_DEVICES_TAG.to_string()));
        }
    }
    let peers = obj
        .entry("peers")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(peers) = peers {
        for d in devices {
            let exists = peers
                .iter()
                .any(|p| ab_peer_id(p).as_deref() == Some(d.device_id.as_str()));
            if !exists {
                peers.push(ab_peer_for_device(d));
            }
        }
    }
    Some(v.to_string())
}

/// Strip server-managed peers (tagged 登录设备) from a posted blob
/// before storing. Returns the blob to store: unchanged when nothing
/// was stripped or the data is not parseable JSON (fail open).
fn ab_strip_managed_peers(data: &str) -> String {
    let mut v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return data.to_string(),
    };
    let mut stripped = false;
    if let Some(peers) = v.get_mut("peers").and_then(|p| p.as_array_mut()) {
        let before = peers.len();
        peers.retain(|p| {
            !p.get("tags")
                .and_then(|t| t.as_array())
                .map(|tags| {
                    tags.iter()
                        .any(|t| t.as_str() == Some(AB_LOGIN_DEVICES_TAG))
                })
                .unwrap_or(false)
        });
        stripped = peers.len() != before;
    }
    if stripped {
        v.to_string()
    } else {
        data.to_string()
    }
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
    // Bound devices for the merge; empty on DB error (fail open).
    let devices = bound_devices(&st, &caller.username).await;
    match row {
        Some(r) => {
            let data: String = r.try_get("data").unwrap_or_default();
            // Malformed blob: merge returns None, return it verbatim.
            let data = ab_merge_bound_devices(&data, &devices).unwrap_or(data);
            jresp(
                StatusCode::OK,
                json!({ "data": data, "licensed_devices": 0 }),
            )
        }
        None => {
            if devices.is_empty() {
                // The client treats the literal body "null" as an empty address book.
                (StatusCode::OK, "null").into_response()
            } else {
                let peers: Vec<Value> =
                    devices.iter().map(ab_peer_for_device).collect();
                let blob = json!({
                    "tags": [AB_LOGIN_DEVICES_TAG],
                    "peers": peers,
                });
                jresp(
                    StatusCode::OK,
                    json!({ "data": blob.to_string(), "licensed_devices": 0 }),
                )
            }
        }
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
    // Strip server-managed peers: they are re-added on GET from live
    // binding state, so an unbind self-heals on the next GET.
    let data = ab_strip_managed_peers(&data);
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

/// Self-service password change (issue #9). Any logged-in web user
/// may change ONLY their own password, and only after proving the
/// old one. Deliberately separate from the admin reset endpoint:
/// opening that one to regular users would let any logged-in user
/// change anyone else's password. Wrong old password returns 400
/// (not 401): the console's api() helper treats any 401 as "session
/// expired" and redirects to the login page, which would swallow
/// the inline error.
async fn change_password(
    Extension(st): Extension<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let ip = addr.ip().to_string();
    let remain = guard_locked(&st, &u.username, &ip);
    if remain > 0 {
        auth_log(
            "pwd-change-locked",
            "web",
            &u.username,
            &ip,
            &format!("remain_secs={remain}"),
        );
        return err_resp(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("尝试过于频繁，已临时锁定，请约 {} 分钟后再试", (remain + 59) / 60),
        );
    }
    let v = parse_body(&body);
    let old_password = sfield(&v, "old_password");
    let new_password = sfield(&v, "new_password");
    if new_password.len() < 6 {
        return err_resp(StatusCode::BAD_REQUEST, "密码至少 6 位");
    }
    match verify_user(&st.pool, &u.username, &old_password).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            guard_fail(&st, &u.username, &ip, "web");
            auth_log(
                "pwd-change-fail",
                "web",
                &u.username,
                &ip,
                "reason=bad-old-password",
            );
            return err_resp(StatusCode::BAD_REQUEST, "旧密码不正确");
        }
        Err(e) => {
            return err_resp(StatusCode::FORBIDDEN, &e);
        }
    }
    let hash = match bcrypt::hash(&new_password, 10) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if let Err(e) = sqlx::query("update users set password_hash=? where username=?")
        .bind(&hash)
        .bind(&u.username)
        .execute(&st.pool)
        .await
    {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    guard_success(&st, &u.username);
    // Revoke the caller's OTHER web sessions (other devices/browsers
    // must sign in again with the new password); the current session
    // stays valid. API-token callers have no web session to keep, so
    // all of their web sessions go.
    let current_sid = cookie_val(&headers, "rdp_sid");
    let revoked = match &current_sid {
        Some(sid) => sqlx::query("delete from web_sessions where username=? and token!=?")
            .bind(&u.username)
            .bind(sid)
            .execute(&st.pool)
            .await,
        None => sqlx::query("delete from web_sessions where username=?")
            .bind(&u.username)
            .execute(&st.pool)
            .await,
    };
    let revoked_n = revoked.map(|r| r.rows_affected()).unwrap_or(0);
    auth_log(
        "password-changed",
        "web",
        &u.username,
        &ip,
        &format!("other_sessions_revoked={revoked_n}"),
    );
    jresp(StatusCode::OK, json!({ "ok": true }))
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

/// Admin creates a user from user management (issue #6). With
/// registration closed, the register page is the only creation path
/// and it signs the admin out (it issues a session for the new user),
/// so admins get a dedicated endpoint instead. Deliberately issues
/// NO session for the new user: the admin stays logged in.
async fn admin_create_user(
    Extension(st): Extension<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
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
    let username = sfield(&v, "username");
    let password = sfield(&v, "password");
    // create_user validates the username (3-32 chars, [A-Za-z0-9_-.])
    // and the password (>= 6 chars) and reports duplicates.
    if let Err(e) = create_user(&st.pool, &username, &password).await {
        return err_resp(StatusCode::BAD_REQUEST, &e);
    }
    auth_log(
        "user-created",
        "admin",
        &username,
        &addr.ip().to_string(),
        &format!("by={}", u.username),
    );
    jresp(StatusCode::OK, json!({ "ok": true }))
}

async fn devices_list(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let rows = if u.is_admin {
        sqlx::query(
            "select device_id, uuid, username, hostname, os, version, last_seen, last_relay, last_relay_at from devices order by last_seen desc",
        )
        .fetch_all(&st.pool)
        .await
    } else {
        sqlx::query(
            "select device_id, uuid, username, hostname, os, version, last_seen, last_relay, last_relay_at from devices where username=? order by last_seen desc",
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
                "last_relay": r.try_get::<String, _>("last_relay").unwrap_or_default(),
                "last_relay_at": r.try_get::<i64, _>("last_relay_at").unwrap_or(0),
            })
        })
        .collect();
    // Relay nodes (admin only) ride along so the device list can
    // show them with a relay badge. The shape matches relays_list
    // items; link keys are never included.
    let t = now();
    let relays: Vec<Value> = if u.is_admin {
        match sqlx::query("select * from relay_nodes order by id desc")
            .fetch_all(&st.pool)
            .await
        {
            Ok(rs) => rs.iter().map(|r| relay_node_summary(r, t)).collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };
    jresp(
        StatusCode::OK,
        json!({ "ok": true, "now": t, "devices": devices, "relays": relays }),
    )
}

// ------------------------------ connection audit (v40) ------------------------------
//
// The controlled side of the official client (1.5.x) posts one audit
// record per connection event to POST {api}/api/audit/conn:
//   "new"  — on_open, {ip, action: "new"} (ip = the socket peer: the
//            relay for a relayed session, the controller when direct)
//   (auth) — after login, {peer: [controller id, name], type,
//            primary_auth?, two_factor?} (no action field)
//   "close"— connection loop exit, {action: "close"}
// Every record also carries id (controlled device), uuid (base64),
// conn_id, session_id and a fresh nonce (unique per record).
// See client src/server/connection.rs post_conn_audit /
// post_audit_async and src/common.rs get_audit_server.
//
// Response contract (from post_audit_async): success is HTTP 200 with
// an EMPTY body; a handler failure is 200 with {"error": ...} (the
// client retries); any other 4xx is a deterministic rejection (the
// record is dropped, no retry). Only /conn is implemented in v40;
// /file and /alarm keep 404ing (future work).

/// A JSON field that may arrive as a string or a number (device IDs
/// and peer IDs are numbers in some client paths), normalized to a
/// string. Anything else yields "".
fn jstr(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn jstr_val(x: &Value) -> String {
    match x {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Parse an IP literal, mapping IPv4-mapped IPv6 (::ffff:1.2.3.4) to
/// plain IPv4 so relay comparison works across address families.
fn norm_ip(s: &str) -> Option<std::net::IpAddr> {
    let ip: std::net::IpAddr = s.trim().parse().ok()?;
    Some(match ip {
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => std::net::IpAddr::V4(v4),
            None => std::net::IpAddr::V6(v6),
        },
        v4 => v4,
    })
}

/// Resolve the --relay-servers list (comma-separated hosts, each a
/// bare host, host:port or [ipv6]:port) to the set of IPs hbbr listens
/// on. Domains resolve via ToSocketAddrs like test_if_valid_server;
/// all resolved addresses are collected. Empty when the list is empty
/// or nothing resolves.
fn resolve_relay_ips(servers: &str) -> std::collections::HashSet<std::net::IpAddr> {
    use std::net::ToSocketAddrs;
    let mut out = std::collections::HashSet::new();
    for entry in servers.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // Strip brackets and any :port suffix to get the bare host.
        let host = if let Some(rest) = entry.strip_prefix('[') {
            match rest.split_once(']') {
                Some((h, _)) => h.to_string(),
                None => rest.to_string(),
            }
        } else if entry.matches(':').count() == 1 {
            entry
                .split_once(':')
                .map(|(h, _)| h.to_string())
                .unwrap_or_else(|| entry.to_string())
        } else {
            entry.to_string()
        };
        if host.is_empty() {
            continue;
        }
        if let Some(ip) = norm_ip(&host) {
            out.insert(ip);
            continue;
        }
        if let Ok(addrs) = format!("{host}:0").to_socket_addrs() {
            for a in addrs {
                if let Some(ip) = norm_ip(&a.ip().to_string()) {
                    out.insert(ip);
                }
            }
        }
    }
    out
}

/// Union of the --relay-servers list and the server's own public
/// address (public_host setting, same source client_config() uses):
/// with no --relay-servers, single-host deployments relay via the
/// server host itself (the client falls back to the rendezvous host,
/// port+1), so the host address belongs in the relay set. Empty when
/// both sources are empty/unresolvable — relay detection then
/// reports unknown instead of guessing.
fn relay_ips_union(
    relay_servers: &str,
    public_host: &str,
) -> std::collections::HashSet<std::net::IpAddr> {
    let mut set = resolve_relay_ips(relay_servers);
    let host = host_part(public_host);
    if !host.is_empty() {
        set.extend(resolve_relay_ips(host));
    }
    set
}

/// Cached relay IPs: (fetched_at, key, Some(set)) when the union
/// resolved to at least one IP, None when both sources are
/// empty/unresolvable (relay detection then reports unknown instead
/// of guessing). Refreshed at most every 5 minutes for unchanged
/// inputs; the key is (relay-servers, public_host), so an admin
/// changing public_host in the console takes effect on the next
/// request instead of waiting out the TTL. A re-pointed relay
/// domain takes up to 5 minutes to show up.
static RELAY_IPS_CACHE: std::sync::OnceLock<
    std::sync::Mutex<
        Option<(
            std::time::Instant,
            String,
            Option<std::collections::HashSet<std::net::IpAddr>>,
        )>,
    >,
> = std::sync::OnceLock::new();

async fn relay_ips_cached(pool: &SqlitePool) -> Option<std::collections::HashSet<std::net::IpAddr>> {
    let relay_servers = crate::common::get_arg("relay-servers");
    let public_host = get_setting(pool, "public_host").await;
    let key = format!("{relay_servers}\0{public_host}");
    let cache = RELAY_IPS_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = match cache.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some((at, k, ips)) = guard.as_ref() {
        if *k == key && at.elapsed() < std::time::Duration::from_secs(300) {
            return ips.clone();
        }
    }
    let set = relay_ips_union(&relay_servers, &public_host);
    let out = if set.is_empty() { None } else { Some(set) };
    *guard = Some((std::time::Instant::now(), key, out.clone()));
    out
}

/// Relay or direct, from the 'new' event's IP: for a relayed session
/// the controlled side's socket peer is hbbr, so the IP is a relay
/// address; for a direct session it is the controller's IP. None when
/// the IP is unparsable/empty or the relay list is undetermined — the
/// console shows '—' rather than guessing.
fn relayed_for_ip(
    ip: &str,
    relay_ips: &Option<std::collections::HashSet<std::net::IpAddr>>,
) -> Option<bool> {
    let ips = relay_ips.as_ref()?;
    let addr = norm_ip(ip)?;
    Some(ips.contains(&addr))
}

/// One assembled connection session: a (device_id, conn_id) group of
/// audit_conn events. conn_id is the correlator across new/auth/close
/// (session_id is empty on the "new" event, which is posted in
/// on_open before the login message arrives). Note: conn_id restarts
/// when the controlled client restarts, so two sessions far apart in
/// time could in theory share a key; the raw table keeps the truth.
struct AuditSessionRow {
    device_id: String,
    controlled_hostname: String,
    controlled_username: Option<String>,
    conn_id: i64,
    started_at: i64,
    ended_at: Option<i64>,
    /// How the session ended: "event" (client "close" event),
    /// "stale" (heartbeat reaper, issue #11), "offline" (device
    /// silent over the offline threshold), None while open.
    closed_by: Option<&'static str>,
    controller_ip: String,
    controller_id: String,
    controller_name: String,
    controller_username: Option<String>,
    relayed: Option<bool>,
    conn_type: String,
}

struct AuditSessionAcc {
    started_new: Option<i64>,
    first: i64,
    last: i64,
    ended: Option<i64>,
    ip: String,
    peer_id: String,
    peer_name: String,
    conn_type: String,
}

/// A session with no close and no stale record is dead once its
/// device has been silent for 24 h: idle devices heartbeat at least
/// every 15 s, so a day of silence means the device is gone (a
/// device that comes back online gets reaped by the heartbeat rule
/// on its first heartbeat instead). 24 h, not shorter, so a device
/// asleep overnight is never cut off by this rule alone — when its
/// heartbeats resume, a genuinely live session keeps its conn_id in
/// the live list and neither rule touches it.
const AUDIT_OFFLINE_SECS: i64 = 86400;

/// Assemble sessions from audit_conn rows (newest first, capped at
/// 20000 events so an unbounded table cannot blow up the console
/// request). `since` filters by server receipt time (None = all).
async fn audit_session_rows(
    pool: &SqlitePool,
    since: Option<i64>,
) -> Result<Vec<AuditSessionRow>, String> {
    let rows = sqlx::query(
        "select device_id, conn_id, action, ip, peer_id, peer_name, conn_type, received_at \
         from audit_conn where received_at >= ? order by received_at desc limit 20000",
    )
    .bind(since.unwrap_or(0))
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut groups: HashMap<(String, i64), AuditSessionAcc> = HashMap::new();
    for r in &rows {
        let device_id = r.try_get::<String, _>("device_id").unwrap_or_default();
        let conn_id = r.try_get::<i64, _>("conn_id").unwrap_or(0);
        let action = r.try_get::<String, _>("action").unwrap_or_default();
        let at = r.try_get::<i64, _>("received_at").unwrap_or(0);
        let acc = groups
            .entry((device_id, conn_id))
            .or_insert_with(|| AuditSessionAcc {
                started_new: None,
                first: at,
                last: at,
                ended: None,
                ip: String::new(),
                peer_id: String::new(),
                peer_name: String::new(),
                conn_type: String::new(),
            });
        if at < acc.first {
            acc.first = at;
        }
        if at > acc.last {
            acc.last = at;
        }
        match action.as_str() {
            "new" => {
                if acc.started_new.is_none_or(|s| at < s) {
                    acc.started_new = Some(at);
                    acc.ip = r.try_get::<String, _>("ip").unwrap_or_default();
                }
            }
            "close" => {
                if acc.ended.is_none_or(|e| at > e) {
                    acc.ended = Some(at);
                }
            }
            _ => {
                // Auth event (or a future action): peer info.
                if acc.peer_id.is_empty() {
                    acc.peer_id = r.try_get::<String, _>("peer_id").unwrap_or_default();
                    acc.peer_name = r.try_get::<String, _>("peer_name").unwrap_or_default();
                    acc.conn_type = r.try_get::<String, _>("conn_type").unwrap_or_default();
                }
            }
        }
    }
    // Device info for both ends (most recently seen row wins when a
    // device_id has several uuid rows, e.g. after a reinstall).
    // last_seen feeds the offline rule below.
    let dev_rows = sqlx::query(
        "select device_id, username, hostname, last_seen from devices order by last_seen desc",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut devs: HashMap<String, (Option<String>, String, i64)> = HashMap::new();
    for r in &dev_rows {
        let id = r.try_get::<String, _>("device_id").unwrap_or_default();
        if id.is_empty() || devs.contains_key(&id) {
            continue;
        }
        let username = r.try_get::<Option<String>, _>("username").ok().flatten();
        let hostname = r.try_get::<String, _>("hostname").unwrap_or_default();
        let last_seen = r.try_get::<i64, _>("last_seen").unwrap_or(0);
        devs.insert(id, (username, hostname, last_seen));
    }
    // Reaped (stale) session ends, recorded by the heartbeat reaper.
    let stale_rows = sqlx::query("select device_id, conn_id, ended_at from audit_session_close")
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let mut stale: HashMap<(String, i64), i64> = HashMap::new();
    for r in &stale_rows {
        let device_id = r.try_get::<String, _>("device_id").unwrap_or_default();
        let conn_id = r.try_get::<i64, _>("conn_id").unwrap_or(0);
        let ended_at = r.try_get::<i64, _>("ended_at").unwrap_or(0);
        stale.insert((device_id, conn_id), ended_at);
    }
    let relay_ips = relay_ips_cached(pool).await;
    let t = now();
    let mut out: Vec<AuditSessionRow> = groups
        .into_iter()
        .map(|((device_id, conn_id), acc)| {
            let started_at = acc.started_new.unwrap_or(acc.first);
            let (controlled_username, controlled_hostname, _) = devs
                .get(&device_id)
                .cloned()
                .unwrap_or((None, String::new(), 0));
            let controller_username = if acc.peer_id.is_empty() {
                None
            } else {
                devs.get(&acc.peer_id)
                    .and_then(|(u, _, _)| u.clone())
            };
            // How the session ended: a "close" event wins; else a
            // stale record — applied only if it postdates the
            // group's last event, because conn_id restarts when the
            // client restarts and a new session may reuse the id
            // after the old one was reaped; else the offline rule
            // (computed here, no writes).
            let mut ended_at = acc.ended;
            let mut closed_by: Option<&'static str> =
                if acc.ended.is_some() { Some("event") } else { None };
            if ended_at.is_none() {
                if let Some(&s) = stale.get(&(device_id.clone(), conn_id)) {
                    if s >= acc.last {
                        ended_at = Some(s);
                        closed_by = Some("stale");
                    }
                }
            }
            if ended_at.is_none() {
                if let Some((_, _, last_seen)) = devs.get(&device_id) {
                    if *last_seen > 0 && t - *last_seen > AUDIT_OFFLINE_SECS {
                        ended_at = Some((*last_seen).max(started_at));
                        closed_by = Some("offline");
                    }
                }
            }
            AuditSessionRow {
                relayed: relayed_for_ip(&acc.ip, &relay_ips),
                device_id,
                controlled_hostname,
                controlled_username,
                conn_id,
                started_at,
                ended_at,
                closed_by,
                controller_ip: acc.ip,
                controller_id: acc.peer_id,
                controller_name: acc.peer_name,
                controller_username,
                conn_type: acc.conn_type,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| a.device_id.cmp(&b.device_id))
            .then_with(|| a.conn_id.cmp(&b.conn_id))
    });
    Ok(out)
}

fn audit_session_json(r: &AuditSessionRow) -> Value {
    json!({
        "device_id": r.device_id,
        "controlled_hostname": r.controlled_hostname,
        "controlled_username": r.controlled_username,
        "conn_id": r.conn_id,
        "started_at": r.started_at,
        "ended_at": r.ended_at,
        "closed_by": r.closed_by,
        "duration_sec": r.ended_at.map(|e| e - r.started_at),
        "controller_ip": r.controller_ip,
        "controller_id": r.controller_id,
        "controller_name": r.controller_name,
        "controller_username": r.controller_username,
        "relayed": r.relayed,
        "conn_type": r.conn_type,
    })
}

async fn audit_conn_post(Extension(st): Extension<Arc<ApiState>>, body: String) -> Response {
    let v = parse_body(&body);
    if !v.is_object() {
        return err_resp(StatusCode::BAD_REQUEST, "bad json");
    }
    let device_id = jstr(&v, "id");
    let uuid = sfield(&v, "uuid");
    let nonce = sfield(&v, "nonce");
    if device_id.is_empty() || nonce.is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "missing id or nonce");
    }
    // Authenticate by the heartbeat-bound (id, uuid) pair: the audit
    // uuid is the same base64(hbb_common::get_uuid()) the heartbeat
    // sends, so the devices table matches it directly. A device whose
    // first heartbeat has not landed yet has its first session's
    // records dropped (403) — accepted, documented in DEPLOY.md.
    let known = sqlx::query("select 1 as c from devices where device_id=? and uuid=? limit 1")
        .bind(&device_id)
        .bind(&uuid)
        .fetch_optional(&st.pool)
        .await;
    match known {
        Ok(Some(_)) => {}
        Ok(None) => {
            return err_resp(StatusCode::FORBIDDEN, "unknown device or uuid mismatch");
        }
        Err(e) => return client_err(&e.to_string()),
    }
    let action_raw = sfield(&v, "action");
    let peer = v.get("peer");
    let action = if !action_raw.is_empty() {
        action_raw
    } else if peer.is_some() {
        "auth".to_string()
    } else {
        return err_resp(StatusCode::BAD_REQUEST, "missing action");
    };
    let (peer_id, peer_name) = match peer {
        Some(Value::Array(a)) => (
            a.first().map(jstr_val).unwrap_or_default(),
            a.get(1).map(jstr_val).unwrap_or_default(),
        ),
        _ => (String::new(), String::new()),
    };
    let res = sqlx::query(
        "insert or ignore into audit_conn(nonce, device_id, device_uuid, conn_id, session_id, action, ip, peer_id, peer_name, conn_type, primary_auth, two_factor, conn_audit_ref, raw, received_at) values(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&nonce)
    .bind(&device_id)
    .bind(&uuid)
    .bind(v.get("conn_id").and_then(|x| x.as_i64()).unwrap_or(0))
    .bind(jstr(&v, "session_id"))
    .bind(&action)
    .bind(jstr(&v, "ip"))
    .bind(&peer_id)
    .bind(&peer_name)
    .bind(v.get("type").map(jstr_val).unwrap_or_default())
    .bind(v.get("primary_auth").and_then(|x| x.as_i64()))
    .bind(v.get("two_factor").and_then(|x| x.as_i64()))
    .bind(jstr(&v, "conn_audit_ref"))
    .bind(body.clone())
    .bind(now())
    .execute(&st.pool)
    .await;
    match res {
        // Inserted or duplicate nonce (insert-or-ignore): both are
        // success, and the body MUST stay empty per the client
        // contract — any 2xx body is parsed as an error.
        Ok(_) => (StatusCode::OK, "").into_response(),
        Err(e) => client_err(&e.to_string()),
    }
}

async fn audit_sessions(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let days: i64 = q
        .get("days")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let days = if days < 0 { 30 } else { days };
    let page: i64 = q
        .get("page")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let per_page: i64 = q
        .get("per_page")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .clamp(1, 100);
    let needle = q
        .get("q")
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_default();
    let since = if days == 0 {
        None
    } else {
        Some(now() - days * 86400)
    };
    let rows = match audit_session_rows(&st.pool, since).await {
        Ok(r) => r,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    let rows: Vec<AuditSessionRow> = rows
        .into_iter()
        .filter(|r| {
            if !u.is_admin {
                let ctrl = r.controller_username.as_deref() == Some(u.username.as_str());
                let cont = r.controlled_username.as_deref() == Some(u.username.as_str());
                if !ctrl && !cont {
                    return false;
                }
            }
            if !needle.is_empty() {
                let hay = format!(
                    "{} {} {} {} {} {} {}",
                    r.device_id,
                    r.controlled_hostname,
                    r.controller_id,
                    r.controller_name,
                    r.controller_ip,
                    r.controller_username.as_deref().unwrap_or(""),
                    r.controlled_username.as_deref().unwrap_or("")
                )
                .to_lowercase();
                if !hay.contains(&needle) {
                    return false;
                }
            }
            true
        })
        .collect();
    let total = rows.len();
    let start = (page.saturating_sub(1).saturating_mul(per_page)) as usize;
    let sessions: Vec<Value> = rows
        .iter()
        .skip(start)
        .take(per_page as usize)
        .map(audit_session_json)
        .collect();
    jresp(
        StatusCode::OK,
        json!({"ok": true, "sessions": sessions, "total": total, "page": page, "per_page": per_page}),
    )
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
        Ok(res) => jresp(
            StatusCode::OK,
            json!({ "ok": true, "token": token, "id": res.last_insert_rowid() }),
        ),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn tokens_rename(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: String,
) -> Response {
    let u = match resolve_user(&st, &headers).await {
        Some(u) => u,
        None => return unauthorized(),
    };
    let v = parse_body(&body);
    let name = sfield(&v, "name");
    let name = if name.is_empty() { "default" } else { &name };
    match sqlx::query("update api_tokens set name=? where id=? and username=?")
        .bind(name)
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
                "must_login": get_setting(&st.pool, "must_login").await == "1",
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
    // No strategy_ts bump: must-login is enforced server-side at
    // punch-hole time and read live from the settings table, not
    // pushed to clients via heartbeat.
    let must_login = v
        .get("must_login")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    if let Err(e) = set_setting(
        &st.pool,
        "must_login",
        if must_login { "1" } else { "0" },
    )
    .await
    {
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

/// GitHub acceleration prefixes in try order: the admin's custom
/// mirror (if configured) first, then the built-in mirrors. The
/// template download and the one-click update share this list and the
/// same `github_mirror` setting; there is no second mirror setting.
/// Every prefix ends with '/', so callers can append a full URL
/// directly (`format!("{prefix}{url}")`).
fn mirror_prefixes(custom: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !custom.is_empty() {
        out.push(format!("{custom}/"));
    }
    for m in GH_MIRRORS {
        out.push(m.to_string());
    }
    out
}

/// Ordered download candidates for one GitHub URL: custom mirror,
/// built-in mirrors, then direct. Each entry is (label, url).
fn github_candidates(custom: &str, direct: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !custom.is_empty() {
        out.push((
            format!("自定义镜像 {custom}"),
            format!("{custom}/{direct}"),
        ));
    }
    for m in GH_MIRRORS {
        out.push((m.to_string(), format!("{m}{direct}")));
    }
    out.push(("直连 GitHub".to_string(), direct.to_string()));
    out
}

/// The admin-configured GitHub mirror prefix (trimmed, no trailing
/// '/'), read from settings. The background download threads have no
/// async runtime, so this builds a throwaway one like the template job.
fn custom_mirror(st: &ApiState) -> String {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let raw = match rt {
        Ok(rt) => rt.block_on(get_setting(&st.pool, "github_mirror")),
        Err(_) => String::new(),
    };
    raw.trim().trim_end_matches('/').to_string()
}

/// GET a small JSON document with the built-in downloader (one attempt).
fn fetch_json(url: &str, timeout_secs: u64) -> Result<Value, String> {
    let bytes = fetch_bytes(url, timeout_secs)?;
    let text = String::from_utf8_lossy(&bytes);
    serde_json::from_str(&text).map_err(|e| format!("解析响应失败：{e}"))
}

fn tpl_part_path(st: &ApiState) -> PathBuf {
    st.templates_dir.join("rustdesk-windows.exe.part")
}

fn tpl_final_path(st: &ApiState) -> PathBuf {
    st.templates_dir.join("rustdesk-windows.exe")
}

/// Fetch the latest official client release document from the GitHub API
/// (small JSON). Mirrors are tried before a direct connection, because
/// direct GitHub API access is the part most likely to be slow or blocked
/// on networks that need acceleration; direct is the last resort. Shared
/// by the template download and the other-platform download links. On
/// failure returns the last transport error; callers add their own
/// context (the template page and the downloads endpoint word the
/// failure differently).
fn fetch_latest_release_json(custom: &str) -> Result<Value, String> {
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
                return serde_json::from_str(&text).map_err(|e| e.to_string());
            }
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// Resolve the latest official client version and the installer size from
/// the GitHub API (small JSON).
fn resolve_latest(custom: &str) -> Result<(String, u64), String> {
    let v = fetch_latest_release_json(custom).map_err(|e| {
        format!("无法获取官方版本信息（GitHub API 不可达）：{e}。可在上方手动填写版本号后重试")
    })?;
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
    Ok((version, size))
}

/// One official client download for another platform. The filename and
/// URL come from the release's asset list, never guessed: a renamed or
/// missing asset simply drops its platform out of the list.
#[derive(Clone, Debug, PartialEq)]
struct ClientDownload {
    id: &'static str,
    name: &'static str,
    file: String,
    url: String,
}

/// The App Store page for the official iOS client, taken from the
/// official 1.5.3 release notes (verified 2026-11-13). iOS has no GitHub
/// install package, so this link is version-independent and is appended
/// without mirrors.
const IOS_APP_STORE_URL: &str =
    "https://apps.apple.com/us/app/rustdesk-remote-desktop/id1581225015";

/// (id, display name, filename suffix) for the other-platform downloads,
/// in display order. Suffixes are matched against the release's actual
/// asset names with a `rustdesk-{version}-` prefix (verified against the
/// real 1.5.3 asset list). Windows is deliberately absent: generated
/// clients are the Windows path.
const CLIENT_DOWNLOAD_SUFFIXES: &[(&str, &str, &str)] = &[
    ("macos-x64", "macOS（Intel）", "x86_64.dmg"),
    ("macos-arm64", "macOS（Apple 芯片）", "aarch64.dmg"),
    ("linux-deb-x64", "Linux x86_64（.deb）", "x86_64.deb"),
    ("linux-deb-arm64", "Linux ARM64（.deb）", "aarch64.deb"),
    ("linux-appimage-x64", "Linux x86_64（AppImage）", "x86_64.AppImage"),
    ("linux-appimage-arm64", "Linux ARM64（AppImage）", "aarch64.AppImage"),
    ("android-arm64", "Android（arm64）", "aarch64-signed.apk"),
    ("android-x64", "Android（x86_64）", "x86_64-signed.apk"),
    ("android-armv7", "Android（armv7）", "armv7-signed.apk"),
    ("android-universal", "Android（通用）", "universal-signed.apk"),
];

/// Pick the other-platform downloads out of a releases/latest JSON
/// document. Returns (version, downloads). Assets that do not match any
/// known suffix are ignored; a release with no matching assets yields an
/// empty list. The download URL is the asset's own browser_download_url.
fn parse_client_downloads(v: &Value) -> Result<(String, Vec<ClientDownload>), String> {
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
    let version = tag.trim_start_matches('v').to_string();
    if version.is_empty() {
        return Err("无法解析官方最新版本".to_string());
    }
    let prefix = format!("rustdesk-{version}-");
    let assets = v.get("assets").and_then(|a| a.as_array());
    let mut out: Vec<ClientDownload> = Vec::new();
    // Iterate the table in display order so the list is stable.
    for (id, name, suffix) in CLIENT_DOWNLOAD_SUFFIXES {
        let file = format!("{prefix}{suffix}");
        let mut url = String::new();
        if let Some(assets) = assets {
            for a in assets {
                if a.get("name").and_then(|n| n.as_str()) == Some(file.as_str()) {
                    url = a
                        .get("browser_download_url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    break;
                }
            }
        }
        if url.is_empty() {
            continue; // asset absent from this release: skip the platform
        }
        out.push(ClientDownload {
            id,
            name,
            file,
            url,
        });
    }
    Ok((version, out))
}

/// How long the latest-release data for /api/client-downloads stays
/// cached (one hour; the unauthenticated GitHub API limit is 60
/// requests per hour per shared IP).
const CLIENT_DOWNLOADS_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

fn client_downloads_cache_fresh(fetched_at: std::time::Instant) -> bool {
    fetched_at.elapsed() < CLIENT_DOWNLOADS_TTL
}

/// (version, downloads) for the other-platform links: from the cache
/// when fresh, else fetched from the GitHub API (mirrors first, direct
/// last — same chain as the template download). Runs on a blocking
/// thread: fetch_bytes builds its own runtime.
fn refresh_client_downloads(
    st: &ApiState,
    custom: &str,
) -> Result<(String, Vec<ClientDownload>), String> {
    if let Ok(guard) = st.dl_cache.lock() {
        if let Some((at, version, downloads)) = guard.as_ref() {
            if client_downloads_cache_fresh(*at) {
                return Ok((version.clone(), downloads.clone()));
            }
        }
    }
    let v = fetch_latest_release_json(custom)
        .map_err(|e| format!("无法获取官方客户端版本信息：{e}"))?;
    let (version, downloads) = parse_client_downloads(&v)?;
    if let Ok(mut guard) = st.dl_cache.lock() {
        *guard = Some((
            std::time::Instant::now(),
            version.clone(),
            downloads.clone(),
        ));
    }
    Ok((version, downloads))
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
/// endpoint, and the winning source is recorded in the job state. Skips
/// the download when the local template already matches the resolved
/// version (phase "up-to-date").
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
    let custom = custom_mirror(&st);
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
    // Skip the download when the local template already matches the
    // resolved version (previously every click re-downloaded). A
    // missing tpl_version (e.g. a manually placed template) or a
    // missing file falls through to the download below.
    if dest.exists() {
        let local = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(get_setting(&st.pool, "tpl_version")),
            Err(_) => String::new(),
        };
        if !local.is_empty() && local.trim().trim_start_matches('v') == version.as_str() {
            set_job(&st, |j| {
                j.running = false;
                j.phase = "up-to-date".into();
                j.error = String::new();
            });
            return;
        }
    }
    let direct =
        format!("https://github.com/rustdesk/rustdesk/releases/download/{version}/rustdesk-{version}-x86_64.exe");
    let candidates = github_candidates(&custom, &direct);
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

// ------------------------- one-click server update -------------------------
// The console can check our GitHub Releases for a newer fork version,
// download the matching release zip, and stage the new hbbs/hbbr
// binaries as `<exe>.new` next to the running ones. The staged files
// are swapped in by the launcher (rdsvc / start.sh / start.ps1 /
// systemd) on the next start — never by hbbs itself, because on
// Windows a running exe is locked and cannot replace itself.
// Downloads reuse the template pipeline's mirror chain and the same
// `github_mirror` setting. The version parameter is strictly
// validated (`^v[0-9]+$`) and only ever addresses our own repo's
// releases; no caller-supplied URL is fetched.

const UPDATE_REPO_API: &str = "https://api.github.com/repos/wirh/rustdesk-server-pro";
const UPDATE_RELEASES_PAGE: &str =
    "https://github.com/wirh/rustdesk-server-pro/releases/latest";
const UPDATE_ASSET_PREFIX: &str =
    "https://github.com/wirh/rustdesk-server-pro/releases/download/";

/// "v32" (or a bare "32") -> Some(32); anything else -> None.
fn parse_pro_tag(tag: &str) -> Option<u64> {
    let t = tag.trim().trim_start_matches('v');
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse::<u64>().ok()
}

fn current_pro_num() -> u64 {
    parse_pro_tag(PRO_VERSION).unwrap_or(0)
}

/// Strict `^v[0-9]+$` check for the download endpoint's version
/// parameter (and for tags extracted from redirect URLs).
fn valid_version_param(v: &str) -> bool {
    let t = v.trim();
    t.len() > 1 && t.starts_with('v') && t[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Release-package platform for (OS, arch, musl). Kept as a pure
/// function of its inputs so every platform maps in unit tests; the
/// live value comes from `this_platform` below.
fn package_for_platform(os: &str, arch: &str, musl: bool) -> Option<&'static str> {
    match (os, arch, musl) {
        ("windows", "x86_64", _) => Some("windows-x86_64"),
        ("linux", "x86_64", _) => Some("linux-x86_64-glibc"),
        ("linux", "aarch64", false) => Some("linux-aarch64-glibc"),
        ("linux", "aarch64", true) => Some("linux-aarch64-musl"),
        _ => None,
    }
}

fn this_platform() -> Option<&'static str> {
    package_for_platform(
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_env = "musl"),
    )
}

fn update_asset_name(version: &str, platform: &str) -> String {
    format!("rustdesk-server-pro-{platform}-{version}.zip")
}

/// GET a GitHub API JSON document: direct first, then each mirror in
/// order. A failed attempt falls through to the next source; only the
/// last error is reported.
fn fetch_github_api(custom: &str, api_url: &str) -> Result<Value, String> {
    match fetch_json(api_url, 15) {
        Ok(v) => return Ok(v),
        Err(e) => {
            let mut last_err = e;
            for m in mirror_prefixes(custom) {
                match fetch_json(&format!("{m}{api_url}"), 15) {
                    Ok(v) => return Ok(v),
                    Err(e) => last_err = e,
                }
            }
            Err(last_err)
        }
    }
}

/// What an update check found. `notes_unavailable` means the version
/// came from the redirect fallback, which has no release body.
struct CheckOutcome {
    tag: String,
    body: String,
    published_at: String,
    notes_unavailable: bool,
}

fn parse_release_json(v: &Value) -> Result<CheckOutcome, String> {
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if tag.is_empty() {
        return Err("GitHub 返回的发布信息里没有版本号".to_string());
    }
    Ok(CheckOutcome {
        tag,
        body: v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .to_string(),
        published_at: v
            .get("published_at")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string(),
        notes_unavailable: false,
    })
}

/// Extract a vN tag from a release URL: ".../releases/tag/v31", or a
/// trailing "v31" segment (query strings and fragments ignored).
fn tag_from_url(url: &str) -> Option<String> {
    let url = url.trim_end_matches('/');
    if let Some(idx) = url.find("/tag/") {
        let seg = url[idx + 5..].split(['/', '?', '#']).next().unwrap_or("");
        if valid_version_param(seg) {
            return Some(seg.to_string());
        }
    }
    let last = url
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if valid_version_param(last) {
        return Some(last.to_string());
    }
    None
}

/// Read the version tag from the /releases/latest redirect without
/// following it in the HTTP client (a second hop to github.com would
/// bypass the mirror that just worked). The Location header carries
/// ".../releases/tag/v31". If a mirror already followed the redirect
/// and returned 200, the final URL is tried instead.
fn fetch_redirect_tag(url: &str) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("初始化下载组件失败：{e}"))?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent("hbbs-api")
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("初始化下载组件失败：{e}"))?;
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("请求失败：{e}"))?;
        let status = resp.status();
        if status.is_redirection() {
            match resp
                .headers()
                .get(header::LOCATION)
                .and_then(|h| h.to_str().ok())
            {
                Some(loc) => tag_from_url(loc)
                    .ok_or_else(|| format!("重定向地址里没有版本号：{loc}")),
                None => Err("重定向响应缺少 Location".to_string()),
            }
        } else if status.is_success() {
            tag_from_url(resp.url().as_str())
                .ok_or_else(|| "响应地址里没有版本号".to_string())
        } else {
            Err(format!("请求失败：HTTP {status}"))
        }
    })
}

/// Check our repo's latest release, with acceleration retries: direct
/// GitHub API first, then the API through each mirror, then the
/// /releases/latest page (redirect tag only, no release notes) through
/// each mirror and direct. Only when every path fails is the check a
/// failure.
fn check_latest_release(custom: &str) -> Result<CheckOutcome, String> {
    let api_url = format!("{UPDATE_REPO_API}/releases/latest");
    if let Ok(v) = fetch_github_api(custom, &api_url) {
        return parse_release_json(&v);
    }
    let page = UPDATE_RELEASES_PAGE;
    let mut page_urls: Vec<String> = mirror_prefixes(custom)
        .into_iter()
        .map(|m| format!("{m}{page}"))
        .collect();
    page_urls.push(page.to_string());
    for u in page_urls {
        if let Ok(tag) = fetch_redirect_tag(&u) {
            return Ok(CheckOutcome {
                tag,
                body: String::new(),
                published_at: String::new(),
                notes_unavailable: true,
            });
        }
    }
    Err("检测更新失败：已尝试直连与 GitHub 加速镜像，均无法获取更新信息，请稍后重试".to_string())
}

fn exe_file_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn upd_part_path(st: &ApiState) -> PathBuf {
    st.exe_dir.join("update").join("update.zip.part")
}

fn staged_hbbs(exe_dir: &std::path::Path) -> PathBuf {
    exe_dir.join(format!("{}.new", exe_file_name("hbbs")))
}

fn staged_hbbr(exe_dir: &std::path::Path) -> PathBuf {
    exe_dir.join(format!("{}.new", exe_file_name("hbbr")))
}

fn staged_meta(exe_dir: &std::path::Path) -> PathBuf {
    exe_dir.join("update").join("staged.json")
}

/// The staged version, if a complete staged update is present (both
/// `.new` binaries plus the marker file).
fn staged_version(exe_dir: &std::path::Path) -> Option<String> {
    if !staged_hbbs(exe_dir).exists() || !staged_hbbr(exe_dir).exists() {
        return None;
    }
    let text = std::fs::read_to_string(staged_meta(exe_dir)).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn file_starts_with_pk(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 4];
    match std::fs::File::open(path) {
        Ok(mut f) => f.read_exact(&mut buf).is_ok() && &buf == b"PK\x03\x04",
        Err(_) => false,
    }
}

/// Extract hbbs + hbbr from a release zip into `<exe>.new` staging
/// files. The zip has one top folder whose name varies, so entries are
/// matched by file name. Each binary is written to a `.tmp` file and
/// renamed into place; a partial extraction cleans up after itself.
fn extract_update(zip_path: &std::path::Path, exe_dir: &std::path::Path) -> Result<(), String> {
    if !file_starts_with_pk(zip_path) {
        return Err("更新包不是有效的 ZIP 文件".to_string());
    }
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开更新包失败：{e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("读取更新包失败：{e}"))?;
    let want = [exe_file_name("hbbs"), exe_file_name("hbbr")];
    let mut found = [false, false];
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取更新包失败：{e}"))?;
        let name = entry.name().to_string();
        let base = name.rsplit('/').next().unwrap_or("");
        for (k, w) in want.iter().enumerate() {
            if found[k] || base != w {
                continue;
            }
            let tmp = exe_dir.join(format!("{w}.new.tmp"));
            {
                let mut out =
                    std::fs::File::create(&tmp).map_err(|e| format!("写入暂存文件失败：{e}"))?;
                std::io::copy(&mut entry, &mut out).map_err(|e| format!("解压 {w} 失败：{e}"))?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &tmp,
                    std::fs::Permissions::from_mode(0o755),
                );
            }
            let dest = exe_dir.join(format!("{w}.new"));
            std::fs::rename(&tmp, &dest).map_err(|e| format!("暂存 {w} 失败：{e}"))?;
            found[k] = true;
        }
    }
    if !found.iter().all(|f| *f) {
        for w in want {
            let _ = std::fs::remove_file(exe_dir.join(format!("{w}.new")));
            let _ = std::fs::remove_file(exe_dir.join(format!("{w}.new.tmp")));
        }
        return Err("更新包里没有找到 hbbs / hbbr".to_string());
    }
    Ok(())
}

fn set_upd_job(st: &ApiState, f: impl FnOnce(&mut UpdJob)) {
    if let Ok(mut j) = st.upd_job.lock() {
        f(&mut j);
    }
}

fn cleanup_staged(exe_dir: &std::path::Path) {
    for w in [exe_file_name("hbbs"), exe_file_name("hbbr")] {
        let _ = std::fs::remove_file(exe_dir.join(format!("{w}.new")));
        let _ = std::fs::remove_file(exe_dir.join(format!("{w}.new.tmp")));
    }
    let _ = std::fs::remove_file(staged_meta(exe_dir));
}

/// Background update download with GitHub mirror failover. Runs on a
/// plain thread; progress is read from the .part file size by the
/// status endpoint. On any failure the partial download and any
/// staged binaries are removed, so a failed update never leaves a
/// half-applied state behind.
fn run_update_job(st: Arc<ApiState>, version: String) {
    let exe_dir = st.exe_dir.clone();
    let finish_err = |st: &ApiState, msg: String| {
        set_upd_job(st, |j| {
            j.running = false;
            j.phase = "error".into();
            j.error = msg;
        });
    };
    let platform = match this_platform() {
        Some(p) => p,
        None => {
            finish_err(&st, "当前平台不支持在线更新".to_string());
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(exe_dir.join("update")) {
        finish_err(&st, e.to_string());
        return;
    }
    let custom = custom_mirror(&st);
    let api_url = format!("{UPDATE_REPO_API}/releases/tags/{version}");
    let asset_name = update_asset_name(&version, platform);
    // Resolve the asset URL from the release metadata. If the API
    // lookup fails (rate-limited, blocked, or the release is gone),
    // fall back to the deterministic download URL instead of failing
    // outright: the asset is still fetched through the mirror chain,
    // and a release that does not exist 404s on every source and ends
    // in the error phase. The fallback URL is built from the validated
    // version and the compile-time platform only — never from caller
    // input — so the no-arbitrary-URL guarantee still holds.
    let (asset_url, total) = match fetch_github_api(&custom, &api_url) {
        Ok(release) => {
            let mut url = String::new();
            let mut size = 0u64;
            if let Some(assets) = release.get("assets").and_then(|a| a.as_array()) {
                for a in assets {
                    if a.get("name").and_then(|n| n.as_str())
                        == Some(asset_name.as_str())
                    {
                        url = a
                            .get("browser_download_url")
                            .and_then(|u| u.as_str())
                            .unwrap_or("")
                            .to_string();
                        size = a.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                    }
                }
            }
            if url.is_empty() || !url.starts_with(UPDATE_ASSET_PREFIX) {
                finish_err(&st, format!("{version} 没有 {platform} 平台的安装包"));
                return;
            }
            (url, size)
        }
        Err(_) => (
            format!("{UPDATE_ASSET_PREFIX}{version}/{asset_name}"),
            0,
        ),
    };
    set_upd_job(&st, |j| {
        j.version = version.clone();
        j.total = total;
    });
    let part = upd_part_path(&st);
    let mut last_err = String::new();
    let mut ok = false;
    for (label, url) in github_candidates(&custom, &asset_url) {
        set_upd_job(&st, |j| j.source = label.clone());
        let _ = std::fs::remove_file(&part);
        match download_to_file(&url, &part, 1800) {
            Ok(()) => {
                let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                let size_ok = len >= 1_000_000 && (total == 0 || len == total);
                if size_ok && file_starts_with_pk(&part) {
                    ok = true;
                    break;
                }
                last_err =
                    format!("{label} 返回的文件不完整或不是有效的更新包（{len} 字节）");
            }
            Err(e) => last_err = format!("{label}：{e}"),
        }
    }
    if !ok {
        let _ = std::fs::remove_file(&part);
        finish_err(
            &st,
            format!("{version} 的更新包下载失败，所有下载源都失败了。最后错误：{last_err}"),
        );
        return;
    }
    // On the deterministic-URL fallback path the package size is
    // unknown until the download finishes; report the real size so
    // the status endpoint is accurate on both paths.
    if total == 0 {
        let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        set_upd_job(&st, |j| j.total = len);
    }
    set_upd_job(&st, |j| j.phase = "extracting".into());
    match extract_update(&part, &exe_dir) {
        Ok(()) => {
            let _ = std::fs::remove_file(&part);
            // `version` passed the strict ^v[0-9]+$ check, so it needs
            // no JSON escaping here.
            let _ = std::fs::write(
                staged_meta(&exe_dir),
                format!("{{\"version\":\"{version}\"}}"),
            );
            set_upd_job(&st, |j| {
                j.running = false;
                j.phase = "done".into();
                j.error = String::new();
            });
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            cleanup_staged(&exe_dir);
            finish_err(&st, e);
        }
    }
}

async fn update_status(
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
    let (running, phase, version, total, source, error) = match st.upd_job.lock() {
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
    let downloaded = std::fs::metadata(upd_part_path(&st))
        .map(|m| m.len())
        .unwrap_or(0);
    let staged = staged_version(&st.exe_dir);
    jresp(
        StatusCode::OK,
        json!({
            "ok": true,
            "current": format!("v{}", PRO_VERSION),
            "current_num": current_pro_num(),
            "upstream_base": crate::version::VERSION,
            "platform": this_platform().unwrap_or("unknown"),
            "staged": staged.is_some(),
            "staged_version": staged,
            "state": if phase.is_empty() { "idle".to_string() } else { phase },
            "running": running,
            "version": version,
            "total": total,
            "downloaded": downloaded,
            "source": source,
            "error": error,
        }),
    )
}

async fn update_check(
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
    // Read the mirror setting with the handler's own async runtime —
    // custom_mirror() builds a throwaway runtime and must only run on
    // plain (non-tokio) threads like the download jobs.
    let custom = get_setting(&st.pool, "github_mirror")
        .await
        .trim()
        .trim_end_matches('/')
        .to_string();
    // The check can take a while when every source is slow (direct
    // API, then each mirror, then the redirect fallback), so it runs
    // on a blocking thread instead of an API worker.
    let res = tokio::task::spawn_blocking(move || check_latest_release(&custom)).await;
    match res {
        Ok(Ok(out)) => {
            let has_update = parse_pro_tag(&out.tag)
                .map(|n| n > current_pro_num())
                .unwrap_or(false);
            jresp(
                StatusCode::OK,
                json!({
                    "ok": true,
                    "latest": out.tag,
                    "has_update": has_update,
                    "release_notes": out.body,
                    "notes_unavailable": out.notes_unavailable,
                    "published_at": out.published_at,
                }),
            )
        }
        Ok(Err(e)) => err_resp(StatusCode::BAD_GATEWAY, &e),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn update_download(
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
    let version = sfield(&v, "version");
    if !valid_version_param(&version) {
        return err_resp(StatusCode::BAD_REQUEST, "版本号格式不正确（应为 v32 这样的格式）");
    }
    if this_platform().is_none() {
        return err_resp(StatusCode::BAD_REQUEST, "当前平台不支持在线更新");
    }
    let already = {
        match st.upd_job.lock() {
            Ok(mut j) => {
                if j.running {
                    true
                } else {
                    *j = UpdJob {
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
        std::thread::spawn(move || run_update_job(st2, version));
    }
    jresp(StatusCode::OK, json!({ "ok": true, "started": !already }))
}

async fn update_restart(
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
    if staged_version(&st.exe_dir).is_none() {
        return err_resp(StatusCode::BAD_REQUEST, "没有已暂存的更新，无法重启生效");
    }
    // Answer first, then exit: the launcher (rdsvc / start.sh /
    // start.ps1 / systemd) swaps the staged binaries in and starts the
    // new version. A directly-run hbbs stays down until started again
    // by hand — the console says so.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::exit(0);
    });
    jresp(StatusCode::OK, json!({ "ok": true }))
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

/// Maximum length (bytes) of the encoded `rustdesk--<payload>.exe`
/// filename before falling back to the plain `rustdesk-host=...` form.
/// Leaves headroom under both the 255-byte per-component limit and
/// Windows' MAX_PATH for the server directory prefix.
const ENCODED_FILENAME_LIMIT: usize = 200;
/// Hard per-component filename limit on Linux (NAME_MAX) and NTFS.
const FILENAME_LIMIT: usize = 255;

/// Whether the api/relay values must be embedded in a generated
/// client's filename, or the official client can derive them when
/// absent (verified against client 1.5.0 `src/custom_server.rs`,
/// `src/common.rs` `get_api_server_` and `rendezvous_mediator.rs`
/// `get_relay_server`): all four `CustomServer` fields carry
/// `#[serde(default)]`, so omitted fields parse as empty strings;
/// an empty api is derived as rendezvous port - 2, and an empty
/// relay falls back to the address hbbs hands out (else rendezvous
/// port + 1). So api is embedded unless it is exactly the
/// plain-http default AND the API port lines up with the
/// rendezvous port the client derives from (a custom override,
/// TLS, or a non-standard api-listen port each force embedding),
/// and relay is embedded unless its port is rendezvous port + 1.
fn needs_embed(
    api: &str,
    default_api: &str,
    api_port: u16,
    relay_port: u16,
    rendezvous_port: u16,
) -> (bool, bool) {
    (
        api != default_api || api_port != rendezvous_port.saturating_sub(2),
        relay_port != rendezvous_port.saturating_add(1),
    )
}

/// The whole server config as one share string, for the console's
/// "one-click copy" button: the user pastes it into the official
/// client's Settings -> Network import box (the paste icon), and the
/// client fills all four fields at once. This is exactly the format
/// the official client itself exports — `ServerConfig.encode()` in
/// flutter/lib/common.dart (verified identical in 1.5.0 and 1.5.3):
/// reversed base64url (no padding) of the JSON, with the keys in the
/// client's own order (host, relay, api, key). It is also the same
/// payload that sits in a generated client's filename and in the
/// paid Pro console's "Config" field — only the carrier differs.
/// Unlike `generated_filename`, all four fields are always present
/// (never omitted as derivable), so the string matches the
/// manual-config card value for value.
fn encode_server_config(host: &str, relay: &str, api: &str, key: &str) -> String {
    // Build the JSON by hand (per-field json!() for correct escaping)
    // so the key order matches the client's export byte for byte;
    // serde_json::Map would sort the keys alphabetically instead.
    let raw = format!(
        "{{\"host\":{},\"relay\":{},\"api\":{},\"key\":{}}}",
        json!(host),
        json!(relay),
        json!(api),
        json!(key)
    );
    let b64 = base64::encode_config(raw.as_bytes(), base64::URL_SAFE_NO_PAD);
    b64.chars().rev().collect()
}

/// Filename for a generated client: the server config is encoded into
/// the name (reversed base64url of the JSON), which is how the
/// official client reads its custom server. `api`/`relay` are
/// `None` when the client can derive them (see `needs_embed`) — they
/// are left out of the JSON entirely, not written as empty strings,
/// so a long public address stays under filesystem limits. When the
/// encoded name would still exceed `ENCODED_FILENAME_LIMIT`, the
/// much shorter plain form is used instead (the client parses its
/// `host=`/`key=`/`relay=` parts and derives a missing api the same
/// way); the plain form is only a fallback because the key is
/// visible in it. Returns a plain-language error when even the plain
/// form exceeds the filesystem limit, instead of leaking the raw OS
/// error to the console.
fn generated_filename(
    host: &str,
    key: &str,
    api: Option<&str>,
    relay: Option<&str>,
) -> Result<String, String> {
    let mut cfg = serde_json::Map::new();
    cfg.insert("key".to_string(), json!(key));
    cfg.insert("host".to_string(), json!(host));
    if let Some(api) = api {
        cfg.insert("api".to_string(), json!(api));
    }
    if let Some(relay) = relay {
        cfg.insert("relay".to_string(), json!(relay));
    }
    let raw = serde_json::to_string(&cfg).unwrap_or_default();
    let b64 = base64::encode_config(raw.as_bytes(), base64::URL_SAFE_NO_PAD);
    let payload: String = b64.chars().rev().collect();
    let encoded = format!("rustdesk--{payload}.exe");
    if !payload.is_empty() && !payload.contains("--") && encoded.len() <= ENCODED_FILENAME_LIMIT {
        return Ok(encoded);
    }
    let plain = match relay {
        Some(relay) => format!("rustdesk-host={host},key={key},relay={relay}.exe"),
        None => format!("rustdesk-host={host},key={key}.exe"),
    };
    if plain.len() > FILENAME_LIMIT {
        return Err("公网地址太长，生成的客户端文件名超出系统限制，请使用更短的公网地址".to_string());
    }
    Ok(plain)
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
/// API URL, relay host, key, plain-http default API URL) — exactly
/// as embedded into a generated client. Shared by client generation
/// and the console's client-config display, so values an admin
/// copies into a stock client by hand can never drift from what a
/// generated client would have used. The fifth element is the
/// default API URL before any override/TLS rewrite; generation uses
/// it to decide whether the api field can be left out of the encoded
/// filename (see `needs_embed`).
async fn client_config(st: &ApiState) -> Result<(String, String, String, String, String), String> {
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
        default_api.clone()
    };
    Ok((host, api, relay, key, default_api))
}

async fn do_generate(
    st: &ApiState,
    created_by: &str,
    label: &str,
) -> Result<(String, String), String> {
    let (host, api, relay, key, default_api) = client_config(st).await?;
    let tpl = st.templates_dir.join("rustdesk-windows.exe");
    if !tpl.exists() {
        return Err("请先在「服务器设置」里下载官方客户端模板".into());
    }
    let (embed_api, embed_relay) = needs_embed(
        &api,
        &default_api,
        api_port(&crate::common::get_arg("api-listen")),
        relay_port(&crate::common::get_arg("relay-servers")),
        rendezvous_port(&crate::common::get_arg("port")),
    );
    let filename = generated_filename(
        &host,
        &key,
        embed_api.then_some(api.as_str()),
        embed_relay.then_some(relay.as_str()),
    )?;
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
        Ok((host, api, relay, key, _)) => {
            let share = encode_server_config(&host, &relay, &api, &key);
            jresp(
                StatusCode::OK,
                json!({ "ok": true, "configured": true, "host": host, "api": api, "relay": relay, "key": key, "share": share }),
            )
        }
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// Other-platform official client downloads, for the generate-client
/// page: the latest official release's macOS / Linux / Android installers
/// (official GitHub link + accelerated mirrors) and the iOS App Store
/// link. Available to any logged-in user — these are public download
/// links, and the manual-config card on the same page is open to every
/// logged-in user too. When GitHub cannot be reached at all, this
/// returns 502 instead of inventing a version.
async fn client_downloads(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
) -> Response {
    if resolve_user(&st, &headers).await.is_none() {
        return unauthorized();
    }
    // Read the mirror setting with the handler's own async runtime —
    // custom_mirror() builds a throwaway runtime and must only run on
    // plain (non-tokio) threads like the download jobs.
    let custom = get_setting(&st.pool, "github_mirror")
        .await
        .trim()
        .trim_end_matches('/')
        .to_string();
    let st2 = st.clone();
    let custom2 = custom.clone();
    let res =
        tokio::task::spawn_blocking(move || refresh_client_downloads(&st2, &custom2)).await;
    let (version, downloads) = match res {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return err_resp(StatusCode::BAD_GATEWAY, &e),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let prefixes = mirror_prefixes(&custom);
    let mut platforms: Vec<Value> = downloads
        .iter()
        .map(|d| {
            let mirrors: Vec<String> = prefixes
                .iter()
                .map(|p| format!("{p}{}", d.url))
                .collect();
            json!({
                "id": d.id,
                "name": d.name,
                "file": d.file,
                "url": d.url,
                "mirrors": mirrors,
            })
        })
        .collect();
    platforms.push(json!({
        "id": "ios",
        "name": "iOS",
        "file": "",
        "url": IOS_APP_STORE_URL,
        "mirrors": [],
    }));
    jresp(
        StatusCode::OK,
        json!({ "ok": true, "version": version, "platforms": platforms }),
    )
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

// ------------------------------ relay nodes ------------------------------
// Managed relay nodes (v36): a relay host runs a small resident agent
// (relay-agent, Go) that registers with a one-time link, heartbeats
// every 20 s, and applies configuration pushed from the console.
// hbbr itself never talks to hbbs, so the agent is the management
// channel. Registration manages nodes; it does NOT add them to the
// relay-selection list (--relay-servers stays the only source of
// rotation — see DEPLOY.md).
//
// Trust model (deliberate, see DEPLOY.md): the registration link
// carries a random 32-byte symmetric key (the "link key"). The key
// itself NEVER crosses the wire: registration proves possession with
// an HMAC over a server-issued challenge, heartbeats are
// encrypt-then-MAC (AES-256-GCM + HMAC-SHA256, per-message nonce,
// timestamp + nonce replay protection), and the server keypair +
// all pushed config travel only inside AEAD bundles sealed with the
// link key. This defeats passive eavesdropping on plain HTTP; an
// active MITM can only deny service or corrupt messages (both are
// detected: AEAD authentication fails and the agent refuses to
// apply). The link itself is the credential: one-time, 24 h expiry,
// shown once — whoever holds an unused link can register a node and
// receive the server keypair. TLS on the API listener closes even
// the residual tampering window.

const RELAY_CHALLENGE_TTL_SECS: i64 = 300;
const RELAY_HEARTBEAT_TS_WINDOW_SECS: i64 = 300;
const RELAY_HEARTBEAT_REPLAY_TTL_SECS: i64 = 600;
const RELAY_ONLINE_THRESHOLD_SECS: i64 = 90;
const RELAY_REG_TOKEN_DEFAULT_TTL_HOURS: i64 = 24;
const RELAY_REG_TOKEN_MAX_TTL_HOURS: i64 = 168;

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < b.len() {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

/// Public identifier of a registration grant: the first 16 hex chars
/// of SHA-256(token). The token itself is never sent; the agent
/// derives this from the link, the server from the stored grant.
fn relay_token_id(token: &str) -> String {
    sha256_hex(token.as_bytes()).chars().take(16).collect()
}

fn relay_hmac_hex(key: &[u8], msg: &str) -> Option<String> {
    // Fully qualified: KeyInit (aes-gcm) and Mac both define
    // new_from_slice for Hmac.
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).ok()?;
    mac.update(msg.as_bytes());
    Some(hex_encode(&mac.finalize().into_bytes()))
}

/// Constant-time HMAC verification (via the hmac crate's verify).
fn relay_hmac_verify(key: &[u8], msg: &str, sig_hex: &str) -> bool {
    let sig = match hex_decode(sig_hex) {
        Some(s) => s,
        None => return false,
    };
    match <Hmac<Sha256> as Mac>::new_from_slice(key) {
        Ok(mut mac) => {
            mac.update(msg.as_bytes());
            mac.verify_slice(&sig).is_ok()
        }
        Err(_) => false,
    }
}

/// Seal with AES-256-GCM under the link key; returns
/// (nonce_hex, ciphertext_hex). One fresh random nonce per message.
fn relay_seal(key: &[u8], plaintext: &[u8]) -> Option<(String, String)> {
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce_bytes: [u8; 12] =
        sodiumoxide::randombytes::randombytes(12).try_into().ok()?;
    let nonce = Nonce::from(nonce_bytes);
    let ct = cipher.encrypt(&nonce, plaintext).ok()?;
    Some((hex_encode(&nonce_bytes), hex_encode(&ct)))
}

fn relay_open(key: &[u8], nonce_hex: &str, data_hex: &str) -> Option<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce_bytes: [u8; 12] = hex_decode(nonce_hex)?.try_into().ok()?;
    let data = hex_decode(data_hex)?;
    let nonce = Nonce::from(nonce_bytes);
    cipher.decrypt(&nonce, data.as_ref()).ok()
}

/// Exact message the agent signs for registration (challenge-
/// response). Byte-identical construction on both sides; keep in
/// sync with relay-agent (Go).
fn relay_register_proof_msg(
    token_id: &str,
    nonce: &str,
    name: &str,
    agent_version: &str,
    address: &str,
) -> String {
    format!("relay-register-v1\n{token_id}\n{nonce}\n{name}\n{agent_version}\n{address}")
}

/// Exact message the agent signs for each heartbeat (over the
/// encrypted bundle: encrypt-then-MAC). Keep in sync with
/// relay-agent (Go).
fn relay_heartbeat_sig_msg(
    node_id: i64,
    ts: i64,
    nonce: &str,
    bundle_nonce: &str,
    bundle_data: &str,
) -> String {
    format!("relay-heartbeat-v1\n{node_id}\n{ts}\n{nonce}\n{bundle_nonce}\n{bundle_data}")
}

/// Default relay config: stock hbbr port, no explicit limits (null =
/// hbbr built-in defaults: 32 / 1024 / 128 Mb/s).
fn relay_default_config() -> Value {
    json!({"port": 21117, "limit_speed": null, "total_bandwidth": null, "single_bandwidth": null})
}

/// Validate + normalize an admin-submitted relay config. Limits are
/// Mb/s; null (or missing) means "hbbr default".
fn parse_relay_config(v: &Value) -> Result<Value, String> {
    let port = v
        .get("port")
        .and_then(|x| x.as_i64())
        .ok_or_else(|| "端口必须是 1-65535 的整数".to_string())?;
    if !(1..=65535).contains(&port) {
        return Err("端口必须是 1-65535 的整数".into());
    }
    let limit = |key: &str| -> Result<Value, String> {
        match v.get(key) {
            None | Some(Value::Null) => Ok(Value::Null),
            Some(x) => {
                // 0 is not a usable value: hbbr ignores 0 on both the
                // console and the env var (keeps the current value),
                // so it would only produce a verify mismatch. Use
                // null (empty) for "hbbr default".
                let n = x
                    .as_f64()
                    .filter(|n| n.is_finite() && *n > 0.0)
                    .ok_or_else(|| format!("{key} 必须大于 0 的数字，留空表示默认值"))?;
                Ok(json!(n))
            }
        }
    };
    let limit_speed = limit("limit_speed")?;
    let total_bandwidth = limit("total_bandwidth")?;
    let single_bandwidth = limit("single_bandwidth")?;
    Ok(json!({
        "port": port,
        "limit_speed": limit_speed,
        "total_bandwidth": total_bandwidth,
        "single_bandwidth": single_bandwidth,
    }))
}

/// The server keypair as file contents (private, public), for
/// encrypted distribution to a registering relay. Normal case: the
/// id_ed25519 files in the server directory. Fallback: the server
/// was started with -k <base64 private key> and has no files — the
/// override itself is the private key, and the public key is its
/// second half. A public-key-only override has no private key to
/// distribute and fails here.
async fn server_keypair(st: &ApiState) -> Result<(String, String), String> {
    if let (Ok(sk), Ok(pk)) = (
        tokio::fs::read_to_string("id_ed25519").await,
        tokio::fs::read_to_string("id_ed25519.pub").await,
    ) {
        let sk = sk.trim().to_string();
        let pk = pk.trim().to_string();
        if !sk.is_empty() && !pk.is_empty() {
            if let Ok(raw) = base64::decode(&sk) {
                if raw.len() == 64 {
                    return Ok((sk, pk));
                }
            }
        }
    }
    let ov = st.key_override.trim();
    if !ov.is_empty() && ov != "-" {
        if let Ok(raw) = base64::decode(ov) {
            if raw.len() == 64 {
                let pk = base64::encode_config(&raw[32..], base64::STANDARD);
                return Ok((ov.to_string(), pk));
            }
        }
    }
    Err("服务器密钥文件不存在，无法下发密钥（请确认服务端目录下有 id_ed25519 文件）".into())
}

fn relay_node_summary(r: &sqlx::sqlite::SqliteRow, now_ts: i64) -> Value {
    let last_hb = r.try_get::<i64, _>("last_heartbeat").unwrap_or(0);
    json!({
        "id": r.try_get::<i64, _>("id").unwrap_or(0),
        "name": r.try_get::<String, _>("name").unwrap_or_default(),
        "address": r.try_get::<String, _>("address").unwrap_or_default(),
        "last_ip": r.try_get::<String, _>("last_ip").unwrap_or_default(),
        "key_delivered": r.try_get::<i64, _>("key_delivered").unwrap_or(0) != 0,
        "registered_at": r.try_get::<i64, _>("registered_at").unwrap_or(0),
        "last_heartbeat": last_hb,
        "online": last_hb > 0 && now_ts - last_hb < RELAY_ONLINE_THRESHOLD_SECS,
        "agent_version": r.try_get::<String, _>("agent_version").unwrap_or_default(),
        "hbbr_version": r.try_get::<String, _>("hbbr_version").unwrap_or_default(),
        "hbbr_running": r.try_get::<i64, _>("hbbr_running").unwrap_or(0) != 0,
        "hbbr_started_at": r.try_get::<i64, _>("hbbr_started_at").unwrap_or(0),
        "listen_port": r.try_get::<i64, _>("listen_port").unwrap_or(0),
        "sessions": r.try_get::<i64, _>("sessions").unwrap_or(0),
        "total_kbps": r.try_get::<i64, _>("total_kbps").unwrap_or(0),
        "config_version": r.try_get::<i64, _>("config_version").unwrap_or(1),
        "applied_version": r.try_get::<i64, _>("applied_version").unwrap_or(0),
        "pending_command": r.try_get::<String, _>("pending_command").unwrap_or_default(),
    })
}

/// Generic registration failure: deliberately does not distinguish
/// unknown / used / expired, so a bad link gives no oracle.
fn relay_reg_invalid() -> Response {
    jresp(
        StatusCode::OK,
        json!({ "ok": false, "error": "注册链接无效、已使用或已过期" }),
    )
}

/// Step 1 of registration: issue a one-time challenge nonce for a
/// registration grant. The nonce is consumed on first verification.
async fn relay_challenge(
    Extension(st): Extension<Arc<ApiState>>,
    body: String,
) -> Response {
    let v = parse_body(&body);
    let token_id = sfield(&v, "token_id");
    if token_id.len() != 16 || !token_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return relay_reg_invalid();
    }
    let t = now();
    if let Ok(mut m) = st.reg_challenges.lock() {
        m.retain(|_, (_, exp)| *exp > t);
        if m.len() >= 4096 {
            return jresp(
                StatusCode::OK,
                json!({ "ok": false, "error": "服务繁忙，请稍后重试" }),
            );
        }
    }
    let row = sqlx::query(
        "select used_at, expires_at from relay_reg_tokens where token_id=?",
    )
    .bind(&token_id)
    .fetch_optional(&st.pool)
    .await;
    let (used_at, expires_at) = match row {
        Ok(Some(r)) => (
            r.try_get::<Option<i64>, _>("used_at").ok().flatten(),
            r.try_get::<i64, _>("expires_at").unwrap_or(0),
        ),
        _ => return relay_reg_invalid(),
    };
    if used_at.is_some() || expires_at <= t {
        return relay_reg_invalid();
    }
    let nonce = hex_encode(&sodiumoxide::randombytes::randombytes(16));
    if let Ok(mut m) = st.reg_challenges.lock() {
        m.insert(nonce.clone(), (token_id, t + RELAY_CHALLENGE_TTL_SECS));
    }
    jresp(StatusCode::OK, json!({ "ok": true, "nonce": nonce }))
}

/// Step 2 of registration: verify the HMAC proof, create the node,
/// and return the server keypair + initial config sealed in an AEAD
/// bundle. The token and the link key never appear on the wire.
async fn relay_register(
    Extension(st): Extension<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: String,
) -> Response {
    let v = parse_body(&body);
    let token_id = sfield(&v, "token_id");
    let nonce = sfield(&v, "nonce");
    let proof = sfield(&v, "proof");
    let name = sfield(&v, "name");
    let agent_version = sfield(&v, "agent_version");
    let address = sfield(&v, "address");
    // 1. Consume the challenge (one attempt per nonce, whatever the
    // outcome — a retry must fetch a fresh challenge).
    let t = now();
    let challenge_ok = match st.reg_challenges.lock() {
        Ok(mut m) => {
            m.retain(|_, (_, exp)| *exp > t);
            match m.remove(&nonce) {
                Some((tid, _)) => tid == token_id,
                None => false,
            }
        }
        Err(_) => false,
    };
    if !challenge_ok {
        return jresp(
            StatusCode::OK,
            json!({ "ok": false, "error": "质询已过期或无效，请重新获取" }),
        );
    }
    // 2. Look up the grant.
    let row = sqlx::query(
        "select link_key, used_at, expires_at, created_by from relay_reg_tokens where token_id=?",
    )
    .bind(&token_id)
    .fetch_optional(&st.pool)
    .await;
    let (link_key_hex, used_at, expires_at, created_by) = match row {
        Ok(Some(r)) => (
            r.try_get::<String, _>("link_key").unwrap_or_default(),
            r.try_get::<Option<i64>, _>("used_at").ok().flatten(),
            r.try_get::<i64, _>("expires_at").unwrap_or(0),
            r.try_get::<String, _>("created_by").unwrap_or_default(),
        ),
        _ => return relay_reg_invalid(),
    };
    if used_at.is_some() || expires_at <= t {
        return relay_reg_invalid();
    }
    // 3. Verify possession of the link key.
    let link_key = match hex_decode(&link_key_hex) {
        Some(b) if b.len() == 32 => b,
        _ => {
            return jresp(
                StatusCode::OK,
                json!({ "ok": false, "error": "注册链接已损坏，请重新生成" }),
            )
        }
    };
    let msg = relay_register_proof_msg(&token_id, &nonce, &name, &agent_version, &address);
    if !relay_hmac_verify(&link_key, &msg, &proof) {
        return jresp(
            StatusCode::OK,
            json!({ "ok": false, "error": "注册验证失败" }),
        );
    }
    // 4. Server keypair for the bundle.
    let (private_key, public_key) = match server_keypair(&st).await {
        Ok(k) => k,
        Err(e) => return jresp(StatusCode::OK, json!({ "ok": false, "error": e })),
    };
    // 5. Create the node (name defaults to relay-<id> when empty).
    let name = name.trim().chars().take(64).collect::<String>();
    let address = address.trim().chars().take(128).collect::<String>();
    let agent_version = agent_version.chars().take(32).collect::<String>();
    let last_ip = addr.ip().to_string();
    let config = relay_default_config();
    let ins = sqlx::query(
        "insert into relay_nodes(name, address, last_ip, link_key, key_delivered, registered_at, agent_version, config_json, config_version, created_by) values(?, ?, ?, ?, 1, ?, ?, ?, 1, ?) returning id",
    )
    .bind(if name.is_empty() { "relay" } else { &name })
    .bind(&address)
    .bind(&last_ip)
    .bind(&link_key_hex)
    .bind(t)
    .bind(&agent_version)
    .bind(config.to_string())
    .bind(&created_by)
    .fetch_one(&st.pool)
    .await;
    let node_id = match ins {
        Ok(r) => r.try_get::<i64, _>("id").unwrap_or(0),
        Err(e) => {
            return jresp(
                StatusCode::OK,
                json!({ "ok": false, "error": format!("创建节点失败：{e}") }),
            )
        }
    };
    if node_id <= 0 {
        return jresp(
            StatusCode::OK,
            json!({ "ok": false, "error": "创建节点失败" }),
        );
    }
    if name.is_empty() {
        let _ = sqlx::query("update relay_nodes set name=? where id=?")
            .bind(format!("relay-{node_id}"))
            .bind(node_id)
            .execute(&st.pool)
            .await;
    }
    // 6. Seal the bundle BEFORE consuming the grant: if sealing ever
    // fails, the node row is dropped and the link stays usable.
    let bundle_plain = json!({
        "node_id": node_id,
        "private_key": private_key,
        "public_key": public_key,
        "config": config,
        "config_version": 1,
    });
    let sealed = relay_seal(&link_key, bundle_plain.to_string().as_bytes());
    let (bn, bd) = match sealed {
        Some(x) => x,
        None => {
            let _ = sqlx::query("delete from relay_nodes where id=?")
                .bind(node_id)
                .execute(&st.pool)
                .await;
            return jresp(
                StatusCode::OK,
                json!({ "ok": false, "error": "密钥加密失败，请重试" }),
            );
        }
    };
    // 7. Consume the grant.
    let _ = sqlx::query("update relay_reg_tokens set used_at=?, used_by=? where token_id=?")
        .bind(t)
        .bind(node_id)
        .bind(&token_id)
        .execute(&st.pool)
        .await;
    jresp(
        StatusCode::OK,
        json!({ "ok": true, "bundle": { "nonce": bn, "data": bd } }),
    )
}

/// Agent heartbeat: verify the HMAC signature over the encrypted
/// bundle (encrypt-then-MAC), decrypt the status, update the node,
/// and return any pending config/command sealed in a fresh bundle.
/// `revoked: true` tells the agent its node was deleted: stop
/// heartbeating, keep hbbr running.
async fn relay_heartbeat(
    Extension(st): Extension<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: String,
) -> Response {
    let v = parse_body(&body);
    let node_id = v.get("node_id").and_then(|x| x.as_i64()).unwrap_or(0);
    let ts = v.get("ts").and_then(|x| x.as_i64()).unwrap_or(0);
    let nonce = sfield(&v, "nonce");
    let sig = sfield(&v, "sig");
    let (b_nonce, b_data) = match v.get("bundle") {
        Some(b) => (sfield(b, "nonce"), sfield(b, "data")),
        None => (String::new(), String::new()),
    };
    if node_id <= 0
        || ts <= 0
        || nonce.is_empty()
        || sig.is_empty()
        || b_nonce.is_empty()
        || b_data.is_empty()
    {
        return jresp(
            StatusCode::OK,
            json!({ "ok": false, "error": "请求不完整", "revoked": false }),
        );
    }
    let row = sqlx::query(
        "select link_key, config_json, config_version, pending_command from relay_nodes where id=?",
    )
    .bind(node_id)
    .fetch_optional(&st.pool)
    .await;
    let (link_key_hex, config_json, config_version, pending_command) = match row {
        Ok(Some(r)) => (
            r.try_get::<String, _>("link_key").unwrap_or_default(),
            r.try_get::<String, _>("config_json").unwrap_or_default(),
            r.try_get::<i64, _>("config_version").unwrap_or(1),
            r.try_get::<String, _>("pending_command").unwrap_or_default(),
        ),
        _ => {
            return jresp(
                StatusCode::OK,
                json!({ "ok": false, "error": "节点不存在或已被删除", "revoked": true }),
            )
        }
    };
    let link_key = match hex_decode(&link_key_hex) {
        Some(b) if b.len() == 32 => b,
        _ => {
            return jresp(
                StatusCode::OK,
                json!({ "ok": false, "error": "节点密钥已损坏", "revoked": true }),
            )
        }
    };
    let t = now();
    if (t - ts).abs() > RELAY_HEARTBEAT_TS_WINDOW_SECS {
        return jresp(
            StatusCode::OK,
            json!({ "ok": false, "error": "时间戳过期，请校准中继机器时间", "revoked": false }),
        );
    }
    let replayed = match st.hb_replay.lock() {
        Ok(mut m) => {
            m.retain(|_, seen| t - *seen < RELAY_HEARTBEAT_REPLAY_TTL_SECS);
            if m.contains_key(&(node_id, nonce.clone())) {
                true
            } else {
                m.insert((node_id, nonce.clone()), t);
                false
            }
        }
        Err(_) => false,
    };
    if replayed {
        return jresp(
            StatusCode::OK,
            json!({ "ok": false, "error": "重放请求被拒绝", "revoked": false }),
        );
    }
    let msg = relay_heartbeat_sig_msg(node_id, ts, &nonce, &b_nonce, &b_data);
    if !relay_hmac_verify(&link_key, &msg, &sig) {
        return jresp(
            StatusCode::OK,
            json!({ "ok": false, "error": "签名校验失败", "revoked": false }),
        );
    }
    let plain = match relay_open(&link_key, &b_nonce, &b_data) {
        Some(p) => p,
        None => {
            return jresp(
                StatusCode::OK,
                json!({ "ok": false, "error": "数据解密失败", "revoked": false }),
            )
        }
    };
    let pv: Value = match serde_json::from_slice(&plain) {
        Ok(p) => p,
        Err(_) => {
            return jresp(
                StatusCode::OK,
                json!({ "ok": false, "error": "数据格式错误", "revoked": false }),
            )
        }
    };
    let applied_version = pv.get("applied_version").and_then(|x| x.as_i64()).unwrap_or(0);
    let command_ack = sfield(&pv, "command_ack");
    let status = pv.get("status").cloned().unwrap_or(json!({}));
    let hbbr_running = status
        .get("hbbr_running")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let hbbr_started_at = status
        .get("hbbr_started_at")
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    let hbbr_version = sfield(&status, "hbbr_version")
        .chars()
        .take(64)
        .collect::<String>();
    let agent_version = sfield(&status, "agent_version")
        .chars()
        .take(32)
        .collect::<String>();
    let listen_port = status.get("listen_port").and_then(|x| x.as_i64()).unwrap_or(0);
    let sessions = status.get("sessions").and_then(|x| x.as_i64()).unwrap_or(0);
    let total_kbps = status.get("total_kbps").and_then(|x| x.as_i64()).unwrap_or(0);
    let new_pending = if !command_ack.is_empty() && command_ack == pending_command {
        String::new()
    } else {
        pending_command.clone()
    };
    let last_ip = addr.ip().to_string();
    let _ = sqlx::query(
        "update relay_nodes set last_heartbeat=?, last_ip=?, agent_version=?, hbbr_version=?, hbbr_running=?, hbbr_started_at=?, listen_port=?, sessions=?, total_kbps=?, applied_version=?, pending_command=? where id=?",
    )
    .bind(t)
    .bind(&last_ip)
    .bind(&agent_version)
    .bind(&hbbr_version)
    .bind(if hbbr_running { 1 } else { 0 })
    .bind(hbbr_started_at)
    .bind(listen_port)
    .bind(sessions)
    .bind(total_kbps)
    .bind(applied_version)
    .bind(&new_pending)
    .bind(node_id)
    .execute(&st.pool)
    .await;
    let need_config = applied_version < config_version;
    let need_command = !new_pending.is_empty();
    if !need_config && !need_command {
        return jresp(StatusCode::OK, json!({ "ok": true, "bundle": null }));
    }
    let cfg = if need_config {
        serde_json::from_str::<Value>(&config_json).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let resp_plain = json!({
        "config": cfg,
        "config_version": config_version,
        "command": if need_command { json!(new_pending) } else { Value::Null },
    });
    match relay_seal(&link_key, resp_plain.to_string().as_bytes()) {
        Some((rn, rd)) => jresp(
            StatusCode::OK,
            json!({ "ok": true, "bundle": { "nonce": rn, "data": rd } }),
        ),
        None => jresp(
            StatusCode::OK,
            json!({ "ok": false, "error": "配置加密失败", "revoked": false }),
        ),
    }
}

/// Browser page for a registration link: shows whether the link is
/// still valid and the exact command to run on the relay host. The
/// token/key are never logged; the page reads them from the URL the
/// admin already holds.
async fn relay_register_page(
    Extension(st): Extension<Arc<ApiState>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let token = q.get("token").cloned().unwrap_or_default();
    let status = if token.is_empty() {
        "missing"
    } else {
        let tid = relay_token_id(&token);
        match sqlx::query("select used_at, expires_at from relay_reg_tokens where token_id=?")
            .bind(&tid)
            .fetch_optional(&st.pool)
            .await
        {
            Ok(Some(r)) => {
                let used = r.try_get::<Option<i64>, _>("used_at").ok().flatten();
                let exp = r.try_get::<i64, _>("expires_at").unwrap_or(0);
                if used.is_some() {
                    "used"
                } else if exp <= now() {
                    "expired"
                } else {
                    "valid"
                }
            }
            _ => "unknown",
        }
    };
    Html(crate::api_pages::relay_register_page_html(status)).into_response()
}

async fn relay_admin(
    st: &ApiState,
    headers: &HeaderMap,
) -> Result<AuthUser, Response> {
    let u = match resolve_user(st, headers).await {
        Some(u) => u,
        None => return Err(unauthorized()),
    };
    if !u.is_admin {
        return Err(forbidden());
    }
    Ok(u)
}

/// Generate a registration link (admin only). The link carries a
/// fresh one-time token AND a fresh 32-byte link key; both are shown
/// exactly once, in this response. The token hash is never stored —
/// only its public token_id — because the token itself is never sent
/// back to the server.
async fn relays_reg_token_create(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let u = match relay_admin(&st, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let v = parse_body(&body);
    let label = sfield(&v, "label").chars().take(64).collect::<String>();
    let hours = v
        .get("expires_hours")
        .and_then(|x| x.as_i64())
        .unwrap_or(RELAY_REG_TOKEN_DEFAULT_TTL_HOURS)
        .clamp(1, RELAY_REG_TOKEN_MAX_TTL_HOURS);
    let (_, api, _, _, _) = match client_config(&st).await {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, &e),
    };
    let token = format!("rrt_{}", rand_hex(32));
    let link_key = rand_hex(32);
    let token_id = relay_token_id(&token);
    let t = now();
    let expires_at = t + hours * 3600;
    if let Err(e) = sqlx::query(
        "insert into relay_reg_tokens(token_id, link_key, label, created_by, created_at, expires_at) values(?, ?, ?, ?, ?, ?)",
    )
    .bind(&token_id)
    .bind(&link_key)
    .bind(&label)
    .bind(&u.username)
    .bind(t)
    .bind(expires_at)
    .execute(&st.pool)
    .await
    {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let link = format!("{api}/relay/register?token={token}&key={link_key}");
    jresp(
        StatusCode::OK,
        json!({ "ok": true, "link": link, "token_id": token_id, "expires_at": expires_at }),
    )
}

/// Outstanding registration links (admin only). Link keys are NEVER
/// returned — a link is shown once, at creation.
async fn relays_reg_tokens_list(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = relay_admin(&st, &headers).await {
        return r;
    }
    let rows = sqlx::query(
        "select token_id, label, created_by, created_at, expires_at, used_at, used_by from relay_reg_tokens order by created_at desc",
    )
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
                "token_id": r.try_get::<String, _>("token_id").unwrap_or_default(),
                "label": r.try_get::<String, _>("label").unwrap_or_default(),
                "created_by": r.try_get::<String, _>("created_by").unwrap_or_default(),
                "created_at": r.try_get::<i64, _>("created_at").unwrap_or(0),
                "expires_at": r.try_get::<i64, _>("expires_at").unwrap_or(0),
                "used_at": r.try_get::<Option<i64>, _>("used_at").ok().flatten(),
                "used_by": r.try_get::<Option<i64>, _>("used_by").ok().flatten(),
            })
        })
        .collect();
    jresp(StatusCode::OK, json!({ "ok": true, "tokens": tokens }))
}

/// Revoke an unused registration link (admin only).
async fn relays_reg_token_revoke(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(token_id): Path<String>,
) -> Response {
    if let Err(r) = relay_admin(&st, &headers).await {
        return r;
    }
    match sqlx::query("delete from relay_reg_tokens where token_id=?")
        .bind(&token_id)
        .execute(&st.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => jresp(StatusCode::OK, json!({ "ok": true })),
        Ok(_) => err_resp(StatusCode::NOT_FOUND, "注册链接不存在"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn relays_list(Extension(st): Extension<Arc<ApiState>>, headers: HeaderMap) -> Response {
    if let Err(r) = relay_admin(&st, &headers).await {
        return r;
    }
    let rows = sqlx::query("select * from relay_nodes order by id desc")
        .fetch_all(&st.pool)
        .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let t = now();
    let nodes: Vec<Value> = rows.iter().map(|r| relay_node_summary(r, t)).collect();
    jresp(StatusCode::OK, json!({ "ok": true, "now": t, "nodes": nodes }))
}

async fn relays_detail(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Err(r) = relay_admin(&st, &headers).await {
        return r;
    }
    let row = sqlx::query("select * from relay_nodes where id=?")
        .bind(id)
        .fetch_optional(&st.pool)
        .await;
    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "中继节点不存在"),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let mut node = relay_node_summary(&row, now());
    let cfg = row
        .try_get::<String, _>("config_json")
        .unwrap_or_default();
    node["config"] = serde_json::from_str::<Value>(&cfg).unwrap_or(Value::Null);
    node["created_by"] = json!(row.try_get::<String, _>("created_by").unwrap_or_default());
    jresp(StatusCode::OK, json!({ "ok": true, "node": node }))
}

/// Delete a relay node (admin only). The node credential dies with
/// the row: the agent's next heartbeat fails lookup and it stops
/// heartbeating (hbbr itself keeps running — the admin stops it on
/// the relay host if the node is really gone).
async fn relays_delete(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Err(r) = relay_admin(&st, &headers).await {
        return r;
    }
    match sqlx::query("delete from relay_nodes where id=?")
        .bind(id)
        .execute(&st.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => jresp(StatusCode::OK, json!({ "ok": true })),
        Ok(_) => err_resp(StatusCode::NOT_FOUND, "中继节点不存在"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Push a config to a relay node (admin only). The config is stored
/// and delivered inside the next heartbeat response bundle; the
/// agent applies it (port change restarts hbbr, limits apply live).
async fn relays_config(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: String,
) -> Response {
    if let Err(r) = relay_admin(&st, &headers).await {
        return r;
    }
    let v = parse_body(&body);
    let cfg = match parse_relay_config(&v) {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, &e),
    };
    let row = sqlx::query("select config_version from relay_nodes where id=?")
        .bind(id)
        .fetch_optional(&st.pool)
        .await;
    let cur = match row {
        Ok(Some(r)) => r.try_get::<i64, _>("config_version").unwrap_or(1),
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "中继节点不存在"),
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    match sqlx::query("update relay_nodes set config_json=?, config_version=? where id=?")
        .bind(cfg.to_string())
        .bind(cur + 1)
        .bind(id)
        .execute(&st.pool)
        .await
    {
        Ok(_) => jresp(
            StatusCode::OK,
            json!({ "ok": true, "config_version": cur + 1 }),
        ),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Queue a remote hbbr restart for a relay node (admin only). The
/// command rides the next heartbeat response; the agent restarts
/// hbbr and acks, which clears the queue. Active relayed sessions
/// on that node drop when hbbr restarts.
async fn relays_restart(
    Extension(st): Extension<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Err(r) = relay_admin(&st, &headers).await {
        return r;
    }
    match sqlx::query("update relay_nodes set pending_command='restart' where id=?")
        .bind(id)
        .execute(&st.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => jresp(StatusCode::OK, json!({ "ok": true })),
        Ok(_) => err_resp(StatusCode::NOT_FOUND, "中继节点不存在"),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
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
            .route("/dl/:token/:filename", get(download))
            // connection audit records posted by the controlled side
            .route("/api/audit/conn", post(audit_conn_post))
            // relay agent endpoints (no session auth; HMAC + AEAD)
            .route("/api/relay/challenge", post(relay_challenge))
            .route("/api/relay/register", post(relay_register))
            .route("/api/relay/heartbeat", post(relay_heartbeat))
            .route("/relay/register", get(relay_register_page));
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
            .route("/api/auth/change-password", post(change_password))
            .route("/api/me", get(me))
            .route("/api/client-config", get(client_config_view))
            .route("/api/client-downloads", get(client_downloads))
            .route("/api/admin/overview", get(overview))
            .route("/api/admin/users", get(admin_users).post(admin_create_user))
            .route("/api/admin/users/:name/password", post(admin_user_password))
            .route("/api/admin/users/:name/:action", post(admin_user_action))
            .route("/api/devices", get(devices_list))
            .route("/api/audit/sessions", get(audit_sessions))
            .route("/api/tokens", get(tokens_list).post(tokens_create))
            .route("/api/tokens/:id", delete(tokens_delete).patch(tokens_rename))
            .route("/api/admin/settings", get(settings_get).post(settings_post))
            .route("/api/admin/template/download", post(template_download))
            .route("/api/admin/template/status", get(template_status))
            .route("/api/admin/update/status", get(update_status))
            .route("/api/admin/update/check", post(update_check))
            .route("/api/admin/update/download", post(update_download))
            .route("/api/admin/update/restart", post(update_restart))
            .route("/api/admin/gen-client", post(gen_client))
            .route("/api/admin/gen-clients", get(gen_list))
            .route("/api/admin/gen-clients/:id", delete(gen_delete))
            // relay node management (admin only)
            .route("/api/admin/relays/reg-token", post(relays_reg_token_create))
            .route("/api/admin/relays/reg-tokens", get(relays_reg_tokens_list))
            .route("/api/admin/relays/reg-tokens/:id", delete(relays_reg_token_revoke))
            .route("/api/admin/relays", get(relays_list))
            .route("/api/admin/relays/:id", get(relays_detail).delete(relays_delete))
            .route("/api/admin/relays/:id/config", post(relays_config))
            .route("/api/admin/relays/:id/restart", post(relays_restart));
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
            exe_dir: cwd.clone(),
            peer_db_path: "db_v2.sqlite3".to_string(),
            key_override,
            tpl_job: std::sync::Mutex::new(TplJob::default()),
            upd_job: std::sync::Mutex::new(UpdJob::default()),
            guard: std::sync::Mutex::new(AuthGuard::default()),
            dl_cache: std::sync::Mutex::new(None),
            reg_challenges: std::sync::Mutex::new(HashMap::new()),
            hb_replay: std::sync::Mutex::new(HashMap::new()),
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
            Some("http://rd.example.com:21114"),
            Some("rd.example.com"),
        )
        .unwrap();
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

    /// Standard layout: api/relay are omitted from the encoded JSON
    /// (the client derives them), so host/key still round-trip and
    /// api/relay decode as empty.
    #[test]
    fn generated_filename_omits_derivable_fields() {
        let name = generated_filename(
            "27.45.254.164:21116",
            "NKsFeb7rJOQ04GYcLrNe8EiUcnj5cqQx6iPQuB3tKqw=",
            None,
            None,
        )
        .unwrap();
        assert!(name.starts_with("rustdesk--"), "filename was: {name}");
        assert!(name.len() < 200, "filename was: {name}");
        assert_eq!(
            client_parse(&name),
            Some((
                "27.45.254.164:21116".to_string(),
                "NKsFeb7rJOQ04GYcLrNe8EiUcnj5cqQx6iPQuB3tKqw=".to_string(),
                String::new(),
                String::new(),
            )),
            "filename was: {name}"
        );
    }

    /// A 40-character public host still yields the encoded form well
    /// under the limit (before v31 it failed with ENAMETOOLONG).
    #[test]
    fn generated_filename_long_host_stays_encoded() {
        let host = "rustdesk-server-0123456789.abexample.com:21116";
        assert_eq!(host.len(), 46); // 40-char domain + ":21116"
        let name = generated_filename(
            host,
            "NKsFeb7rJOQ04GYcLrNe8EiUcnj5cqQx6iPQuB3tKqw=",
            None,
            None,
        )
        .unwrap();
        assert!(name.starts_with("rustdesk--"), "filename was: {name}");
        assert!(name.len() <= ENCODED_FILENAME_LIMIT, "filename was: {name}");
        let parsed = client_parse(&name).unwrap();
        assert_eq!(parsed.0, host);
    }

    /// A very long host falls back to the plain form, which the
    /// client parses via its `host=` branch.
    #[test]
    fn generated_filename_very_long_host_falls_back_to_plain() {
        let host = "a-very-long-rustdesk-server-hostname-for-a-branch-office-0123456789.example.com:21116";
        let name = generated_filename(
            host,
            "NKsFeb7rJOQ04GYcLrNe8EiUcnj5cqQx6iPQuB3tKqw=",
            None,
            None,
        )
        .unwrap();
        assert!(
            name.starts_with("rustdesk-host="),
            "filename was: {name}"
        );
        assert!(name.len() <= FILENAME_LIMIT, "filename was: {name}");
        assert!(name.contains("key=NKsFeb7rJOQ04GYcLrNe8EiUcnj5cqQx6iPQuB3tKqw="));
    }

    /// An absurdly long host fails with a plain-language error,
    /// not a raw OS error.
    #[test]
    fn generated_filename_absurd_host_errors_clearly() {
        let host = "x".repeat(240);
        let err = generated_filename(
            &host,
            "NKsFeb7rJOQ04GYcLrNe8EiUcnj5cqQx6iPQuB3tKqw=",
            None,
            None,
        )
        .unwrap_err();
        assert!(err.contains("公网地址太长"), "error was: {err}");
    }

    /// The share string is the same payload as a generated client's
    /// filename: wrapping it in the `rustdesk--<payload>.exe` form
    /// and running the client-side filename parser over it must
    /// recover all four fields exactly.
    #[test]
    fn server_config_share_matches_filename_payload() {
        let share = encode_server_config(
            "rd.example.com:21116",
            "rd.example.com:21117",
            "http://rd.example.com:21114",
            "AbCdEf0123456789+/xyz=",
        );
        assert!(!share.is_empty());
        let (host, key, api, relay) =
            client_parse(&format!("rustdesk--{share}.exe")).expect("share must parse");
        assert_eq!(host, "rd.example.com:21116");
        assert_eq!(key, "AbCdEf0123456789+/xyz=");
        assert_eq!(api, "http://rd.example.com:21114");
        assert_eq!(relay, "rd.example.com:21117");
    }

    /// Byte-for-byte identical to the official client's own export:
    /// `ServerConfig.encode()` (flutter/lib/common.dart) emits
    /// base64url of `{"host","relay","api","key"}` in that order,
    /// then reverses it. Decoding the share string must reproduce
    /// that exact JSON.
    #[test]
    fn server_config_share_matches_client_export() {
        let share = encode_server_config(
            "rd.example.com:21116",
            "rd.example.com:21117",
            "http://rd.example.com:21114",
            "AbCdEf0123456789+/xyz=",
        );
        let rev: String = share.chars().rev().collect();
        let data = base64::decode_config(&rev, base64::URL_SAFE_NO_PAD).unwrap();
        let raw = String::from_utf8(data).unwrap();
        assert_eq!(
            raw,
            "{\"host\":\"rd.example.com:21116\",\"relay\":\"rd.example.com:21117\",\"api\":\"http://rd.example.com:21114\",\"key\":\"AbCdEf0123456789+/xyz=\"}"
        );
    }

    /// All four fields are always present in the share string, even
    /// when the filename form would omit derivable ones — the string
    /// must match the manual-config card value for value.
    #[test]
    fn server_config_share_keeps_all_fields() {
        let share = encode_server_config(
            "rd.example.com:21116",
            "rd.example.com:21117",
            "http://rd.example.com:21114",
            "k",
        );
        let rev: String = share.chars().rev().collect();
        let data = base64::decode_config(&rev, base64::URL_SAFE_NO_PAD).unwrap();
        let v: Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(v["host"], "rd.example.com:21116");
        assert_eq!(v["relay"], "rd.example.com:21117");
        assert_eq!(v["api"], "http://rd.example.com:21114");
        assert_eq!(v["key"], "k");
    }

    /// Non-standard ports and TLS/override keep the fields embedded.
    #[test]
    fn needs_embed_decisions() {
        // Standard layout: both derivable, neither embedded.
        assert_eq!(
            needs_embed("http://h:21114", "http://h:21114", 21114, 21117, 21116),
            (false, false)
        );
        // API on a non-default port: the client's port-2 derivation
        // would point at 21114, not 31114, so api stays embedded
        // even though it equals the computed default; relay omitted.
        assert_eq!(
            needs_embed("http://h:31114", "http://h:31114", 31114, 21117, 21116),
            (true, false)
        );
        // Relay port not rendezvous+1: relay stays embedded.
        assert_eq!(
            needs_embed("http://h:21114", "http://h:21114", 21114, 21119, 21116),
            (false, true)
        );
        // TLS (https) or a custom override: api differs from the
        // plain-http default, so it stays embedded.
        assert_eq!(
            needs_embed("https://h:21114", "http://h:21114", 21114, 21117, 21116),
            (true, false)
        );
        assert_eq!(
            needs_embed(
                "http://other.example:8080/api",
                "http://h:21114",
                21114,
                21117,
                21116
            ),
            (true, false)
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

    #[test]
    fn update_tag_parsing() {
        assert_eq!(parse_pro_tag("v32"), Some(32));
        assert_eq!(parse_pro_tag("32"), Some(32));
        assert_eq!(parse_pro_tag("v031"), Some(31));
        assert_eq!(parse_pro_tag("v32.1"), None);
        assert_eq!(parse_pro_tag("latest"), None);
        assert_eq!(parse_pro_tag("v"), None);
        assert_eq!(parse_pro_tag(""), None);
        assert_eq!(parse_pro_tag("v32 "), Some(32));
        assert_eq!(current_pro_num(), 43);
    }

    #[test]
    fn update_version_param() {
        assert!(valid_version_param("v32"));
        assert!(valid_version_param("v1"));
        assert!(!valid_version_param("32"));
        assert!(!valid_version_param("v"));
        assert!(!valid_version_param("v32x"));
        assert!(!valid_version_param("v32.1"));
        assert!(!valid_version_param("https://example.com/x.zip"));
        assert!(!valid_version_param(""));
    }

    #[test]
    fn update_platform_mapping() {
        assert_eq!(
            package_for_platform("windows", "x86_64", false),
            Some("windows-x86_64")
        );
        assert_eq!(
            package_for_platform("linux", "x86_64", false),
            Some("linux-x86_64-glibc")
        );
        assert_eq!(
            package_for_platform("linux", "aarch64", false),
            Some("linux-aarch64-glibc")
        );
        assert_eq!(
            package_for_platform("linux", "aarch64", true),
            Some("linux-aarch64-musl")
        );
        assert_eq!(package_for_platform("macos", "x86_64", false), None);
        assert_eq!(package_for_platform("linux", "arm", false), None);
        assert_eq!(package_for_platform("windows", "aarch64", false), None);
        // This machine: Linux x86_64 glibc.
        assert_eq!(this_platform(), Some("linux-x86_64-glibc"));
    }

    #[test]
    fn update_asset_name_format() {
        assert_eq!(
            update_asset_name("v32", "linux-x86_64-glibc"),
            "rustdesk-server-pro-linux-x86_64-glibc-v32.zip"
        );
        assert_eq!(
            update_asset_name("v31", "windows-x86_64"),
            "rustdesk-server-pro-windows-x86_64-v31.zip"
        );
    }

    #[test]
    fn update_mirror_candidates() {
        let cs = github_candidates("", "https://github.com/x/y");
        assert_eq!(cs.len(), 6); // 5 built-in mirrors + direct
        assert_eq!(cs[0].0, "https://gh-proxy.com/");
        assert_eq!(cs[0].1, "https://gh-proxy.com/https://github.com/x/y");
        assert_eq!(cs[5].0, "直连 GitHub");
        assert_eq!(cs[5].1, "https://github.com/x/y");
        let cs = github_candidates("https://m.example", "https://github.com/x/y");
        assert_eq!(cs.len(), 7);
        assert_eq!(cs[0].0, "自定义镜像 https://m.example");
        assert_eq!(cs[0].1, "https://m.example/https://github.com/x/y");
        let ps = mirror_prefixes("https://m.example");
        assert_eq!(ps[0], "https://m.example/");
        assert!(ps.iter().all(|p| p.ends_with('/')));
    }

    #[test]
    fn update_tag_from_url() {
        assert_eq!(
            tag_from_url("https://github.com/wirh/rustdesk-server-pro/releases/tag/v31"),
            Some("v31".to_string())
        );
        assert_eq!(
            tag_from_url("https://github.com/wirh/rustdesk-server-pro/releases/tag/v31/"),
            Some("v31".to_string())
        );
        assert_eq!(
            tag_from_url("https://github.com/x/y/releases/tag/v32?foo=1"),
            Some("v32".to_string())
        );
        assert_eq!(
            tag_from_url("https://example.com/dl/v33"),
            Some("v33".to_string())
        );
        assert_eq!(tag_from_url("https://github.com/x/y/releases"), None);
        assert_eq!(
            tag_from_url("https://github.com/x/y/releases/tag/latest"),
            None
        );
    }

    #[test]
    fn update_parse_release_json() {
        let v: Value = serde_json::from_str(
            r##"{"tag_name":"v33","body":"# v33\n\n- fix A","published_at":"2026-11-14T00:00:00Z"}"##,
        )
        .unwrap();
        let out = parse_release_json(&v).unwrap();
        assert_eq!(out.tag, "v33");
        assert_eq!(out.body, "# v33\n\n- fix A");
        assert_eq!(out.published_at, "2026-11-14T00:00:00Z");
        assert!(!out.notes_unavailable);
        let v: Value = serde_json::from_str(r#"{"body":"x"}"#).unwrap();
        assert!(parse_release_json(&v).is_err());
    }

    #[test]
    fn update_extract_roundtrip() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let exe_dir = dir.path();
        let hbbs_name = exe_file_name("hbbs");
        let hbbr_name = exe_file_name("hbbr");
        // Build a release-shaped zip: one top folder, both binaries,
        // plus an unrelated file that must be ignored.
        let zip_path = exe_dir.join("rel.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file(format!("rustdesk-server-pro-test/{hbbs_name}"), opts)
                .unwrap();
            zw.write_all(b"fake-hbbs-binary").unwrap();
            zw.start_file(format!("rustdesk-server-pro-test/{hbbr_name}"), opts)
                .unwrap();
            zw.write_all(b"fake-hbbr-binary").unwrap();
            zw.start_file("rustdesk-server-pro-test/README.md", opts)
                .unwrap();
            zw.write_all(b"readme").unwrap();
            zw.finish().unwrap();
        }
        extract_update(&zip_path, exe_dir).unwrap();
        assert_eq!(
            std::fs::read(staged_hbbs(exe_dir)).unwrap(),
            b"fake-hbbs-binary"
        );
        assert_eq!(
            std::fs::read(staged_hbbr(exe_dir)).unwrap(),
            b"fake-hbbr-binary"
        );
        assert!(!exe_dir.join("README.md").exists());
        // No marker yet: not a complete staged update.
        assert_eq!(staged_version(exe_dir), None);
        std::fs::create_dir_all(exe_dir.join("update")).unwrap();
        std::fs::write(staged_meta(exe_dir), r#"{"version":"v33"}"#).unwrap();
        assert_eq!(staged_version(exe_dir), Some("v33".to_string()));
        std::fs::remove_file(staged_hbbr(exe_dir)).unwrap();
        assert_eq!(staged_version(exe_dir), None);

        // A zip missing one binary fails and leaves no staged files.
        let dir2 = tempfile::tempdir().unwrap();
        let exe_dir2 = dir2.path();
        let zip_path2 = exe_dir2.join("rel.zip");
        {
            let f = std::fs::File::create(&zip_path2).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file(format!("top/{hbbs_name}"), opts).unwrap();
            zw.write_all(b"only-hbbs").unwrap();
            zw.finish().unwrap();
        }
        assert!(extract_update(&zip_path2, exe_dir2).is_err());
        assert!(!staged_hbbs(exe_dir2).exists());
        assert!(!staged_hbbr(exe_dir2).exists());
    }

    #[test]
    fn update_redirect_tag_live() {
        // Live network: GitHub must be reachable (same assumption as
        // the template e2e suites). The redirect fallback must resolve
        // our latest release tag.
        let tag = fetch_redirect_tag(UPDATE_RELEASES_PAGE)
            .expect("redirect fallback must reach GitHub");
        let n = parse_pro_tag(&tag).expect("redirect tag must be a vN tag");
        assert!(n >= 31, "expected at least v31, got {tag}");
    }

    fn release_fixture(tag: &str, names: &[&str]) -> Value {
        let assets: Vec<Value> = names
            .iter()
            .map(|n| {
                json!({
                    "name": n,
                    "browser_download_url": format!(
                        "https://github.com/rustdesk/rustdesk/releases/download/{tag}/{n}"
                    ),
                })
            })
            .collect();
        json!({ "tag_name": tag, "assets": assets })
    }

    #[test]
    fn client_downloads_parse_real_1_5_3_assets() {
        // The exact 1.5.3 asset list from the GitHub API (verified
        // 2026-11-13): 10 of these are other-platform downloads, the
        // rest (Windows installers, rpm/flatpak, sciter builds, the
        // unattended-wayland deb, update metadata) must be ignored.
        let names = [
            "rustdesk-1.5.3-0-x86_64.pkg.tar.zst",
            "rustdesk-1.5.3-0.aarch64-suse.rpm",
            "rustdesk-1.5.3-0.aarch64.rpm",
            "rustdesk-1.5.3-0.x86_64-suse.rpm",
            "rustdesk-1.5.3-0.x86_64.rpm",
            "rustdesk-1.5.3-aarch64-signed.apk",
            "rustdesk-1.5.3-aarch64.AppImage",
            "rustdesk-1.5.3-aarch64.deb",
            "rustdesk-1.5.3-aarch64.dmg",
            "rustdesk-1.5.3-aarch64.exe",
            "rustdesk-1.5.3-aarch64.flatpak",
            "rustdesk-1.5.3-aarch64.msi",
            "rustdesk-1.5.3-armv7-sciter.deb",
            "rustdesk-1.5.3-armv7-signed.apk",
            "rustdesk-1.5.3-universal-signed.apk",
            "rustdesk-1.5.3-unsigned.tar.gz",
            "rustdesk-1.5.3-x86-sciter.exe",
            "rustdesk-1.5.3-x86_64-sciter.deb",
            "rustdesk-1.5.3-x86_64-sciter.flatpak",
            "rustdesk-1.5.3-x86_64-signed.apk",
            "rustdesk-1.5.3-x86_64.AppImage",
            "rustdesk-1.5.3-x86_64.deb",
            "rustdesk-1.5.3-x86_64.dmg",
            "rustdesk-1.5.3-x86_64.exe",
            "rustdesk-1.5.3-x86_64.flatpak",
            "rustdesk-1.5.3-x86_64.msi",
            "rustdesk-unattended-wayland-1.5.3-x86_64.deb",
            "rustdesk-update.json",
            "rustdesk-update.json.sig",
            "rustdesk.sbom.json",
        ];
        let (version, downloads) =
            parse_client_downloads(&release_fixture("1.5.3", &names)).unwrap();
        assert_eq!(version, "1.5.3");
        let ids: Vec<&str> = downloads.iter().map(|d| d.id).collect();
        assert_eq!(
            ids,
            [
                "macos-x64",
                "macos-arm64",
                "linux-deb-x64",
                "linux-deb-arm64",
                "linux-appimage-x64",
                "linux-appimage-arm64",
                "android-arm64",
                "android-x64",
                "android-armv7",
                "android-universal",
            ]
        );
        assert_eq!(downloads[0].file, "rustdesk-1.5.3-x86_64.dmg");
        assert_eq!(downloads[0].url, "https://github.com/rustdesk/rustdesk/releases/download/1.5.3/rustdesk-1.5.3-x86_64.dmg");
        assert_eq!(downloads[2].file, "rustdesk-1.5.3-x86_64.deb");
        assert_eq!(downloads[9].file, "rustdesk-1.5.3-universal-signed.apk");
        // The unattended-wayland and sciter debs must not have matched.
        assert!(downloads
            .iter()
            .all(|d| !d.file.contains("wayland") && !d.file.contains("sciter")));
    }

    #[test]
    fn client_downloads_parse_edge_cases() {
        // Missing tag: an error, never an invented version.
        assert!(parse_client_downloads(&json!({ "assets": [] })).is_err());
        // A 'v'-prefixed tag is accepted and stripped.
        let (version, downloads) =
            parse_client_downloads(&release_fixture("v9.9.9", &[])).unwrap();
        assert_eq!(version, "9.9.9");
        assert!(downloads.is_empty());
        // A release with no matching assets yields an empty list.
        let (version, downloads) = parse_client_downloads(&release_fixture(
            "1.5.3",
            &["rustdesk-1.5.3-x86_64.exe", "rustdesk-1.5.3-x86_64.msi"],
        ))
        .unwrap();
        assert_eq!(version, "1.5.3");
        assert!(downloads.is_empty());
    }

    #[test]
    fn client_downloads_mirror_urls() {
        let direct = "https://github.com/rustdesk/rustdesk/releases/download/1.5.3/rustdesk-1.5.3-x86_64.dmg";
        // No custom mirror: built-ins only, direct excluded (it is the
        // separate official link).
        let ps = mirror_prefixes("");
        assert_eq!(ps.len(), 5);
        let urls: Vec<String> = ps.iter().map(|p| format!("{p}{direct}")).collect();
        assert_eq!(
            urls[0],
            "https://gh-proxy.com/https://github.com/rustdesk/rustdesk/releases/download/1.5.3/rustdesk-1.5.3-x86_64.dmg"
        );
        assert!(urls.iter().all(|u| u.starts_with("https://")));
        // Custom mirror comes first.
        let ps = mirror_prefixes("https://m.example");
        assert_eq!(ps.len(), 6);
        assert_eq!(ps[0], "https://m.example/");
        let urls: Vec<String> = ps.iter().map(|p| format!("{p}{direct}")).collect();
        assert_eq!(
            urls[0],
            "https://m.example/https://github.com/rustdesk/rustdesk/releases/download/1.5.3/rustdesk-1.5.3-x86_64.dmg"
        );
    }

    #[test]
    fn client_downloads_cache_freshness() {
        assert!(client_downloads_cache_fresh(std::time::Instant::now()));
        let old = std::time::Instant::now()
            - std::time::Duration::from_secs(2 * 3600);
        assert!(!client_downloads_cache_fresh(old));
    }

    // ---- relay nodes (v36) ----

    #[test]
    fn relay_token_id_known_vector() {
        // sha256("abc") is a published vector, not our own output:
        // ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(relay_token_id("abc"), "ba7816bf8f01cfea");
        assert_eq!(relay_token_id("rrt_example").len(), 16);
    }

    #[test]
    fn relay_proof_msg_exact() {
        assert_eq!(
            relay_register_proof_msg("tok123", "n0nce", "relay-1", "1.0.0", "1.2.3.4"),
            "relay-register-v1\ntok123\nn0nce\nrelay-1\n1.0.0\n1.2.3.4"
        );
        // Empty fields still produce the separators (no trailing \n).
        assert_eq!(
            relay_register_proof_msg("t", "n", "", "", ""),
            "relay-register-v1\nt\nn\n\n\n"
        );
    }

    #[test]
    fn relay_heartbeat_sig_msg_exact() {
        assert_eq!(
            relay_heartbeat_sig_msg(12, 1792000000, "ab12", "cd34", "ef56"),
            "relay-heartbeat-v1\n12\n1792000000\nab12\ncd34\nef56"
        );
    }

    #[test]
    fn relay_hmac_known_vector_and_tamper() {
        // Published HMAC-SHA256 vector (Wikipedia "HMAC" article).
        let sig = relay_hmac_hex(
            b"key",
            "The quick brown fox jumps over the lazy dog",
        )
        .unwrap();
        assert_eq!(
            sig,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
        assert!(relay_hmac_verify(
            b"key",
            "The quick brown fox jumps over the lazy dog",
            &sig
        ));
        // Tampered message, wrong key, and malformed hex all fail.
        assert!(!relay_hmac_verify(
            b"key",
            "The quick brown fox jumps over the lazy dog.",
            &sig
        ));
        assert!(!relay_hmac_verify(
            b"other",
            "The quick brown fox jumps over the lazy dog",
            &sig
        ));
        assert!(!relay_hmac_verify(
            b"key",
            "The quick brown fox jumps over the lazy dog",
            "zz"
        ));
    }

    #[test]
    fn relay_aead_roundtrip_and_tamper() {
        let key = [7u8; 32];
        let plain = b"{\"node_id\": 12, \"secret\": true}";
        let (nonce, data) = relay_seal(&key, plain).unwrap();
        assert_eq!(nonce.len(), 24);
        assert_eq!(relay_open(&key, &nonce, &data).unwrap(), plain);
        // Tampered ciphertext fails authentication.
        let mut bad = data.clone();
        bad.replace_range(0..2, if &data[0..2] == "00" { "ff" } else { "00" });
        assert!(relay_open(&key, &nonce, &bad).is_none());
        // Wrong key fails.
        assert!(relay_open(&[8u8; 32], &nonce, &data).is_none());
        // Short keys cannot seal or open.
        assert!(relay_seal(&[1u8; 16], plain).is_none());
        assert!(relay_open(&[1u8; 16], &nonce, &data).is_none());
        // Two seals of the same plaintext differ (fresh nonce).
        let (n2, d2) = relay_seal(&key, plain).unwrap();
        assert_ne!((nonce, data), (n2, d2));
    }

    #[test]
    fn relay_hex_roundtrip() {
        assert_eq!(hex_encode(&[0u8, 171, 255]), "00abff");
        assert_eq!(hex_decode("00abff").unwrap(), vec![0u8, 171, 255]);
        assert_eq!(hex_decode("00ABFF").unwrap(), vec![0u8, 171, 255]);
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
        assert!(hex_decode("").unwrap().is_empty());
    }

    #[test]
    fn relay_config_parse() {
        let cfg = parse_relay_config(&json!({"port": 21117})).unwrap();
        assert_eq!(cfg["port"], 21117);
        assert!(cfg["limit_speed"].is_null());
        assert!(cfg["total_bandwidth"].is_null());
        assert!(cfg["single_bandwidth"].is_null());
        let cfg = parse_relay_config(
            &json!({"port": 22000, "limit_speed": 16.5, "total_bandwidth": 512, "single_bandwidth": null}),
        )
        .unwrap();
        assert_eq!(cfg["port"], 22000);
        assert_eq!(cfg["limit_speed"], 16.5);
        assert_eq!(cfg["total_bandwidth"], 512.0);
        assert!(cfg["single_bandwidth"].is_null());
        assert!(parse_relay_config(&json!({"port": 0})).is_err());
        assert!(parse_relay_config(&json!({"port": 65536})).is_err());
        assert!(parse_relay_config(&json!({})).is_err());
        assert!(parse_relay_config(&json!({"port": "21117"})).is_err());
        assert!(parse_relay_config(&json!({"port": 21117, "limit_speed": -1})).is_err());
        // 0 is rejected: hbbr ignores it, so it cannot mean anything.
        assert!(parse_relay_config(&json!({"port": 21117, "limit_speed": 0})).is_err());
        assert!(parse_relay_config(&json!({"port": 21117, "limit_speed": "fast"})).is_err());
    }

    #[test]
    fn relay_default_config_values() {
        let cfg = relay_default_config();
        assert_eq!(cfg["port"], 21117);
        assert!(cfg["limit_speed"].is_null());
        assert!(cfg["total_bandwidth"].is_null());
        assert!(cfg["single_bandwidth"].is_null());
    }

    // ---- v39: admin create-user (issue #6) ----

    /// Test ApiState backed by a throwaway temp-file DB (an in-memory
    /// pool would split across connections: init_db allows 8).
    async fn v39_test_state(tag: &str) -> (Arc<ApiState>, std::path::PathBuf) {
        // PID-suffixed: two checkouts running the suite at once
        // (e.g. parallel worktrees) must not share temp DB files.
        let path = std::env::temp_dir().join(format!(
            "rdp-v39-test-{}-{tag}.sqlite3",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let pool = init_db(path.to_str().unwrap_or(""))
            .await
            .expect("init test db");
        let dir = std::env::temp_dir();
        let st = Arc::new(ApiState {
            pool,
            clients_dir: dir.join("v39-clients"),
            templates_dir: dir.join("v39-templates"),
            exe_dir: dir.clone(),
            peer_db_path: "db_v2.sqlite3".to_string(),
            key_override: String::new(),
            tpl_job: std::sync::Mutex::new(TplJob::default()),
            upd_job: std::sync::Mutex::new(UpdJob::default()),
            guard: std::sync::Mutex::new(AuthGuard::default()),
            dl_cache: std::sync::Mutex::new(None),
            reg_challenges: std::sync::Mutex::new(HashMap::new()),
            hb_replay: std::sync::Mutex::new(HashMap::new()),
        });
        (st, path)
    }

    fn v39_sid_headers(sid: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Ok(v) = format!("rdp_sid={sid}").parse::<header::HeaderValue>() {
            h.insert(header::COOKIE, v);
        }
        h
    }

    fn v39_block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(f)
    }

    async fn v39_count(pool: &SqlitePool, sql: &str, username: &str) -> i64 {
        sqlx::query(sql)
            .bind(username)
            .fetch_one(pool)
            .await
            .expect("count query")
            .try_get("c")
            .unwrap_or(-1)
    }

    async fn v39_call_create(st: &Arc<ApiState>, headers: HeaderMap, body: &str) -> StatusCode {
        admin_create_user(
            Extension(st.clone()),
            ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 54321))),
            headers,
            body.to_string(),
        )
        .await
        .status()
    }

    #[test]
    fn admin_create_user_admin_ok_and_no_session() {
        v39_block(async {
            let (st, path) = v39_test_state("ok").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            let sid = new_web_session(&st, "admin").await.expect("admin session");
            let headers = v39_sid_headers(&sid);
            let status = v39_call_create(
                &st,
                headers.clone(),
                "{\"username\":\"bob\",\"password\":\"secret123\"}",
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            // The user exists and can log in...
            assert_eq!(
                v39_count(
                    &st.pool,
                    "select count(*) as c from users where username=?",
                    "bob"
                )
                .await,
                1
            );
            assert!(matches!(
                verify_user(&st.pool, "bob", "secret123").await,
                Ok(Some(_))
            ));
            // ...but no web session was issued for the new user...
            assert_eq!(
                v39_count(
                    &st.pool,
                    "select count(*) as c from web_sessions where username=?",
                    "bob"
                )
                .await,
                0
            );
            // ...and the admin's own session still resolves.
            let u = resolve_user(&st, &headers).await;
            assert!(u.is_some());
            assert_eq!(u.map(|x| x.username).unwrap_or_default(), "admin");
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn admin_create_user_forbids_non_admin() {
        v39_block(async {
            let (st, path) = v39_test_state("nonadmin").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            create_user(&st.pool, "alice", "alice123")
                .await
                .expect("create alice");
            let sid = new_web_session(&st, "alice").await.expect("alice session");
            let status = v39_call_create(
                &st,
                v39_sid_headers(&sid),
                "{\"username\":\"bob\",\"password\":\"secret123\"}",
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(
                v39_count(
                    &st.pool,
                    "select count(*) as c from users where username=?",
                    "bob"
                )
                .await,
                0
            );
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn admin_create_user_requires_login() {
        v39_block(async {
            let (st, path) = v39_test_state("nologin").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            let status = v39_call_create(
                &st,
                HeaderMap::new(),
                "{\"username\":\"bob\",\"password\":\"secret123\"}",
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn admin_create_user_rejects_duplicate() {
        v39_block(async {
            let (st, path) = v39_test_state("dup").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            create_user(&st.pool, "bob", "secret123")
                .await
                .expect("create bob");
            let sid = new_web_session(&st, "admin").await.expect("admin session");
            let status = v39_call_create(
                &st,
                v39_sid_headers(&sid),
                "{\"username\":\"bob\",\"password\":\"otherpass\"}",
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn admin_create_user_rejects_short_password() {
        v39_block(async {
            let (st, path) = v39_test_state("shortpw").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            let sid = new_web_session(&st, "admin").await.expect("admin session");
            let status = v39_call_create(
                &st,
                v39_sid_headers(&sid),
                "{\"username\":\"bob\",\"password\":\"12345\"}",
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(
                v39_count(
                    &st.pool,
                    "select count(*) as c from users where username=?",
                    "bob"
                )
                .await,
                0
            );
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn admin_create_user_rejects_bad_username() {
        v39_block(async {
            let (st, path) = v39_test_state("badname").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            let sid = new_web_session(&st, "admin").await.expect("admin session");
            let headers = v39_sid_headers(&sid);
            // Too short.
            assert_eq!(
                v39_call_create(
                    &st,
                    headers.clone(),
                    "{\"username\":\"ab\",\"password\":\"secret123\"}",
                )
                .await,
                StatusCode::BAD_REQUEST
            );
            // Illegal characters (space and '!').
            assert_eq!(
                v39_call_create(
                    &st,
                    headers.clone(),
                    "{\"username\":\"bad name!\",\"password\":\"secret123\"}",
                )
                .await,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                v39_count(
                    &st.pool,
                    "select count(*) as c from users where username=?",
                    "ab"
                )
                .await,
                0
            );
            let _ = std::fs::remove_file(&path);
        });
    }

    // ---- v40: connection audit log (issue #7) ----

    async fn v40_post(st: &Arc<ApiState>, body: &str) -> (StatusCode, Vec<u8>) {
        let resp = audit_conn_post(Extension(st.clone()), body.to_string()).await;
        let status = resp.status();
        let bytes = hyper::body::to_bytes(resp.into_body())
            .await
            .unwrap_or_default()
            .to_vec();
        (status, bytes)
    }

    async fn v40_sessions(
        st: &Arc<ApiState>,
        headers: HeaderMap,
        q: HashMap<String, String>,
    ) -> (StatusCode, Value) {
        let resp = audit_sessions(Extension(st.clone()), headers, Query(q)).await;
        let status = resp.status();
        let bytes = hyper::body::to_bytes(resp.into_body())
            .await
            .unwrap_or_default();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    fn v40_new_body(
        device: &str,
        uuid: &str,
        conn: i64,
        nonce: &str,
        ip: &str,
    ) -> String {
        format!(
            "{{\"id\":\"{device}\",\"uuid\":\"{uuid}\",\"conn_id\":{conn},\"session_id\":\"s{conn}\",\"nonce\":\"{nonce}\",\"action\":\"new\",\"ip\":\"{ip}\"}}"
        )
    }

    fn v40_auth_body(
        device: &str,
        uuid: &str,
        conn: i64,
        nonce: &str,
        peer_id: &str,
        peer_name: &str,
    ) -> String {
        format!(
            "{{\"id\":\"{device}\",\"uuid\":\"{uuid}\",\"conn_id\":{conn},\"session_id\":\"s{conn}\",\"nonce\":\"{nonce}\",\"peer\":[\"{peer_id}\",\"{peer_name}\"],\"type\":\"remote\"}}"
        )
    }

    fn v40_close_body(device: &str, uuid: &str, conn: i64, nonce: &str) -> String {
        format!(
            "{{\"id\":\"{device}\",\"uuid\":\"{uuid}\",\"conn_id\":{conn},\"session_id\":\"s{conn}\",\"nonce\":\"{nonce}\",\"action\":\"close\"}}"
        )
    }

    async fn v40_seed_device(st: &Arc<ApiState>, id: &str, uuid: &str, user: Option<&str>) {
        upsert_device(
            st,
            id,
            uuid,
            user.map(|u| u.to_string()),
            &format!("host-{id}"),
            "windows",
            "user",
            "1005030",
            "",
        )
        .await;
    }

    #[test]
    fn audit_post_good_stores_row_and_empty_body() {
        v39_block(async {
            let (st, path) = v39_test_state("audit-ok").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            let (status, body) = v40_post(
                &st,
                &v40_new_body("111", "dXV1aWQx", 7, "n-1", "203.0.113.9"),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            // Success MUST be an empty body per the client contract.
            assert!(body.is_empty(), "body was {:?}", String::from_utf8_lossy(&body));
            let n = v39_count(&st.pool, "select count(*) as c from audit_conn where nonce=?", "n-1").await;
            assert_eq!(n, 1);
            let row = sqlx::query("select action, ip, conn_id from audit_conn where nonce=?")
                .bind("n-1")
                .fetch_one(&st.pool)
                .await
                .expect("audit row");
            assert_eq!(row.try_get::<String, _>("action").unwrap_or_default(), "new");
            assert_eq!(row.try_get::<String, _>("ip").unwrap_or_default(), "203.0.113.9");
            assert_eq!(row.try_get::<i64, _>("conn_id").unwrap_or(0), 7);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn audit_post_duplicate_nonce_idempotent() {
        v39_block(async {
            let (st, path) = v39_test_state("audit-dup").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            let body = v40_new_body("111", "dXV1aWQx", 7, "n-dup", "203.0.113.9");
            let (s1, b1) = v40_post(&st, &body).await;
            let (s2, b2) = v40_post(&st, &body).await;
            assert_eq!(s1, StatusCode::OK);
            assert_eq!(s2, StatusCode::OK);
            assert!(b1.is_empty());
            assert!(b2.is_empty());
            let n = v39_count(&st.pool, "select count(*) as c from audit_conn where nonce=?", "n-dup").await;
            assert_eq!(n, 1);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn audit_post_unknown_device_403() {
        v39_block(async {
            let (st, path) = v39_test_state("audit-403").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            // No such device at all.
            let (s, _) = v40_post(&st, &v40_new_body("999", "dXV1aWQ5", 1, "n-x1", "1.1.1.1")).await;
            assert_eq!(s, StatusCode::FORBIDDEN);
            // Known device, wrong uuid.
            let (s, _) = v40_post(&st, &v40_new_body("111", "d3Jvbmd1dWlk", 1, "n-x2", "1.1.1.1")).await;
            assert_eq!(s, StatusCode::FORBIDDEN);
            let n = v39_count(&st.pool, "select count(*) as c from audit_conn where nonce=?", "n-x1").await;
            assert_eq!(n, 0);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn audit_post_malformed_400() {
        v39_block(async {
            let (st, path) = v39_test_state("audit-400").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            let (s, _) = v40_post(&st, "not json at all").await;
            assert_eq!(s, StatusCode::BAD_REQUEST);
            let (s, _) = v40_post(&st, "{\"uuid\":\"dXV1aWQx\",\"nonce\":\"n-m1\"}").await;
            assert_eq!(s, StatusCode::BAD_REQUEST);
            let (s, _) = v40_post(&st, "{\"id\":\"111\",\"uuid\":\"dXV1aWQx\"}").await;
            assert_eq!(s, StatusCode::BAD_REQUEST);
            // Neither action nor peer: not a recognizable audit event.
            let (s, _) = v40_post(&st, "{\"id\":\"111\",\"uuid\":\"dXV1aWQx\",\"nonce\":\"n-m2\"}").await;
            assert_eq!(s, StatusCode::BAD_REQUEST);
            let n = v39_count(&st.pool, "select count(*) as c from audit_conn where nonce=?", "n-m2").await;
            assert_eq!(n, 0);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn audit_session_assembly_open_and_closed() {
        v39_block(async {
            let (st, path) = v39_test_state("audit-asm").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            v40_seed_device(&st, "222", "dXV1aWQy", Some("bob")).await;
            // Closed session: new + auth + close.
            let (s, _) = v40_post(&st, &v40_new_body("111", "dXV1aWQx", 7, "a-1", "203.0.113.9")).await;
            assert_eq!(s, StatusCode::OK);
            let (s, _) = v40_post(&st, &v40_auth_body("111", "dXV1aWQx", 7, "a-2", "222", "bob-pc")).await;
            assert_eq!(s, StatusCode::OK);
            let (s, _) = v40_post(&st, &v40_close_body("111", "dXV1aWQx", 7, "a-3")).await;
            assert_eq!(s, StatusCode::OK);
            // Open session: new only (close lost or still running).
            let (s, _) = v40_post(&st, &v40_new_body("111", "dXV1aWQx", 8, "a-4", "203.0.113.9")).await;
            assert_eq!(s, StatusCode::OK);
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            assert_eq!(rows.len(), 2);
            let closed = rows.iter().find(|r| r.conn_id == 7).expect("closed session");
            assert!(closed.ended_at.is_some());
            assert!(closed.ended_at.unwrap_or(0) >= closed.started_at);
            assert_eq!(closed.closed_by, Some("event"));
            assert_eq!(audit_session_json(closed)["closed_by"], "event");
            assert_eq!(closed.controller_id, "222");
            assert_eq!(closed.controller_name, "bob-pc");
            assert_eq!(closed.controller_username.as_deref(), Some("bob"));
            assert_eq!(closed.controlled_username.as_deref(), Some("alice"));
            assert_eq!(closed.controlled_hostname, "host-111");
            assert_eq!(closed.controller_ip, "203.0.113.9");
            assert_eq!(closed.conn_type, "remote");
            // No RELAY_SERVERS in the test env: relayed must be NULL,
            // never guessed.
            assert_eq!(closed.relayed, None);
            let open = rows.iter().find(|r| r.conn_id == 8).expect("open session");
            assert_eq!(open.ended_at, None);
            assert_eq!(open.closed_by, None);
            assert!(audit_session_json(open)["duration_sec"].is_null());
            assert!(audit_session_json(open)["ended_at"].is_null());
            assert!(audit_session_json(open)["closed_by"].is_null());
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn relay_ip_matching() {
        let set = resolve_relay_ips("203.0.113.1, 198.51.100.2:21117, [2001:db8::1]:21117");
        assert_eq!(set.len(), 3);
        let some = Some(set);
        assert_eq!(relayed_for_ip("203.0.113.1", &some), Some(true));
        assert_eq!(relayed_for_ip("198.51.100.2", &some), Some(true));
        assert_eq!(relayed_for_ip("2001:db8::1", &some), Some(true));
        // IPv4-mapped IPv6 normalizes to plain IPv4.
        assert_eq!(relayed_for_ip("::ffff:203.0.113.1", &some), Some(true));
        assert_eq!(relayed_for_ip("192.0.2.55", &some), Some(false));
        assert_eq!(relayed_for_ip("not-an-ip", &some), None);
        assert_eq!(relayed_for_ip("", &some), None);
        // Undetermined relay list: never guess.
        assert_eq!(relayed_for_ip("203.0.113.1", &None), None);
        assert!(resolve_relay_ips("").is_empty());
        assert!(resolve_relay_ips("   ").is_empty());
    }

    #[test]
    fn audit_sessions_scoping() {
        v39_block(async {
            let (st, path) = v39_test_state("audit-scope").await;
            create_user(&st.pool, "admin", "admin123").await.expect("admin");
            create_user(&st.pool, "alice", "alice123").await.expect("alice");
            create_user(&st.pool, "bob", "bob12345").await.expect("bob");
            // alice's device controlled by bob's device; bob's device
            // controlled by an unbound device; alice controls bob's.
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            v40_seed_device(&st, "222", "dXV1aWQy", Some("bob")).await;
            v40_seed_device(&st, "333", "dXV1aWQz", None).await;
            // Session 1: bob (222) -> alice's 111.
            v40_post(&st, &v40_new_body("111", "dXV1aWQx", 1, "s-1", "10.0.0.9")).await;
            v40_post(&st, &v40_auth_body("111", "dXV1aWQx", 1, "s-2", "222", "bob-pc")).await;
            // Session 2: unbound 333 -> bob's 222.
            v40_post(&st, &v40_new_body("222", "dXV1aWQy", 1, "s-3", "10.0.0.8")).await;
            v40_post(&st, &v40_auth_body("222", "dXV1aWQy", 1, "s-4", "333", "guest-pc")).await;
            // Session 3: alice (111) -> bob's 222.
            v40_post(&st, &v40_new_body("222", "dXV1aWQy", 2, "s-5", "10.0.0.7")).await;
            v40_post(&st, &v40_auth_body("222", "dXV1aWQy", 2, "s-6", "111", "alice-pc")).await;
            let admin_sid = new_web_session(&st, "admin").await.expect("admin sid");
            let alice_sid = new_web_session(&st, "alice").await.expect("alice sid");
            let bob_sid = new_web_session(&st, "bob").await.expect("bob sid");
            // Admin sees all three.
            let (s, v) = v40_sessions(&st, v39_sid_headers(&admin_sid), HashMap::new()).await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(v["total"].as_i64().unwrap_or(-1), 3);
            // Alice sees sessions 1 (controlled hers) and 3
            // (controller hers), not 2.
            let (s, v) = v40_sessions(&st, v39_sid_headers(&alice_sid), HashMap::new()).await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(v["total"].as_i64().unwrap_or(-1), 2);
            // Bob sees all three: controller of 1, controlled of 2+3.
            let (s, v) = v40_sessions(&st, v39_sid_headers(&bob_sid), HashMap::new()).await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(v["total"].as_i64().unwrap_or(-1), 3);
            // Unauthenticated: 401.
            let (s, _) = v40_sessions(&st, HeaderMap::new(), HashMap::new()).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED);
            // Search filter: only session 1 mentions bob-pc as
            // controller name... but session 2's controlled device is
            // 222/bob — search matches controller OR controlled, so
            // "bob" matches all three (bob-pc, bob's device, bob's
            // device). "guest" matches only session 2.
            let mut q = HashMap::new();
            q.insert("q".to_string(), "guest".to_string());
            let (s, v) = v40_sessions(&st, v39_sid_headers(&admin_sid), q).await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(v["total"].as_i64().unwrap_or(-1), 1);
            // Pagination: 2 per page.
            let mut q = HashMap::new();
            q.insert("per_page".to_string(), "2".to_string());
            q.insert("page".to_string(), "2".to_string());
            let (s, v) = v40_sessions(&st, v39_sid_headers(&admin_sid), q).await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(v["total"].as_i64().unwrap_or(-1), 3);
            assert_eq!(v["sessions"].as_array().map(|a| a.len()).unwrap_or(0), 1);
            let _ = std::fs::remove_file(&path);
        });
    }

    // ---------------- v43: audit reaping + relay inference (issue #11) --------

    async fn v43_seed_audit(
        pool: &SqlitePool,
        device: &str,
        uuid: &str,
        conn: i64,
        nonce: &str,
        action: &str,
        ip: &str,
        at: i64,
    ) {
        sqlx::query(
            "insert into audit_conn(nonce, device_id, device_uuid, conn_id, session_id, action, ip, received_at) values(?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(nonce)
        .bind(device)
        .bind(uuid)
        .bind(conn)
        .bind(format!("s{conn}"))
        .bind(action)
        .bind(ip)
        .bind(at)
        .execute(pool)
        .await
        .expect("seed audit");
    }

    async fn v43_seed_stale(pool: &SqlitePool, device: &str, conn: i64, ended_at: i64) {
        sqlx::query(
            "insert or ignore into audit_session_close(device_id, conn_id, ended_at, reason) values(?, ?, ?, 'stale')",
        )
        .bind(device)
        .bind(conn)
        .bind(ended_at)
        .execute(pool)
        .await
        .expect("seed stale");
    }

    async fn v43_stale_count(pool: &SqlitePool, device: &str, conn: i64) -> i64 {
        sqlx::query("select count(*) as c from audit_session_close where device_id=? and conn_id=?")
            .bind(device)
            .bind(conn)
            .fetch_one(pool)
            .await
            .expect("stale count")
            .try_get("c")
            .unwrap_or(-1)
    }

    async fn v43_close_count(pool: &SqlitePool, device: &str, conn: i64) -> i64 {
        sqlx::query(
            "select count(*) as c from audit_conn where device_id=? and conn_id=? and action='close'",
        )
        .bind(device)
        .bind(conn)
        .fetch_one(pool)
        .await
        .expect("close count")
        .try_get("c")
        .unwrap_or(-1)
    }

    async fn v43_set_last_seen(pool: &SqlitePool, device: &str, at: i64) {
        sqlx::query("update devices set last_seen=? where device_id=?")
            .bind(at)
            .bind(device)
            .execute(pool)
            .await
            .expect("set last_seen");
    }

    #[test]
    fn audit_reap_stale_session_on_heartbeat() {
        v39_block(async {
            let (st, path) = v39_test_state("v43-reap").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            let t = now();
            // Pre-existing stuck session: last event an hour ago, no
            // close — the yguangg case. A second old session whose
            // conn_id is still live must NOT be reaped.
            v43_seed_audit(&st.pool, "111", "dXV1aWQx", 7, "r-1", "new", "192.0.2.55", t - 3600)
                .await;
            v43_seed_audit(&st.pool, "111", "dXV1aWQx", 8, "r-2", "new", "192.0.2.55", t - 3600)
                .await;
            reap_audit_sessions(&st.pool, "111", &[8], t).await;
            assert_eq!(v43_stale_count(&st.pool, "111", 7).await, 1);
            assert_eq!(v43_stale_count(&st.pool, "111", 8).await, 0);
            // No close rows fabricated in the raw table.
            assert_eq!(v43_close_count(&st.pool, "111", 7).await, 0);
            // Reaping twice is a no-op (NOT EXISTS excludes it).
            reap_audit_sessions(&st.pool, "111", &[8], t).await;
            assert_eq!(v43_stale_count(&st.pool, "111", 7).await, 1);
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            let dead = rows.iter().find(|r| r.conn_id == 7).expect("reaped session");
            assert_eq!(dead.ended_at, Some(t));
            assert_eq!(dead.closed_by, Some("stale"));
            assert_eq!(audit_session_json(dead)["closed_by"], "stale");
            // Duration stops growing: ended - started is fixed.
            assert_eq!(
                audit_session_json(dead)["duration_sec"].as_i64().unwrap_or(-1),
                3600
            );
            let live = rows.iter().find(|r| r.conn_id == 8).expect("live session");
            assert_eq!(live.ended_at, None);
            assert_eq!(live.closed_by, None);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn audit_reap_grace_period_keeps_fresh_session() {
        v39_block(async {
            let (st, path) = v39_test_state("v43-grace").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            let t = now();
            // Fresh session (10 s old): heartbeat without the conn_id
            // must NOT reap it — the race the grace period covers.
            v43_seed_audit(&st.pool, "111", "dXV1aWQx", 7, "g-1", "new", "192.0.2.55", t - 10)
                .await;
            reap_audit_sessions(&st.pool, "111", &[], t).await;
            assert_eq!(v43_stale_count(&st.pool, "111", 7).await, 0);
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            let open = rows.iter().find(|r| r.conn_id == 7).expect("fresh session");
            assert_eq!(open.ended_at, None);
            assert_eq!(open.closed_by, None);
            // Past the grace period, the same heartbeat reaps it.
            reap_audit_sessions(&st.pool, "111", &[], t + 120).await;
            assert_eq!(v43_stale_count(&st.pool, "111", 7).await, 1);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn audit_stale_reuse_guard_ignores_old_record() {
        v39_block(async {
            let (st, path) = v39_test_state("v43-reuse").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            let t = now();
            // Old session reaped at t-1800; the client restarted and
            // a NEW session reused conn_id 7, with events after the
            // reaping — the stale record must not close the new one.
            v43_seed_audit(&st.pool, "111", "dXV1aWQx", 7, "u-1", "new", "192.0.2.55", t - 3600)
                .await;
            v43_seed_stale(&st.pool, "111", 7, t - 1800).await;
            v43_seed_audit(&st.pool, "111", "dXV1aWQx", 7, "u-2", "new", "192.0.2.55", t - 60)
                .await;
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].ended_at, None);
            assert_eq!(rows[0].closed_by, None);
            // And a stale record that postdates the last event DOES
            // apply (same setup, reaped after the last event).
            sqlx::query("update audit_session_close set ended_at=? where device_id=? and conn_id=?")
                .bind(t - 30)
                .bind("111")
                .bind(7)
                .execute(&st.pool)
                .await
                .expect("rewrite stale");
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            assert_eq!(rows[0].ended_at, Some(t - 30));
            assert_eq!(rows[0].closed_by, Some("stale"));
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn audit_reap_deletes_stale_on_reappearance() {
        v39_block(async {
            let (st, path) = v39_test_state("v43-reappear").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            let t = now();
            v43_seed_audit(&st.pool, "111", "dXV1aWQx", 7, "p-1", "new", "192.0.2.55", t - 3600)
                .await;
            v43_seed_stale(&st.pool, "111", 7, t - 60).await;
            assert_eq!(v43_stale_count(&st.pool, "111", 7).await, 1);
            // conn_id 7 is live again: the stale record is deleted.
            reap_audit_sessions(&st.pool, "111", &[7], t).await;
            assert_eq!(v43_stale_count(&st.pool, "111", 7).await, 0);
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            let open = rows.iter().find(|r| r.conn_id == 7).expect("session");
            assert_eq!(open.ended_at, None);
            assert_eq!(open.closed_by, None);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn audit_offline_rule_ends_silent_device_session() {
        v39_block(async {
            let (st, path) = v39_test_state("v43-offline").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            v40_seed_device(&st, "222", "dXV1aWQy", Some("bob")).await;
            let t = now();
            // Device 111 silent 25 h: its open session is dead, ended
            // at max(last_seen, started_at) = last_seen here.
            v43_seed_audit(&st.pool, "111", "dXV1aWQx", 7, "o-1", "new", "192.0.2.55", t - 26 * 3600)
                .await;
            v43_set_last_seen(&st.pool, "111", t - 25 * 3600).await;
            // Device 222 seen 1 h ago: its open session stays open.
            v43_seed_audit(&st.pool, "222", "dXV1aWQy", 7, "o-2", "new", "192.0.2.55", t - 3600)
                .await;
            v43_set_last_seen(&st.pool, "222", t - 3600).await;
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            let dead = rows
                .iter()
                .find(|r| r.device_id == "111")
                .expect("offline session");
            assert_eq!(dead.ended_at, Some(t - 25 * 3600));
            assert_eq!(dead.closed_by, Some("offline"));
            assert_eq!(audit_session_json(dead)["closed_by"], "offline");
            let live = rows
                .iter()
                .find(|r| r.device_id == "222")
                .expect("recent session");
            assert_eq!(live.ended_at, None);
            assert_eq!(live.closed_by, None);
            // The offline rule writes nothing: no stale rows appear.
            assert_eq!(v43_stale_count(&st.pool, "111", 7).await, 0);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn heartbeat_conns_parsing() {
        assert!(heartbeat_conns(&json!({"id": "111"})).is_empty());
        assert_eq!(heartbeat_conns(&json!({"conns": [1, 2]})), vec![1, 2]);
        assert!(heartbeat_conns(&json!({"conns": []})).is_empty());
        // Non-number entries are ignored; a non-array is empty.
        assert_eq!(
            heartbeat_conns(&json!({"conns": [1, "x", null, 3]})),
            vec![1, 3]
        );
        assert!(heartbeat_conns(&json!({"conns": 7})).is_empty());
    }

    #[test]
    fn heartbeat_conns_stored() {
        v39_block(async {
            let (st, path) = v39_test_state("v43-store").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            // Fresh rows carry the migration default.
            let row = sqlx::query("select conns, conns_updated_at from devices where device_id=?")
                .bind("111")
                .fetch_one(&st.pool)
                .await
                .expect("device row");
            assert_eq!(row.try_get::<String, _>("conns").unwrap_or_default(), "[]");
            assert_eq!(row.try_get::<i64, _>("conns_updated_at").unwrap_or(-1), 0);
            let t = now();
            store_heartbeat_conns(&st.pool, "111", "dXV1aWQx", &[7, 8], t).await;
            let row = sqlx::query("select conns, conns_updated_at from devices where device_id=?")
                .bind("111")
                .fetch_one(&st.pool)
                .await
                .expect("device row");
            assert_eq!(row.try_get::<String, _>("conns").unwrap_or_default(), "[7,8]");
            assert_eq!(row.try_get::<i64, _>("conns_updated_at").unwrap_or(-1), t);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn relay_ips_union_public_host() {
        // public_host-only: no --relay-servers at all.
        let set = relay_ips_union("", "203.0.113.1");
        assert_eq!(set.len(), 1);
        assert!(set.contains(&"203.0.113.1".parse().unwrap()));
        // Both empty: empty set, never guessed.
        assert!(relay_ips_union("", "").is_empty());
        assert!(relay_ips_union("   ", "").is_empty());
        // Union of both sources; a :port suffix on public_host is
        // stripped (host_part).
        let set = relay_ips_union("203.0.113.1", "198.51.100.2:21116");
        assert_eq!(set.len(), 2);
        assert!(set.contains(&"203.0.113.1".parse().unwrap()));
        assert!(set.contains(&"198.51.100.2".parse().unwrap()));
        // Bracketed IPv6 with a port.
        let set = relay_ips_union("", "[2001:db8::1]:21116");
        assert_eq!(set.len(), 1);
        assert!(set.contains(&"2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn audit_relay_inference_public_host() {
        v39_block(async {
            let (st, path) = v39_test_state("v43-relay").await;
            v40_seed_device(&st, "111", "dXV1aWQx", Some("alice")).await;
            v40_post(&st, &v40_new_body("111", "dXV1aWQx", 7, "y-1", "203.0.113.1")).await;
            v40_post(&st, &v40_close_body("111", "dXV1aWQx", 7, "y-2")).await;
            v40_post(&st, &v40_new_body("111", "dXV1aWQx", 8, "y-3", "192.0.2.55")).await;
            v40_post(&st, &v40_close_body("111", "dXV1aWQx", 8, "y-4")).await;
            // No --relay-servers in the test env and no public_host:
            // relayed is null, never guessed.
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().all(|r| r.relayed.is_none()));
            // public_host set to the relay IP: the session whose
            // new-event IP matches is relayed, the other is direct.
            set_setting(&st.pool, "public_host", "203.0.113.1")
                .await
                .expect("set public_host");
            let rows = audit_session_rows(&st.pool, None).await.expect("assemble");
            let relayed = rows.iter().find(|r| r.conn_id == 7).expect("relayed");
            assert_eq!(relayed.relayed, Some(true));
            let direct = rows.iter().find(|r| r.conn_id == 8).expect("direct");
            assert_eq!(direct.relayed, Some(false));
            let _ = std::fs::remove_file(&path);
        });
    }

    // ---------------- v40.1: address book auto-add bound devices --------

    fn v401_device(id: &str, host: &str, os: &str, user: &str) -> BoundDevice {
        BoundDevice {
            device_id: id.to_string(),
            hostname: host.to_string(),
            os: os.to_string(),
            os_username: user.to_string(),
        }
    }

    #[test]
    fn ab_platform_mapping() {
        assert_eq!(ab_platform_from_os("windows / Windows 10 Pro"), "Windows");
        assert_eq!(ab_platform_from_os("Windows 11"), "Windows");
        assert_eq!(ab_platform_from_os("ubuntu / Ubuntu 22.04.5 LTS"), "Linux");
        assert_eq!(ab_platform_from_os("debian / Debian GNU/Linux 12"), "Linux");
        assert_eq!(ab_platform_from_os("macos / macOS 14.5"), "Mac OS");
        assert_eq!(ab_platform_from_os("darwin / Darwin 23.0"), "Mac OS");
        assert_eq!(ab_platform_from_os("android / Android 16"), "Android");
        assert_eq!(ab_platform_from_os("ios / iOS 18.0"), "iOS");
        assert_eq!(
            ab_platform_from_os("freebsd / FreeBSD 14"),
            "freebsd / FreeBSD 14"
        );
        assert_eq!(ab_platform_from_os(""), "");
    }

    #[test]
    fn ab_merge_adds_devices_and_tag() {
        let blob = r#"{"tags":["work"],"peers":[{"id":"999","alias":"mine","forceAlwaysRelay":"true","rdpPort":"3389","extra_field":1}]}"#;
        let devices = vec![
            v401_device("111", "pc-1", "windows / Windows 10 Pro", "alice"),
            v401_device("222", "pc-2", "ubuntu / Ubuntu 22.04", "bob"),
        ];
        let merged = ab_merge_bound_devices(blob, &devices).expect("merged");
        let v: Value = serde_json::from_str(&merged).expect("json");
        let tags = v["tags"].as_array().expect("tags");
        assert!(tags.iter().any(|t| t.as_str() == Some("work")));
        assert!(tags.iter().any(|t| t.as_str() == Some("登录设备")));
        // Existing peer untouched, all its fields intact.
        let peers = v["peers"].as_array().expect("peers");
        assert_eq!(peers.len(), 3);
        let mine = &peers[0];
        assert_eq!(mine["id"].as_str(), Some("999"));
        assert_eq!(mine["alias"].as_str(), Some("mine"));
        assert_eq!(mine["forceAlwaysRelay"].as_str(), Some("true"));
        assert_eq!(mine["rdpPort"].as_str(), Some("3389"));
        assert_eq!(mine["extra_field"].as_i64(), Some(1));
        // Added peers carry device fields + the tag.
        let p1 = &peers[1];
        assert_eq!(p1["id"].as_str(), Some("111"));
        assert_eq!(p1["hostname"].as_str(), Some("pc-1"));
        assert_eq!(p1["username"].as_str(), Some("alice"));
        assert_eq!(p1["platform"].as_str(), Some("Windows"));
        assert_eq!(p1["hash"].as_str(), Some(""));
        assert_eq!(p1["password"].as_str(), Some(""));
        assert_eq!(p1["forceAlwaysRelay"].as_str(), Some("false"));
        assert!(p1["same_server"].is_null());
        assert_eq!(p1["tags"].as_array().map(|a| a.len()).unwrap_or(0), 1);
        assert_eq!(p1["tags"][0].as_str(), Some("登录设备"));
        assert_eq!(peers[2]["platform"].as_str(), Some("Linux"));
    }

    #[test]
    fn ab_merge_dedup_by_id() {
        // The user's own entry for a bound device wins as-is: no tag
        // added to it, no field touched, no duplicate peer.
        let blob = r#"{"tags":[],"peers":[{"id":"111","alias":"renamed","tags":["work"],"password":"secret"}]}"#;
        let devices = vec![v401_device("111", "pc-1", "windows", "alice")];
        let merged = ab_merge_bound_devices(blob, &devices).expect("merged");
        let v: Value = serde_json::from_str(&merged).expect("json");
        let peers = v["peers"].as_array().expect("peers");
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0]["alias"].as_str(), Some("renamed"));
        assert_eq!(peers[0]["password"].as_str(), Some("secret"));
        let tags = peers[0]["tags"].as_array().expect("peer tags");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].as_str(), Some("work"));
    }

    #[test]
    fn ab_merge_malformed_passthrough() {
        let devices = vec![v401_device("111", "pc-1", "windows", "alice")];
        // Not JSON at all.
        assert_eq!(ab_merge_bound_devices("not json{", &devices), None);
        // JSON but not an object.
        assert_eq!(ab_merge_bound_devices("[1,2]", &devices), None);
        // No devices: verbatim, no tag added.
        let blob = r#"{"tags":[],"peers":[]}"#;
        assert_eq!(ab_merge_bound_devices(blob, &[]).as_deref(), Some(blob));
        // Blob without peers/tags keys: the keys are created.
        let merged = ab_merge_bound_devices("{}", &devices).expect("merged");
        let v: Value = serde_json::from_str(&merged).expect("json");
        assert_eq!(v["peers"].as_array().map(|a| a.len()).unwrap_or(0), 1);
        assert!(v["tags"]
            .as_array()
            .map(|a| a.iter().any(|t| t.as_str() == Some("登录设备")))
            .unwrap_or(false));
    }

    #[test]
    fn ab_strip_managed_peers_cases() {
        // Tagged peer stripped, the rest kept.
        let data = r#"{"tags":["登录设备"],"peers":[{"id":"111","tags":["登录设备"]},{"id":"999","tags":["work"]}]}"#;
        let stripped = ab_strip_managed_peers(data);
        let v: Value = serde_json::from_str(&stripped).expect("json");
        let peers = v["peers"].as_array().expect("peers");
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0]["id"].as_str(), Some("999"));
        // Nothing tagged: the exact input string comes back.
        let plain = r#"{"tags":[],"peers":[{"id":"1"}]}"#;
        assert_eq!(ab_strip_managed_peers(plain), plain);
        // Not JSON: verbatim.
        assert_eq!(ab_strip_managed_peers("garbage{"), "garbage{");
    }

    fn v401_bearer_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().expect("bearer"),
        );
        h
    }

    async fn v401_ab_get(st: &Arc<ApiState>, headers: HeaderMap) -> (StatusCode, Vec<u8>) {
        let resp = ab_get(Extension(st.clone()), headers).await;
        let status = resp.status();
        let bytes = hyper::body::to_bytes(resp.into_body())
            .await
            .unwrap_or_default()
            .to_vec();
        (status, bytes)
    }

    async fn v401_ab_post(st: &Arc<ApiState>, headers: HeaderMap, body: &str) -> StatusCode {
        let resp = ab_post(Extension(st.clone()), headers, body.to_string()).await;
        resp.status()
    }

    async fn v401_seed_device(
        st: &Arc<ApiState>,
        id: &str,
        user: &str,
        host: &str,
        os: &str,
        os_user: &str,
    ) {
        upsert_device(
            st,
            id,
            &format!("uuid-{id}"),
            Some(user.to_string()),
            host,
            os,
            os_user,
            "1005030",
            "",
        )
        .await;
    }

    #[test]
    fn ab_get_no_blob_synthesizes_or_null() {
        v39_block(async {
            let (st, path) = v39_test_state("ab-noblob").await;
            create_user(&st.pool, "alice", "alice123").await.expect("alice");
            create_user(&st.pool, "bob", "bob12345").await.expect("bob");
            let alice_tok = issue_client_token(&st, "alice", "1", "u1")
                .await
                .expect("tok");
            let bob_tok = issue_client_token(&st, "bob", "2", "u2").await.expect("tok");
            v401_seed_device(&st, "111", "alice", "alice-pc", "windows / Windows 10", "alice")
                .await;
            // Alice: no blob, one bound device -> synthesized blob.
            let (s, body) = v401_ab_get(&st, v401_bearer_headers(&alice_tok)).await;
            assert_eq!(s, StatusCode::OK);
            let v: Value = serde_json::from_slice(&body).expect("json");
            let blob: Value =
                serde_json::from_str(v["data"].as_str().unwrap_or("")).expect("blob");
            assert_eq!(blob["peers"].as_array().map(|a| a.len()).unwrap_or(0), 1);
            assert_eq!(blob["peers"][0]["id"].as_str(), Some("111"));
            assert_eq!(blob["peers"][0]["hostname"].as_str(), Some("alice-pc"));
            assert_eq!(blob["peers"][0]["platform"].as_str(), Some("Windows"));
            // Bob: no blob, no devices -> literal "null".
            let (s, body) = v401_ab_get(&st, v401_bearer_headers(&bob_tok)).await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(String::from_utf8_lossy(&body), "null");
            // Unauthenticated: 401.
            let (s, _) = v401_ab_get(&st, HeaderMap::new()).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn ab_handlers_merge_strip_and_scope() {
        v39_block(async {
            let (st, path) = v39_test_state("ab-handlers").await;
            create_user(&st.pool, "alice", "alice123").await.expect("alice");
            create_user(&st.pool, "bob", "bob12345").await.expect("bob");
            let alice_tok = issue_client_token(&st, "alice", "1", "u1")
                .await
                .expect("tok");
            let bob_tok = issue_client_token(&st, "bob", "2", "u2").await.expect("tok");
            v401_seed_device(&st, "111", "alice", "alice-pc", "windows", "alice").await;
            v401_seed_device(&st, "222", "bob", "bob-pc", "ubuntu", "bob").await;
            // Alice posts a blob containing a stale managed peer (111,
            // as if synced from an older GET) plus her own peer.
            let posted = r#"{"tags":["登录设备","work"],"peers":[{"id":"111","tags":["登录设备"],"hostname":"stale"},{"id":"999","tags":["work"],"alias":"mine"}]}"#;
            let body = serde_json::to_string(&json!({ "data": posted })).expect("body");
            let s = v401_ab_post(&st, v401_bearer_headers(&alice_tok), &body).await;
            assert_eq!(s, StatusCode::OK);
            // GET: the stale managed peer is gone, her own peer is
            // intact, and 111 is re-added fresh from binding state.
            let (s, resp) = v401_ab_get(&st, v401_bearer_headers(&alice_tok)).await;
            assert_eq!(s, StatusCode::OK);
            let v: Value = serde_json::from_slice(&resp).expect("json");
            let blob: Value =
                serde_json::from_str(v["data"].as_str().unwrap_or("")).expect("blob");
            let peers = blob["peers"].as_array().expect("peers");
            assert_eq!(peers.len(), 2);
            let mine = peers
                .iter()
                .find(|p| p["id"].as_str() == Some("999"))
                .expect("mine");
            assert_eq!(mine["alias"].as_str(), Some("mine"));
            let auto = peers
                .iter()
                .find(|p| p["id"].as_str() == Some("111"))
                .expect("auto");
            assert_eq!(auto["hostname"].as_str(), Some("alice-pc"));
            assert_ne!(auto["hostname"].as_str(), Some("stale"));
            // Bob sees none of Alice's devices.
            let (s, resp) = v401_ab_get(&st, v401_bearer_headers(&bob_tok)).await;
            assert_eq!(s, StatusCode::OK);
            let v: Value = serde_json::from_slice(&resp).expect("json");
            let blob: Value =
                serde_json::from_str(v["data"].as_str().unwrap_or("")).expect("blob");
            let peers = blob["peers"].as_array().expect("peers");
            assert_eq!(peers.len(), 1);
            assert_eq!(peers[0]["id"].as_str(), Some("222"));
            let _ = std::fs::remove_file(&path);
        });
    }

    // ---- issue #9: self-service password change ----

    async fn v41_call_change(
        st: &Arc<ApiState>,
        headers: HeaderMap,
        body: &str,
    ) -> (StatusCode, Value) {
        let resp = change_password(
            Extension(st.clone()),
            ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 54321))),
            headers,
            body.to_string(),
        )
        .await;
        let status = resp.status();
        let bytes = hyper::body::to_bytes(resp.into_body())
            .await
            .unwrap_or_default();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    #[test]
    fn pwd_change_ok_updates_hash_and_revokes_other_sessions() {
        v39_block(async {
            let (st, path) = v39_test_state("pwd-ok").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            create_user(&st.pool, "alice", "alice123")
                .await
                .expect("create alice");
            create_user(&st.pool, "bob", "bob12345")
                .await
                .expect("create bob");
            let sid_cur = new_web_session(&st, "alice").await.expect("alice session 1");
            let sid_other = new_web_session(&st, "alice").await.expect("alice session 2");
            let (status, v) = v41_call_change(
                &st,
                v39_sid_headers(&sid_cur),
                "{\"old_password\":\"alice123\",\"new_password\":\"alice456\"}",
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(v["ok"], true);
            // New password works, old one does not.
            assert!(matches!(
                verify_user(&st.pool, "alice", "alice456").await,
                Ok(Some(_))
            ));
            assert!(matches!(
                verify_user(&st.pool, "alice", "alice123").await,
                Ok(None)
            ));
            // Current session survives, the other one is revoked.
            assert_eq!(
                v39_count(
                    &st.pool,
                    "select count(*) as c from web_sessions where token=?",
                    sid_cur.as_str()
                )
                .await,
                1
            );
            assert_eq!(
                v39_count(
                    &st.pool,
                    "select count(*) as c from web_sessions where token=?",
                    sid_other.as_str()
                )
                .await,
                0
            );
            // Another user's password is untouched.
            assert!(matches!(
                verify_user(&st.pool, "bob", "bob12345").await,
                Ok(Some(_))
            ));
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn pwd_change_wrong_old_rejected() {
        v39_block(async {
            let (st, path) = v39_test_state("pwd-wrong").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            create_user(&st.pool, "alice", "alice123")
                .await
                .expect("create alice");
            let sid = new_web_session(&st, "alice").await.expect("alice session");
            let (status, v) = v41_call_change(
                &st,
                v39_sid_headers(&sid),
                "{\"old_password\":\"not-the-password\",\"new_password\":\"alice456\"}",
            )
            .await;
            // 400, not 401: the caller IS authenticated; the console's
            // api() helper would redirect to login on a 401.
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(v["error"], "旧密码不正确");
            assert!(matches!(
                verify_user(&st.pool, "alice", "alice123").await,
                Ok(Some(_))
            ));
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn pwd_change_short_new_rejected() {
        v39_block(async {
            let (st, path) = v39_test_state("pwd-short").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            create_user(&st.pool, "alice", "alice123")
                .await
                .expect("create alice");
            let sid = new_web_session(&st, "alice").await.expect("alice session");
            let (status, v) = v41_call_change(
                &st,
                v39_sid_headers(&sid),
                "{\"old_password\":\"alice123\",\"new_password\":\"abc\"}",
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(v["error"], "密码至少 6 位");
            assert!(matches!(
                verify_user(&st.pool, "alice", "alice123").await,
                Ok(Some(_))
            ));
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn pwd_change_unauthenticated_401() {
        v39_block(async {
            let (st, path) = v39_test_state("pwd-401").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            let (status, _) = v41_call_change(
                &st,
                HeaderMap::new(),
                "{\"old_password\":\"admin123\",\"new_password\":\"admin456\"}",
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn pwd_change_lockout_after_failures() {
        v39_block(async {
            let (st, path) = v39_test_state("pwd-lock").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            create_user(&st.pool, "alice", "alice123")
                .await
                .expect("create alice");
            let sid = new_web_session(&st, "alice").await.expect("alice session");
            for _ in 0..5 {
                let (status, _) = v41_call_change(
                    &st,
                    v39_sid_headers(&sid),
                    "{\"old_password\":\"wrong\",\"new_password\":\"alice456\"}",
                )
                .await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
            }
            // The 5th failure engaged the account lock: even the
            // correct old password is turned away now.
            let (status, _) = v41_call_change(
                &st,
                v39_sid_headers(&sid),
                "{\"old_password\":\"alice123\",\"new_password\":\"alice456\"}",
            )
            .await;
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn pwd_change_success_clears_failures() {
        v39_block(async {
            let (st, path) = v39_test_state("pwd-clear").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            create_user(&st.pool, "alice", "alice123")
                .await
                .expect("create alice");
            let sid = new_web_session(&st, "alice").await.expect("alice session");
            for _ in 0..4 {
                let (status, _) = v41_call_change(
                    &st,
                    v39_sid_headers(&sid),
                    "{\"old_password\":\"wrong\",\"new_password\":\"alice456\"}",
                )
                .await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
            }
            let (status, _) = v41_call_change(
                &st,
                v39_sid_headers(&sid),
                "{\"old_password\":\"alice123\",\"new_password\":\"alice456\"}",
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            // Without the success clearing the counter, these 4 more
            // failures (8 total) would have engaged the lock at the
            // 5th; the change below would then be 429, not 200.
            for _ in 0..4 {
                let (status, _) = v41_call_change(
                    &st,
                    v39_sid_headers(&sid),
                    "{\"old_password\":\"wrong\",\"new_password\":\"alice789\"}",
                )
                .await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
            }
            let (status, _) = v41_call_change(
                &st,
                v39_sid_headers(&sid),
                "{\"old_password\":\"alice456\",\"new_password\":\"alice789\"}",
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let _ = std::fs::remove_file(&path);
        });
    }

    // ---- v44: token rename ----

    async fn v44_call_create(
        st: &Arc<ApiState>,
        headers: HeaderMap,
        body: &str,
    ) -> (StatusCode, Value) {
        let resp = tokens_create(Extension(st.clone()), headers, body.to_string()).await;
        let status = resp.status();
        let bytes = hyper::body::to_bytes(resp.into_body())
            .await
            .unwrap_or_default();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    async fn v44_call_rename(
        st: &Arc<ApiState>,
        headers: HeaderMap,
        id: i64,
        body: &str,
    ) -> (StatusCode, Value) {
        let resp = tokens_rename(Extension(st.clone()), headers, Path(id), body.to_string()).await;
        let status = resp.status();
        let bytes = hyper::body::to_bytes(resp.into_body())
            .await
            .unwrap_or_default();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    async fn v44_token_name(pool: &SqlitePool, id: i64) -> String {
        sqlx::query("select name from api_tokens where id=?")
            .bind(id)
            .fetch_one(pool)
            .await
            .ok()
            .and_then(|r| r.try_get::<String, _>("name").ok())
            .unwrap_or_default()
    }

    #[test]
    fn token_create_returns_id_and_rename_updates_name() {
        v39_block(async {
            let (st, path) = v39_test_state("tok-rename").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            let sid = new_web_session(&st, "admin").await.expect("admin session");
            let headers = v39_sid_headers(&sid);
            let (status, v) = v44_call_create(&st, headers.clone(), "{\"name\":\"ci\"}").await;
            assert_eq!(status, StatusCode::OK);
            let id = v["id"].as_i64().unwrap_or(0);
            assert!(id > 0);
            let (status, _) =
                v44_call_rename(&st, headers.clone(), id, "{\"name\":\"renamed\"}").await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(v44_token_name(&st.pool, id).await, "renamed");
            // Empty name falls back to "default", same as create.
            let (status, _) = v44_call_rename(&st, headers, id, "{\"name\":\"\"}").await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(v44_token_name(&st.pool, id).await, "default");
            let _ = std::fs::remove_file(&path);
        });
    }

    #[test]
    fn token_rename_owner_only_and_not_found() {
        v39_block(async {
            let (st, path) = v39_test_state("tok-rename-own").await;
            create_user(&st.pool, "admin", "admin123")
                .await
                .expect("create admin");
            create_user(&st.pool, "alice", "alice123")
                .await
                .expect("create alice");
            let admin_sid = new_web_session(&st, "admin").await.expect("admin session");
            let alice_sid = new_web_session(&st, "alice").await.expect("alice session");
            let (_, v) = v44_call_create(
                &st,
                v39_sid_headers(&admin_sid),
                "{\"name\":\"admins\"}",
            )
            .await;
            let id = v["id"].as_i64().unwrap_or(0);
            assert!(id > 0);
            // Another user's token: 404, name untouched.
            let (status, _) = v44_call_rename(
                &st,
                v39_sid_headers(&alice_sid),
                id,
                "{\"name\":\"stolen\"}",
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(v44_token_name(&st.pool, id).await, "admins");
            // Nonexistent id: 404.
            let (status, _) = v44_call_rename(
                &st,
                v39_sid_headers(&admin_sid),
                id + 1000,
                "{\"name\":\"ghost\"}",
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            // No session: 401.
            let (status, _) =
                v44_call_rename(&st, HeaderMap::new(), id, "{\"name\":\"anon\"}").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(v44_token_name(&st.pool, id).await, "admins");
            let _ = std::fs::remove_file(&path);
        });
    }

}
