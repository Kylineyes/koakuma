//! 调用 ubus 的 trafficd 服务采集各设备当前速率。

use std::process::Command;
use serde_json::Value;

/// 单台设备的采样结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSample {
    pub mac: String,
    pub ip: String,
    pub name: String,
    /// 下载速率（字节/秒）
    pub rx_rate: i64,
    /// 上传速率（字节/秒）
    pub tx_rate: i64,
}

/// 读取 trafficd 的原始 JSON 文本。优先使用配置的本地 JSON 文件（测试用），否则调用 ubus。
pub fn read_raw_json(ubus_json_file: &Option<String>) -> Result<String, String> {
    if let Some(path) = ubus_json_file {
        let s = std::fs::read_to_string(path).map_err(|e| format!("读取 JSON 文件失败：{}", e))?;
        // 去除可能存在的 UTF-8 BOM，避免 JSON 解析失败
        return Ok(s.strip_prefix('\u{feff}').map(|x| x.to_string()).unwrap_or(s));
    }
    let out = Command::new("ubus")
        .args(["call", "trafficd", "hw", r#"{"tree":true}"#])
        .output()
        .map_err(|e| format!("执行 ubus 失败：{}", e))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("ubus 返回非零状态：{}", err.trim()));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("ubus 输出不是有效 UTF-8：{}", e))
}

/// 从 serde_json 数值里尽读取 i64（兼容整数与浮点）。
fn num_i64(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        // 兼容某些固件把 assoc 返回成布尔值
        Some(Value::Bool(b)) => {
            if *b {
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// 解析 trafficd hw 的 JSON，筛出活跃设备并汇总其速率。
///
/// 活跃判定：assoc == 1（当前无线关联），或该设备任一 IP 的收发速率 > 0。
/// 同一 MAC 若存在多个 IP，则把各自速率相加作为该设备总速率；IP 取第一个非空值。
pub fn parse(json: &str) -> Result<Vec<DeviceSample>, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("JSON 解析失败：{}", e))?;
    let obj = root
        .as_object()
        .ok_or_else(|| "trafficd 返回不是 JSON 对象".to_string())?;

    let mut result = Vec::new();
    for (mac, dev) in obj.iter() {
        let assoc = num_i64(dev.get("assoc"));
        let hostname = dev.get("hostname").and_then(|v| v.as_str()).unwrap_or("");
        let mut sum_rx: i64 = 0;
        let mut sum_tx: i64 = 0;
        let mut first_ip: Option<String> = None;
        let mut has_rate = false;

        if let Some(ips) = dev.get("ip_list").and_then(|v| v.as_array()) {
            for ipent in ips {
                let ip = ipent.get("ip").and_then(|v| v.as_str()).unwrap_or("");
                if ip.is_empty() {
                    continue;
                }
                let rx = num_i64(ipent.get("rx_rate"));
                let tx = num_i64(ipent.get("tx_rate"));
                sum_rx += rx;
                sum_tx += tx;
                if first_ip.is_none() {
                    first_ip = Some(ip.to_string());
                }
                if rx > 0 || tx > 0 {
                    has_rate = true;
                }
            }
        }

        let active = assoc == 1 || has_rate;
        if !active {
            continue;
        }
        // 无有效 IP 的条目无法按 IP 统计，跳过
        let ip = match first_ip {
            Some(v) => v,
            None => continue,
        };
        let name = if hostname.is_empty() || hostname == "*" {
            String::new()
        } else {
            hostname.to_string()
        };
        result.push(DeviceSample {
            mac: mac.clone(),
            ip,
            name,
            rx_rate: sum_rx,
            tx_rate: sum_tx,
        });
    }
    Ok(result)
}

/// WAN（路由器出口）接口的累计收发字节计数器快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfaceCounters {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// 从 /proc/net/dev 文本中解析指定接口的累计收发字节；找不到接口返回 None。
pub fn parse_proc_net_dev(text: &str, iface: &str) -> Option<IfaceCounters> {
    for line in text.lines() {
        let (name, rest) = match line.split_once(':') {
            Some(v) => v,
            None => continue,
        };
        if name.trim() != iface {
            continue;
        }
        let cols: Vec<&str> = rest.split_whitespace().collect();
        // 每行依次为 rx: bytes packets errs drop fifo frame compressed multicast，
        // 然后 tx: bytes packets ...；bytes 分别是第 0 与第 8 个字段。
        if cols.len() < 9 {
            continue;
        }
        let rx = cols[0].parse::<u64>().ok()?;
        let tx = cols[8].parse::<u64>().ok()?;
        return Some(IfaceCounters { rx_bytes: rx, tx_bytes: tx });
    }
    None
}

/// 读取指定网络接口的累计收发字节（读取 /proc/net/dev）。
pub fn read_iface_counters(iface: &str) -> Result<IfaceCounters, String> {
    let text = std::fs::read_to_string("/proc/net/dev")
        .map_err(|e| format!("读取 /proc/net/dev 失败：{}", e))?;
    parse_proc_net_dev(&text, iface)
        .ok_or_else(|| format!("未找到网络接口 {}（可能未连接）", iface))
}

/// 解析 ubus network.interface.wan status 的 JSON，取 l3_device 作为 WAN 出口接口名。
pub fn parse_wan_iface(json: &str) -> Option<String> {
    let root: Value = serde_json::from_str(json).ok()?;
    root.get("l3_device")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 通过 ubus 动态解析 WAN 逻辑接口对应的设备名（如 pppoe-wan / eth0.2）。
/// 失败时回退到 None，由调用方使用配置的默认接口名。
pub fn resolve_wan_iface() -> Option<String> {
    let out = Command::new("ubus")
        .args(["call", "network.interface.wan", "status"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    parse_wan_iface(&text)
}

/// 采集一次：读取并解析，返回活跃设备列表。
pub fn collect(ubus_json_file: &Option<String>) -> Result<Vec<DeviceSample>, String> {
    let raw = read_raw_json(ubus_json_file)?;
    parse(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "AA:BB:CC:DD:EE:01": {"hw":"AA:BB:CC:DD:EE:01","hostname":"phone","assoc":1,
        "ip_list":[{"ip":"10.0.0.5","rx_rate":1000,"tx_rate":200}]},
      "AA:BB:CC:DD:EE:02": {"hw":"AA:BB:CC:DD:EE:02","hostname":"stale","assoc":0,
        "ip_list":[{"ip":"10.0.0.9","rx_rate":0,"tx_rate":0}]},
      "AA:BB:CC:DD:EE:03": {"hw":"AA:BB:CC:DD:EE:03","hostname":"","assoc":0,
        "ip_list":[{"ip":"10.0.0.7","rx_rate":50,"tx_rate":0}]},
      "AA:BB:CC:DD:EE:04": {"hw":"AA:BB:CC:DD:EE:04","hostname":"dual","assoc":1,
        "ip_list":[{"ip":"10.0.0.8","rx_rate":10,"tx_rate":1},{"ip":"10.0.0.18","rx_rate":20,"tx_rate":2}]}
    }"#;

    #[test]
    fn filters_and_sums() {
        let v = parse(SAMPLE).unwrap();
        // 活跃：assoc1 的 .5、有速率的 .7、assoc1 的 .4；stale .9 被过滤
        assert_eq!(v.len(), 3);
        assert!(v.iter().any(|s| s.ip == "10.0.0.5" && s.rx_rate == 1000 && s.tx_rate == 200));
        assert!(v.iter().any(|s| s.ip == "10.0.0.7" && s.rx_rate == 50));
        assert!(!v.iter().any(|s| s.ip == "10.0.0.9"));
        // 多 IP 相加
        assert!(v.iter().any(|s| s.ip == "10.0.0.8" && s.rx_rate == 30 && s.tx_rate == 3));
        // 空主机名规整为空串
        assert!(v.iter().any(|s| s.ip == "10.0.0.7" && s.name.is_empty()));
    }

    const PROC: &str = r#"Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 100 2 0 0 0 0 0 0 200 2 0 0 0 0 0 0
pppoe-wan: 478217281841 454984843 0 275 0 0 0 0 422206499108 424876108 0 0 0 0 0 0
br-lan: 34399127763612 36146857310 0 16 0 0 0 26479462 20752136011685 28163088904 0 0 0 0 0 0
"#;

    #[test]
    fn parses_iface_counters() {
        let c = parse_proc_net_dev(PROC, "pppoe-wan").unwrap();
        assert_eq!(c.rx_bytes, 478217281841);
        assert_eq!(c.tx_bytes, 422206499108);
        assert!(parse_proc_net_dev(PROC, "nope").is_none());
    }

    #[test]
    fn parses_wan_iface() {
        let j = r#"{"up":true,"l3_device":"pppoe-wan","device":"eth0.1","proto":"pppoe"}"#;
        assert_eq!(parse_wan_iface(j).as_deref(), Some("pppoe-wan"));
        assert!(parse_wan_iface("{}").is_none());
        assert!(parse_wan_iface(r#"{"l3_device":""}"#).is_none());
    }
}