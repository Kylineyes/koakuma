//! 内置轻量 HTTP 服务：数据查询接口、静态页面与可选的登录鉴权。
//!
//! 设计：
//! - 静态资源在编译期内嵌进二进制（include_str!），部署时无需额外文件；
//! - Web 线程使用独立的只读连接查询数据库，WAL 模式下与主线程的写入互不阻塞；
//! - 若启用登录鉴权，则 /api/* 数据接口需携带有效会话 Cookie；静态资源与 / 始终可访问，
//!   以便前端先加载登录弹窗。

use std::collections::HashSet;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Response, Server};

use crate::db;

type Resp = Response<Cursor<Vec<u8>>>;
type Sessions = Arc<Mutex<HashSet<String>>>;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE_CSS: &str = include_str!("../web/style.css");
const CHART_JS: &str = include_str!("../web/vendor/chart.umd.min.js");

const SESSION_COOKIE: &str = "koakuma_session";

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn respond_body(content_type: &'static str, status: u16, body: &str) -> Resp {
    Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
}

fn respond_body_headers(content_type: &'static str, status: u16, body: &str, extra: Vec<Header>) -> Resp {
    let mut resp = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap());
    for h in extra {
        resp = resp.with_header(h);
    }
    resp
}

fn json_resp(status: u16, value: &Value) -> Resp {
    respond_body("application/json; charset=utf-8", status, &value.to_string())
}

fn json_resp_headers(status: u16, value: &Value, extra: Vec<Header>) -> Resp {
    respond_body_headers("application/json; charset=utf-8", status, &value.to_string(), extra)
}

fn query_params(url: &str) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    if let Some(q) = url.split('?').nth(1) {
        for pair in q.split('&') {
            let mut it = pair.splitn(2, '=');
            let k = it.next().unwrap_or("");
            let v = it.next().unwrap_or("");
            if !k.is_empty() {
                m.insert(k.to_string(), v.to_string());
            }
        }
    }
    m
}

/// 启动内置 HTTP 服务（在独立线程中运行）。
pub fn serve(
    db_path: String,
    listen_addr: String,
    auth_enabled: bool,
    auth_user: String,
    auth_pass: String,
) {
    let sessions: Sessions = Arc::new(Mutex::new(HashSet::new()));
    thread::spawn(move || {
        let server = match Server::http(listen_addr.as_str()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[web] 监听 {} 失败：{}", listen_addr, e);
                return;
            }
        };
        if auth_enabled {
            eprintln!("[web] HTTP 服务已启动：http://{}（已启用登录鉴权）", listen_addr);
        } else {
            eprintln!("[web] HTTP 服务已启动：http://{}", listen_addr);
        }
        for mut request in server.incoming_requests() {
            let client_ip = request.remote_addr().map(|a| normalize_ip(&a.ip().to_string()));
            let is_post = matches!(request.method(), Method::Post);
            let method = if is_post { "POST" } else { "GET" };
            let url = request.url().to_string();
            let cookie = header_value(&request, "Cookie");
            let mut body = String::new();
            if is_post {
                let _ = request.as_reader().read_to_string(&mut body);
            }
            let resp = handle(
                method,
                &url,
                &body,
                cookie.as_deref(),
                client_ip.as_deref(),
                &db_path,
                auth_enabled,
                &auth_user,
                &auth_pass,
                &sessions,
            );
            let _ = request.respond(resp);
        }
    });
}

fn header_value(req: &tiny_http::Request, name: &str) -> Option<String> {
    for h in req.headers() {
        if h.field.as_str().as_str().eq_ignore_ascii_case(name) {
            return Some(h.value.as_str().to_string());
        }
    }
    None
}

/// 归一化 IP 字符串（去除 IPv4-mapped IPv6 前缀）。
fn normalize_ip(s: &str) -> String {
    s.strip_prefix("::ffff:").unwrap_or(s).to_string()
}

#[allow(clippy::too_many_arguments)]
fn handle(
    method: &str,
    url: &str,
    body: &str,
    cookie: Option<&str>,
    client_ip: Option<&str>,
    db_path: &str,
    auth_enabled: bool,
    auth_user: &str,
    auth_pass: &str,
    sessions: &Sessions,
) -> Resp {
    let path = url.split('?').next().unwrap_or("/");
    let authenticated = !auth_enabled || is_authed(cookie, sessions);

    // 鉴权相关接口
    match path {
        "/api/session" => {
            return json_resp(
                200,
                &json!({ "authRequired": auth_enabled, "authenticated": authenticated }),
            );
        }
        "/api/login" => {
            return api_login(method, body, auth_enabled, auth_user, auth_pass, sessions);
        }
        "/api/logout" => {
            return api_logout(cookie, sessions);
        }
        "/api/status" => {
            return api_status(db_path);
        }
        _ => {}
    }

    // 其余数据接口在未登录时拒绝
    if path.starts_with("/api/") && !authenticated {
        return json_resp(401, &json!({ "error": "未登录", "code": 401 }));
    }

    match path {
        "/" | "/index.html" => respond_body("text/html; charset=utf-8", 200, INDEX_HTML),
        "/app.js" => respond_body("application/javascript; charset=utf-8", 200, APP_JS),
        "/style.css" => respond_body("text/css; charset=utf-8", 200, STYLE_CSS),
        "/vendor/chart.umd.min.js" => {
            respond_body("application/javascript; charset=utf-8", 200, CHART_JS)
        }
        "/favicon.ico" => respond_body("text/plain; charset=utf-8", 204, ""),
        "/api/devices" => api_devices(db_path, client_ip),
        "/api/latest" => api_latest(db_path),
        "/api/series" => api_series(db_path, url),
        _ => respond_body("text/plain; charset=utf-8", 404, "未找到页面"),
    }
}

fn gen_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:x}", nanos, c)
}

fn cookie_token(cookie: Option<&str>) -> Option<&str> {
    let c = cookie?;
    for part in c.split(';') {
        let p = part.trim();
        if let Some(v) = p.strip_prefix("koakuma_session=") {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn is_authed(cookie: Option<&str>, sessions: &Sessions) -> bool {
    if let Some(tok) = cookie_token(cookie) {
        if let Ok(set) = sessions.lock() {
            return set.contains(tok);
        }
    }
    false
}

fn api_login(
    method: &str,
    body: &str,
    auth_enabled: bool,
    auth_user: &str,
    auth_pass: &str,
    sessions: &Sessions,
) -> Resp {
    if method != "POST" {
        return json_resp(405, &json!({ "ok": false, "error": "方法不允许" }));
    }
    if !auth_enabled {
        return json_resp(200, &json!({ "ok": true }));
    }
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let u = v.get("username").and_then(|x| x.as_str()).unwrap_or("");
    let p = v.get("password").and_then(|x| x.as_str()).unwrap_or("");
    if u == auth_user && p == auth_pass {
        let token = gen_token();
        if let Ok(mut set) = sessions.lock() {
            set.insert(token.clone());
        }
        let cookie = format!(
            "{}={}; HttpOnly; Path=/; SameSite=Lax; Max-Age=604800",
            SESSION_COOKIE, token
        );
        let h = Header::from_bytes(&b"Set-Cookie"[..], cookie.as_bytes()).unwrap();
        return json_resp_headers(200, &json!({ "ok": true }), vec![h]);
    }
    json_resp(401, &json!({ "ok": false, "error": "用户名或密码错误" }))
}

fn api_logout(cookie: Option<&str>, sessions: &Sessions) -> Resp {
    if let Some(tok) = cookie_token(cookie) {
        if let Ok(mut set) = sessions.lock() {
            set.remove(tok);
        }
    }
    let expired = format!(
        "{}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0",
        SESSION_COOKIE
    );
    let h = Header::from_bytes(&b"Set-Cookie"[..], expired.as_bytes()).unwrap();
    json_resp_headers(200, &json!({ "ok": true }), vec![h])
}

fn file_size(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 返回工程版本（编译期 CARGO_PKG_VERSION）与数据库占用字节数（含 -wal/-shm）。
fn api_status(db_path: &str) -> Resp {
    let mut total = file_size(db_path);
    total += file_size(&format!("{}-wal", db_path));
    total += file_size(&format!("{}-shm", db_path));
    json_resp(200, &json!({ "version": env!("CARGO_PKG_VERSION"), "dbBytes": total }))
}

fn api_devices(db_path: &str, client_ip: Option<&str>) -> Resp {
    let conn = match db::open(db_path) {
        Ok(c) => c,
        Err(e) => return json_resp(500, &json!({"error": format!("打开数据库失败：{}", e)})),
    };
    let res = (|| -> rusqlite::Result<Vec<Value>> {
        let mut stmt = conn.prepare(
            "SELECT id, mac, COALESCE(ip,''), COALESCE(name,''), last_seen
             FROM devices
             ORDER BY (name IS NULL OR name = ''), name COLLATE NOCASE, id",
        )?;
        let it = stmt.query_map([], |r| {
            let ip: String = r.get(2)?;
            let is_self = match client_ip {
                Some(c) => !c.is_empty() && c == ip,
                None => false,
            };
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "mac": r.get::<_, String>(1)?,
                "ip": ip,
                "name": r.get::<_, String>(3)?,
                "last_seen": r.get::<_, i64>(4)?,
                "self": is_self,
            }))
        })?;
        let mut v = Vec::new();
        for x in it {
            v.push(x?);
        }
        Ok(v)
    })();
    match res {
        Ok(v) => json_resp(200, &Value::Array(v)),
        Err(e) => json_resp(500, &json!({"error": format!("查询失败：{}", e)})),
    }
}

fn api_latest(db_path: &str) -> Resp {
    let conn = match db::open(db_path) {
        Ok(c) => c,
        Err(e) => return json_resp(500, &json!({"error": format!("打开数据库失败：{}", e)})),
    };
    let res = (|| -> rusqlite::Result<Value> {
        let tmax: Option<i64> = conn.query_row("SELECT MAX(ts) FROM a_raw", [], |r| r.get::<_, Option<i64>>(0))?;
        let tmax = match tmax {
            Some(t) => t,
            None => return Ok(json!({"ts": 0, "devices": []})),
        };
        let mut stmt = conn.prepare(
            "SELECT a.dev_id, d.mac, COALESCE(d.ip,''), COALESCE(d.name,''), a.rx_rate, a.tx_rate
             FROM a_raw a JOIN devices d ON d.id = a.dev_id
             WHERE a.ts = ?1
             ORDER BY (d.name IS NULL OR d.name = ''), d.name COLLATE NOCASE, a.dev_id",
        )?;
        let it = stmt.query_map(params![tmax], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "mac": r.get::<_, String>(1)?,
                "ip": r.get::<_, String>(2)?,
                "name": r.get::<_, String>(3)?,
                "rx_rate": r.get::<_, i64>(4)?,
                "tx_rate": r.get::<_, i64>(5)?,
            }))
        })?;
        let mut v = Vec::new();
        for x in it {
            v.push(x?);
        }
        Ok(json!({"ts": tmax, "devices": v}))
    })();
    match res {
        Ok(v) => json_resp(200, &v),
        Err(e) => json_resp(500, &json!({"error": format!("查询失败：{}", e)})),
    }
}

fn api_series(db_path: &str, url: &str) -> Resp {
    let p = query_params(url);
    let dev: i64 = match p.get("dev").and_then(|s| s.parse::<i64>().ok()) {
        Some(v) => v,
        None => return json_resp(400, &json!({"error": "缺少或非法的 dev 参数"})),
    };
    let layer = p.get("layer").cloned().unwrap_or_else(|| "b".to_string());
    let to = p.get("to").and_then(|s| s.parse::<i64>().ok()).unwrap_or_else(now_secs);
    let from = p.get("from").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);

    let sql = match layer.as_str() {
        "a" => "SELECT ts, rx_rate, tx_rate FROM a_raw WHERE dev_id = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts",
        "b" => "SELECT ts, rx_avg, tx_avg FROM b_min WHERE dev_id = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts",
        "c" => "SELECT ts, rx_avg, tx_avg FROM c_10min WHERE dev_id = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts",
        _ => return json_resp(400, &json!({"error": "layer 只能为 a / b / c"})),
    };

    let conn = match db::open(db_path) {
        Ok(c) => c,
        Err(e) => return json_resp(500, &json!({"error": format!("打开数据库失败：{}", e)})),
    };
    let res = (|| -> rusqlite::Result<Vec<Value>> {
        let mut stmt = conn.prepare(sql)?;
        let it = stmt.query_map(params![dev, from, to], |r| {
            let ts: i64 = r.get(0)?;
            let rx: i64 = r.get(1)?;
            let tx: i64 = r.get(2)?;
            Ok(json!([ts, rx, tx]))
        })?;
        let mut v = Vec::new();
        for x in it {
            v.push(x?);
        }
        Ok(v)
    })();
    match res {
        Ok(points) => json_resp(200, &json!({"dev": dev, "layer": layer, "points": points})),
        Err(e) => json_resp(500, &json!({"error": format!("查询失败：{}", e)})),
    }
}