//! 运行配置：采样周期、数据库路径、各层保留时长、监听地址、Web 登录鉴权等。
//!
//! 支持从 config.yaml 读取配置（默认 ./config.yaml，可用 --config 或 KOAKUMA_CONFIG 指定）。
//! 优先级：默认值 < config.yaml < 环境变量 < 命令行参数。

use std::collections::HashMap;
use std::env;
use std::fs;

#[derive(Clone, Debug)]
pub struct Config {
    /// 采样周期（秒）
    pub sample_interval_secs: u64,
    /// 数据库文件路径
    pub db_path: String,
    /// A 表（原始采样）保留天数
    pub a_retain_days: u32,
    /// B 表（1 分钟均值）保留天数
    pub b_retain_days: u32,
    /// C 表（10 分钟均值）保留天数
    pub c_retain_days: u32,
    /// 内置 HTTP 服务监听地址（IP:端口）
    pub listen_addr: String,
    /// 只采样一次然后退出（调试/验证用）
    pub once: bool,
    /// 从本地文件读取 trafficd JSON（本地测试用）
    pub ubus_json_file: Option<String>,
    /// 设备别名配置文件路径（MAC=名称）
    pub aliases_path: String,
    /// 是否启用 Web 页面登录鉴权
    pub auth_enabled: bool,
    /// 登录用户名
    pub auth_username: String,
    /// 登录密码
    pub auth_password: String,
    /// 是否统计路由器出口（WAN）上下行总流量
    pub wan_enabled: bool,
    /// WAN 出口接口名（为空时自动通过 ubus 解析，无法解析则回退 pppoe-wan）
    pub wan_iface: String,
    /// 实际加载的配置文件路径（不存在则为 None，仅用于日志）
    pub config_file: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_interval_secs: 10,
            db_path: "/data/other_vol/koakuma/traffic.db".to_string(),
            a_retain_days: 3,
            b_retain_days: 35,
            c_retain_days: 180,
            listen_addr: "0.0.0.0:8686".to_string(),
            once: false,
            ubus_json_file: None,
            aliases_path: "/data/other_vol/koakuma/aliases.conf".to_string(),
            auth_enabled: false,
            auth_username: "admin".to_string(),
            auth_password: String::new(),
            wan_enabled: true,
            wan_iface: String::new(),
            config_file: None,
        }
    }
}

impl Config {
    /// 从配置文件、环境变量与命令行参数加载配置。命令行优先于环境变量，二者缺省时使用配置文件/默认值。
    pub fn load() -> Self {
        let args: Vec<String> = env::args().collect();

        // 1) 定位配置文件：--config > KOAKUMA_CONFIG > 默认 config.yaml
        let mut cfg_path = "config.yaml".to_string();
        if let Ok(v) = env::var("KOAKUMA_CONFIG") {
            if !v.trim().is_empty() {
                cfg_path = v;
            }
        }
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--config" && i + 1 < args.len() {
                cfg_path = args[i + 1].clone();
            }
            i += 1;
        }

        let mut cfg = Config::default();
        match fs::read_to_string(&cfg_path) {
            Ok(text) => match cfg.apply_yaml(&text) {
                Ok(()) => cfg.config_file = Some(cfg_path.clone()),
                Err(e) => eprintln!("解析配置文件 {} 失败：{}（将使用默认值继续）", cfg_path, e),
            },
            Err(_) => { /* 文件不存在：使用默认值，不算错误 */ }
        }

        // 2) 环境变量
        if let Ok(v) = env::var("KOAKUMA_INTERVAL") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.sample_interval_secs = n;
            }
        }
        if let Ok(v) = env::var("KOAKUMA_DB") {
            cfg.db_path = v;
        }
        if let Ok(v) = env::var("KOAKUMA_LISTEN") {
            cfg.listen_addr = v;
        }
        if let Ok(v) = env::var("KOAKUMA_A_RETAIN_DAYS") {
            if let Ok(n) = v.parse::<u32>() {
                cfg.a_retain_days = n;
            }
        }
        if let Ok(v) = env::var("KOAKUMA_B_RETAIN_DAYS") {
            if let Ok(n) = v.parse::<u32>() {
                cfg.b_retain_days = n;
            }
        }
        if let Ok(v) = env::var("KOAKUMA_C_RETAIN_DAYS") {
            if let Ok(n) = v.parse::<u32>() {
                cfg.c_retain_days = n;
            }
        }
        if let Ok(v) = env::var("KOAKUMA_UBUS_JSON_FILE") {
            cfg.ubus_json_file = Some(v);
        }
        if let Ok(v) = env::var("KOAKUMA_ALIASES") {
            cfg.aliases_path = v;
        }
        if let Ok(v) = env::var("KOAKUMA_AUTH_ENABLED") {
            if let Some(b) = parse_bool(&v) {
                cfg.auth_enabled = b;
            }
        }
        if let Ok(v) = env::var("KOAKUMA_AUTH_USER") {
            if !v.is_empty() {
                cfg.auth_username = v;
            }
        }
        if let Ok(v) = env::var("KOAKUMA_AUTH_PASSWORD") {
            cfg.auth_password = v;
        }
        if let Ok(v) = env::var("KOAKUMA_WAN_ENABLED") {
            if let Some(b) = parse_bool(&v) {
                cfg.wan_enabled = b;
            }
        }
        if let Ok(v) = env::var("KOAKUMA_WAN_IFACE") {
            cfg.wan_iface = v;
        }

        // 3) 命令行参数（覆盖环境变量）
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--once" => cfg.once = true,
                "--db" => {
                    i += 1;
                    if i < args.len() {
                        cfg.db_path = args[i].clone();
                    }
                }
                "--interval" => {
                    i += 1;
                    if i < args.len() {
                        if let Ok(n) = args[i].parse::<u64>() {
                            cfg.sample_interval_secs = n;
                        }
                    }
                }
                "--listen" => {
                    i += 1;
                    if i < args.len() {
                        cfg.listen_addr = args[i].clone();
                    }
                }
                "--ubus-json" => {
                    i += 1;
                    if i < args.len() {
                        cfg.ubus_json_file = Some(args[i].clone());
                    }
                }
                "--aliases" => {
                    i += 1;
                    if i < args.len() {
                        cfg.aliases_path = args[i].clone();
                    }
                }
                "--config" => {
                    // 值已在第一步读取
                    i += 1;
                }
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                other => {
                    eprintln!("未知参数：{}（使用 --help 查看用法）", other);
                }
            }
            i += 1;
        }
        cfg
    }

    /// 解析并应用 config.yaml 文本（最小 YAML 子集：两级 section -> key: value）。
    pub fn apply_yaml(&mut self, text: &str) -> Result<(), String> {
        let map = parse_simple_yaml(text)?;
        let get = |sec: &str, key: &str| -> Option<String> {
            map.get(sec)
                .and_then(|m| m.get(key))
                .cloned()
                .or_else(|| map.get("").and_then(|m| m.get(key)).cloned())
        };

        if let Some(v) = get("server", "listen") {
            if !v.trim().is_empty() {
                self.listen_addr = v.trim().to_string();
            }
        }
        if let Some(v) = get("db", "path") {
            if !v.trim().is_empty() {
                self.db_path = v.trim().to_string();
            }
        } else if let Some(v) = map.get("").and_then(|m| m.get("db_path")).cloned() {
            if !v.trim().is_empty() {
                self.db_path = v.trim().to_string();
            }
        }
        if let Some(v) = get("sampling", "interval") {
            if let Ok(n) = v.trim().parse::<u64>() {
                self.sample_interval_secs = n;
            }
        }
        if let Some(v) = get("retention", "a_days") {
            if let Ok(n) = v.trim().parse::<u32>() {
                self.a_retain_days = n;
            }
        }
        if let Some(v) = get("retention", "b_days") {
            if let Ok(n) = v.trim().parse::<u32>() {
                self.b_retain_days = n;
            }
        }
        if let Some(v) = get("retention", "c_days") {
            if let Ok(n) = v.trim().parse::<u32>() {
                self.c_retain_days = n;
            }
        }
        if let Some(v) = get("aliases", "path") {
            if !v.trim().is_empty() {
                self.aliases_path = v.trim().to_string();
            }
        }
        if let Some(v) = get("auth", "enabled") {
            if let Some(b) = parse_bool(&v) {
                self.auth_enabled = b;
            }
        }
        if let Some(v) = get("auth", "username") {
            if !v.is_empty() {
                self.auth_username = v;
            }
        }
        if let Some(v) = get("auth", "password") {
            self.auth_password = v;
        }
        if let Some(v) = get("wan", "enabled") {
            if let Some(b) = parse_bool(&v) {
                self.wan_enabled = b;
            }
        }
        if let Some(v) = get("wan", "interface") {
            if !v.trim().is_empty() {
                self.wan_iface = v.trim().to_string();
            }
        }
        Ok(())
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// 解析最小 YAML 子集，返回 section -> (key -> value)；顶层标量归入 "" 段。
fn parse_simple_yaml(text: &str) -> Result<HashMap<String, HashMap<String, String>>, String> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    out.insert(String::new(), HashMap::new());
    let mut cur = String::new();
    for (ln, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            if let Some(sec) = trimmed.strip_suffix(':') {
                cur = sec.trim().to_string();
                out.entry(cur.clone()).or_default();
                continue;
            }
            match split_kv(trimmed) {
                Some((k, v)) => {
                    out.entry(String::new()).or_default().insert(k, v);
                    cur = String::new();
                }
                None => return Err(format!("第 {} 行无法解析：{}", ln + 1, raw)),
            }
        } else {
            match split_kv(trimmed) {
                Some((k, v)) => {
                    out.entry(cur.clone()).or_default().insert(k, v);
                }
                None => return Err(format!("第 {} 行无法解析：{}", ln + 1, raw)),
            }
        }
    }
    Ok(out)
}

fn split_kv(s: &str) -> Option<(String, String)> {
    let idx = s.find(':')?;
    let k = s[..idx].trim().to_string();
    if k.is_empty() {
        return None;
    }
    let raw = s[idx + 1..].trim();
    Some((k, unquote_value(raw)))
}

fn unquote_value(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return raw[1..raw.len() - 1].to_string();
    }
    // 去掉未加引号值里的行内注释（" #" 起）
    if let Some(pos) = raw.find(" #") {
        return raw[..pos].trim().to_string();
    }
    raw.to_string()
}

fn print_help() {
    println!("{}", HELP_TEXT);
}

const HELP_TEXT: &str = "koakuma —— 分设备网络流量采集守护进程

用法：
  koakuma [选项]

选项：
  --config <路径>        配置文件（YAML，默认 config.yaml）
  --once                 只采样一次并落库后退出（调试用）
  --db <路径>            数据库文件路径（覆盖配置）
  --interval <秒>        采样周期（覆盖配置）
  --listen <地址:端口>   内置 HTTP 监听地址（覆盖配置）
  --ubus-json <路径>     从本地文件读取 trafficd JSON 而非调用 ubus（本地测试用）
  --aliases <路径>       设备别名配置（MAC=名称）
  -h, --help             显示本帮助

环境变量：
  KOAKUMA_CONFIG / KOAKUMA_INTERVAL / KOAKUMA_DB / KOAKUMA_LISTEN
  KOAKUMA_A_RETAIN_DAYS / KOAKUMA_B_RETAIN_DAYS / KOAKUMA_C_RETAIN_DAYS
  KOAKUMA_UBUS_JSON_FILE / KOAKUMA_ALIASES
  KOAKUMA_AUTH_ENABLED / KOAKUMA_AUTH_USER / KOAKUMA_AUTH_PASSWORD";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_sections_applied() {
        let text = "server:\n  listen: 1.2.3.4:9999\ndb:\n  path: /tmp/x.db\nauth:\n  enabled: true\n  username: bob\n  password: \"p@ss #1\"\nretention:\n  a_days: 5\n  b_days: 7\n";
        let mut c = Config::default();
        c.apply_yaml(text).unwrap();
        assert_eq!(c.listen_addr, "1.2.3.4:9999");
        assert_eq!(c.db_path, "/tmp/x.db");
        assert!(c.auth_enabled);
        assert_eq!(c.auth_username, "bob");
        assert_eq!(c.auth_password, "p@ss #1");
        assert_eq!(c.a_retain_days, 5);
        assert_eq!(c.b_retain_days, 7);
    }

    #[test]
    fn yaml_inline_comment_and_bool_variants() {
        let text = "auth:\n  enabled: yes # 启用\n  username: alice\nlisten: 0.0.0.0:80\n";
        let mut c = Config::default();
        c.apply_yaml(text).unwrap();
        assert!(c.auth_enabled);
        assert_eq!(c.auth_username, "alice");
        assert_eq!(c.listen_addr, "0.0.0.0:80"); // 顶层标量兜底
    }

    #[test]
    fn yaml_quoted_password_keeps_hash() {
        let text = "auth:\n  password: 'a#b'\n";
        let mut c = Config::default();
        c.apply_yaml(text).unwrap();
        assert_eq!(c.auth_password, "a#b");
    }
}
