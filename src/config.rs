//! 运行配置：采集周期、数据库路径、各层保留时长。

pub struct Config {
    /// 采样周期（秒）
    pub sample_interval_secs: u64,
    /// 数据库文件路径
    pub db_path: String,
    /// A 表保留天数
    pub a_retain_days: u32,
    /// B 表保留天数
    pub b_retain_days: u32,
    /// C 表保留天数
    pub c_retain_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_interval_secs: 10,
            db_path: "/data/other_vol/koakuma/traffic.db".to_string(),
            a_retain_days: 3,
            b_retain_days: 35,
            c_retain_days: 180,
        }
    }
}
