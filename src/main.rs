//! koakuma 入口：初始化数据库、日志、信号处理与主循环。
mod aggregator;
mod collector;
mod config;
mod db;
mod retention;
mod web;

fn main() {
    // 骨架：具体逻辑在后续分支实现
    let cfg = config::Config::default();
    println!("koakuma 启动，采集周期 = {} 秒", cfg.sample_interval_secs);
}
