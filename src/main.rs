//! koakuma 入口：初始化数据库、启动 Web 服务、采集主循环与信号处理。

mod aggregator;
mod aliases;
mod collector;
mod config;
mod db;
mod retention;
mod web;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 停止标志：信号处理器置位，主循环据此优雅退出。
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn log(msg: &str) {
    eprintln!("[{}] {}", now_secs(), msg);
}

fn main() {
    let cfg = config::Config::load();

    // 确保数据库所在目录存在
    if let Some(parent) = std::path::Path::new(&cfg.db_path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let conn = match db::open(&cfg.db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("打开数据库失败：{}", e);
            std::process::exit(1);
        }
    };
    if let Err(e) = db::init_schema(&conn) {
        eprintln!("初始化表结构失败：{}", e);
        std::process::exit(1);
    }

    // 设备别名（友好名称）配置，支持热加载
    let mut aliases = aliases::Aliases::new(&cfg.aliases_path);

    log(&format!(
        "koakuma 启动：db={} 采样周期={}s 监听={} 别名={} 配置={}",
        cfg.db_path,
        cfg.sample_interval_secs,
        cfg.listen_addr,
        cfg.aliases_path,
        cfg.config_file.as_deref().unwrap_or("(无)")
    ));
    if cfg.auth_enabled {
        if cfg.auth_password.is_empty() {
            log("警告：已启用登录鉴权但密码为空，请尽快在 config.yaml 的 auth.password 设置强密码");
        } else {
            log(&format!("已启用登录鉴权，用户名={}", cfg.auth_username));
        }
    }

    // --once：采样一次、落库、清理后退出（便于部署后快速自检）
    if cfg.once {
        let mut agg = aggregator::Aggregator::new(now_secs());
        match run_sample(&conn, &cfg, &mut agg, &mut aliases) {
            Ok(n) => log(&format!("单次采样完成，写入 {} 台活跃设备", n)),
            Err(e) => {
                eprintln!("单次采样失败：{}", e);
                std::process::exit(1);
            }
        }
        // --once 也要把当前桶落库，使自检能覆盖 B/C 聚合链路
        let _ = agg.flush_all(&conn);
        let ts = now_secs();
        match retention::run(&conn, ts, &cfg) {
            Ok((a, b, c)) => log(&format!("过期清理：A={} B={} C={} 行", a, b, c)),
            Err(e) => log(&format!("过期清理失败：{}", e)),
        }
        return;
    }

    // 启动内置 Web 服务（独立线程、独立只读连接）
    web::serve(
        cfg.db_path.clone(),
        cfg.listen_addr.clone(),
        cfg.auth_enabled,
        cfg.auth_username.clone(),
        cfg.auth_password.clone(),
    );

    // 安装信号处理（SIGINT / SIGTERM）
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }

    let mut agg = aggregator::Aggregator::new(now_secs());
    let mut last_retention = 0i64;
    let mut sample_count: u64 = 0;
    let mut wan = WanTracker::new();

    while !STOP.load(Ordering::SeqCst) {
        let ts = now_secs();
        match run_sample(&conn, &cfg, &mut agg, &mut aliases) {
            Ok(_n) => {}
            Err(e) => log(&format!("采样失败：{}", e)),
        }
        // 路由器出口（WAN）流量：与设备采样同一周期落库
        match wan.sample(&conn, &cfg, &mut agg, ts) {
            Ok(_) => {}
            Err(e) => log(&format!("出口流量采样失败：{}", e)),
        }
        sample_count += 1;
        // 每 30 次采样（约 5 分钟）快照一次当前未满桶，降低异常退出（SIGKILL/断电）时的数据丢失
        if sample_count % 30 == 0 {
            let _ = agg.flush_all(&conn);
        }

        // 每小时清理一次过期数据
        if ts - last_retention >= 3600 {
            match retention::run(&conn, ts, &cfg) {
                Ok((a, b, c)) => log(&format!("过期清理：A={} B={} C={} 行", a, b, c)),
                Err(e) => log(&format!("过期清理失败：{}", e)),
            }
            last_retention = ts;
        }

        // 采样周期下限 1 秒，避免 --interval 0 造成忙循环
        sleep_interruptible(cfg.sample_interval_secs.max(1));
    }

    log("收到停止信号，落库未满桶并退出…");
    let _ = agg.flush_all(&conn);
    log("koakuma 已退出");
}

/// 路由器出口（WAN）在 devices 表中使用的保留 MAC 与显示名。
const WAN_MAC: &str = "__wan__";
const WAN_NAME: &str = "路由器出口（WAN）";
const WAN_IP: &str = "wan";

/// WAN 出口流量追踪：保存上一次的接口累计计数器与时间戳，用于差分求速率。
struct WanTracker {
    iface: String,
    prev: Option<(i64, u64, u64)>,
}

impl WanTracker {
    fn new() -> Self {
        Self { iface: String::new(), prev: None }
    }

    /// 采样一次出口流量：读取接口累计字节，按时间差求上下行速率并写入 A 表与聚合器。
    /// 首个采样点无前值，仅记录基准（rate=0），从第二次起得到真实速率。
    fn sample(
        &mut self,
        conn: &rusqlite::Connection,
        cfg: &config::Config,
        agg: &mut aggregator::Aggregator,
        ts: i64,
    ) -> Result<(), String> {
        if !cfg.wan_enabled {
            return Ok(());
        }
        // 解析出口接口名：显式配置优先，否则动态查询，最后回退 pppoe-wan。
        if self.iface.is_empty() {
            self.iface = if !cfg.wan_iface.trim().is_empty() {
                cfg.wan_iface.trim().to_string()
            } else {
                collector::resolve_wan_iface().unwrap_or_else(|| "pppoe-wan".to_string())
            };
        }
        let counters = match collector::read_iface_counters(&self.iface) {
            Ok(c) => c,
            Err(_) => {
                // 接口偶发不存在（pppoe 重拨等）：清空接口名以便下次重新解析
                self.iface.clear();
                return Ok(());
            }
        };

        let (mut rx_rate, mut tx_rate) = (0i64, 0i64);
        if let Some((prev_ts, prev_rx, prev_tx)) = self.prev {
            let dt = ts - prev_ts;
            if dt > 0 {
                // 计数器回绕/重置（比前值小）时按 0 处理，避免负数。
                let d_rx = counters.rx_bytes.saturating_sub(prev_rx) as i64;
                let d_tx = counters.tx_bytes.saturating_sub(prev_tx) as i64;
                rx_rate = d_rx / dt;
                tx_rate = d_tx / dt;
            }
        }
        self.prev = Some((ts, counters.rx_bytes, counters.tx_bytes));

        let dev_id = db::upsert_device(conn, WAN_MAC, WAN_IP, WAN_NAME, ts)
            .map_err(|e| format!("upsert 出口设备失败：{}", e))?;
        db::insert_a(conn, dev_id, ts, rx_rate, tx_rate)
            .map_err(|e| format!("写出口 A 表失败：{}", e))?;
        agg.ingest(conn, dev_id, ts, rx_rate, tx_rate)
            .map_err(|e| format!("出口聚合落库失败：{}", e))?;
        Ok(())
    }
}

/// 采样一次：读 trafficd → 更新设备字典 → 写 A 表 → 滚动聚合。返回写入的活跃设备数。
fn run_sample(conn: &rusqlite::Connection, cfg: &config::Config, agg: &mut aggregator::Aggregator, aliases: &mut aliases::Aliases) -> Result<usize, String> {
    aliases.refresh();
    let ts = now_secs();
    let mut samples = collector::collect(&cfg.ubus_json_file)?;
    // 应用别名覆盖：优先使用友好名称
    for s in samples.iter_mut() {
        if let Some(name) = aliases.resolve(&s.mac) {
            s.name = name.to_string();
        }
    }
    let mut n = 0usize;
    for s in &samples {
        let dev_id = db::upsert_device(conn, &s.mac, &s.ip, &s.name, ts)
            .map_err(|e| format!("upsert 设备失败：{}", e))?;
        db::insert_a(conn, dev_id, ts, s.rx_rate, s.tx_rate)
            .map_err(|e| format!("写 A 表失败：{}", e))?;
        agg.ingest(conn, dev_id, ts, s.rx_rate, s.tx_rate)
            .map_err(|e| format!("聚合落库失败：{}", e))?;
        n += 1;
    }
    Ok(n)
}

/// 可被停止信号打断的休眠。
fn sleep_interruptible(secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline && !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }
}