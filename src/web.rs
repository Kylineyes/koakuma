//! 内置轻量 HTTP 服务：数据查询接口与静态页面。
//!
//! 设计：
//! - 静态资源在编译期内嵌进二进制（include_str!），部署时无需额外文件；
//! - Web 线程使用独立的只读连接查询数据库，WAL 模式下与主线程的写入互不阻塞。

use std::io::Cursor;
use std::thread;

use rusqlite::params;
use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use crate::db;

type Resp = Response<Cursor<Vec<u8>>>;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE_CSS: &str = include_str!("../web/style.css");
const CHART_JS: &str = include_str!("../web/vendor/chart.umd.min.js");

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn respond_body(content_type: &'static str, status: u16, body: &str) -> Resp {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
}

fn json_resp(status: u16, value: &Value) -> Resp {
    respond_body("application/json; charset=utf-8", status, &value.to_string())
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
pub fn serve(db_path: String, listen_addr: String) {
    thread::spawn(move || {
        let server = match Server::http(listen_addr.as_str()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[web] 监听 {} 失败：{}", listen_addr, e);
                return;
            }
        };
        eprintln!("[web] HTTP 服务已启动：http://{}", listen_addr);
        for request in server.incoming_requests() {
            // 用请求来源 IP 判断“本机设备”（浏览器所在的那台机器）
            let client_ip = request.remote_addr().map(|a| normalize_ip(&a.ip().to_string()));
            let resp = handle(&request.url().to_string(), &db_path, client_ip.as_deref());
            let _ = request.respond(resp);
        }
    });
}

/// 归一化 IP 字符串（去除 IPv4-mapped IPv6 前缀）。
fn normalize_ip(s: &str) -> String {
    s.strip_prefix("::ffff:").unwrap_or(s).to_string()
}

fn handle(url: &str, db_path: &str, client_ip: Option<&str>) -> Resp {
    let path = url.split('?').next().unwrap_or("/");
    match path {
        "/" | "/index.html" => respond_body("text/html; charset=utf-8", 200, INDEX_HTML),
        "/app.js" => respond_body("application/javascript; charset=utf-8", 200, APP_JS),
        "/style.css" => respond_body("text/css; charset=utf-8", 200, STYLE_CSS),
        "/vendor/chart.umd.min.js" => respond_body("application/javascript; charset=utf-8", 200, CHART_JS),
        "/favicon.ico" => respond_body("text/plain; charset=utf-8", 204, ""),
        "/api/devices" => api_devices(db_path, client_ip),
        "/api/latest" => api_latest(db_path),
        "/api/series" => api_series(db_path, url),
        _ => respond_body("text/plain; charset=utf-8", 404, "未找到页面"),
    }
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
            // 与请求来源 IP 相同的设备即“本机”
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