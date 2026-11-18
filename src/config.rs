//! 运行配置：采集周期、数据库路径、各层保留时长、监听地址等。

use std::env;

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
    /// 内置 HTTP 服务监听地址
    pub listen_addr: String,
    /// 只采样一次然后退出（调试/验证用）
    pub once: bool,
    /// 从本地文件读取 trafficd JSON（本地测试用），设置后优先于调用 ubus
    pub ubus_json_file: Option<String>,
    /// 设备别名配置文件路径（MAC=名称），文件不存在则忽略
    pub aliases_path: String,
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
        }
    }
}

impl Config {
    /// 从环境变量与命令行参数加载配置。命令行优先于环境变量，二者缺省时使用默认值。
    pub fn load() -> Self {
        let mut cfg = Config::default();

        // 环境变量
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

        // 命令行参数（覆盖环境变量）
        let args: Vec<String> = env::args().collect();
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
}

fn print_help() {
    println!("{}", HELP_TEXT);
}

const HELP_TEXT: &str = "koakuma —— 分设备网络流量采集守护进程

用法：
  koakuma [选项]

选项：
  --once                 只采样一次并落库后退出（调试用）
  --db <路径>            数据库文件路径（默认 /data/other_vol/koakuma/traffic.db）
  --interval <秒>        采样周期（默认 10 秒）
  --listen <地址:端口>   内置 HTTP 监听地址（默认 0.0.0.0:8686）
  --ubus-json <路径>     从本地文件读取 trafficd JSON 而非调用 ubus（本地测试用）
  --aliases <路径>       设备别名配置（MAC=名称，默认 /data/other_vol/koakuma/aliases.conf）
  -h, --help             显示本帮助

环境变量：
  KOAKUMA_INTERVAL / KOAKUMA_DB / KOAKUMA_LISTEN
  KOAKUMA_A_RETAIN_DAYS / KOAKUMA_B_RETAIN_DAYS / KOAKUMA_C_RETAIN_DAYS
  KOAKUMA_UBUS_JSON_FILE / KOAKUMA_ALIASES";
