//! 设备别名覆盖：读取别名配置文件，把 MAC 映射为友好名称。
//!
//! 配置文件格式（每行一条，大小写不敏感，# 开头为注释）：
//!   AA:BB:CC:DD:EE:FF = 客厅电视
//!   11:22:33:44:55:66   书房电脑
//! 支持 "MAC = 名称"、"MAC<TAB>名称" 或 "MAC 名称"。
//! 文件不存在时忽略（等价于无别名）；文件修改后按 mtime 自动热加载。

use std::collections::HashMap;
use std::time::SystemTime;

pub struct Aliases {
    path: String,
    map: HashMap<String, String>,
    mtime: Option<SystemTime>,
}

impl Aliases {
    pub fn new(path: &str) -> Self {
        let mut a = Aliases {
            path: path.to_string(),
            map: HashMap::new(),
            mtime: None,
        };
        // 初始加载：记录当前 mtime 并解析
        a.mtime = Self::file_mtime(&a.path);
        a.reload();
        a
    }

    fn file_mtime(path: &str) -> Option<SystemTime> {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }

    /// 若文件修改时间变化则重新加载（文件删除则清空）。开销极小，可每轮调用。
    pub fn refresh(&mut self) {
        let cur = Self::file_mtime(&self.path);
        if cur == self.mtime {
            return;
        }
        self.mtime = cur;
        self.reload();
    }

    fn reload(&mut self) {
        let mut map = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&self.path) {
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let (mac, name) = if let Some(idx) = line.find('=') {
                    (line[..idx].trim(), line[idx + 1..].trim())
                } else if let Some(idx) = line.find(char::is_whitespace) {
                    (line[..idx].trim(), line[idx..].trim())
                } else {
                    continue;
                };
                if mac.is_empty() || name.is_empty() {
                    continue;
                }
                map.insert(mac.to_lowercase(), name.to_string());
            }
        }
        self.map = map;
    }

    /// 解析给定 MAC 的别名（大小写不敏感）。
    pub fn resolve(&self, mac: &str) -> Option<&str> {
        self.map.get(&mac.to_lowercase()).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_resolves() {
        let mut p = std::env::temp_dir();
        p.push(format!("koakuma_alias_{}.conf", std::process::id()));
        std::fs::write(
            &p,
            "# 注释\nAA:BB:CC:DD:EE:FF = 客厅电视\n11:22:33:44:55:66\t书房电脑\nbadline\n",
        )
        .unwrap();
        let a = Aliases::new(p.to_str().unwrap());
        assert_eq!(a.resolve("aa:bb:cc:dd:ee:ff"), Some("客厅电视"));
        assert_eq!(a.resolve("AA:BB:CC:DD:EE:FF"), Some("客厅电视"));
        assert_eq!(a.resolve("11:22:33:44:55:66"), Some("书房电脑"));
        assert_eq!(a.resolve("00:00:00:00:00:00"), None);
        let _ = std::fs::remove_file(&p);
    }
}