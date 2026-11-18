//! A→B→C 三层滚动聚合逻辑。
//!
//! 每来一条 A 表采样，就把它累加进“当前分钟”和“当前十分钟”的累加器；
//! 当采样时间跨过分钟（或十分钟）边界时，先把上一桶的均值/峰值落库到
//! B 表（或 C 表），再清空并开始新桶。这样无需回扫 A 表，开销恒定。

use std::collections::HashMap;

use rusqlite::Connection;

use crate::db;

#[derive(Debug, Clone)]
struct BucketStat {
    sum_rx: i64,
    sum_tx: i64,
    count: i64,
    max_rx: i64,
    max_tx: i64,
}

impl BucketStat {
    fn new() -> Self {
        Self { sum_rx: 0, sum_tx: 0, count: 0, max_rx: 0, max_tx: 0 }
    }
    fn add(&mut self, rx: i64, tx: i64) {
        self.sum_rx += rx;
        self.sum_tx += tx;
        self.count += 1;
        if rx > self.max_rx {
            self.max_rx = rx;
        }
        if tx > self.max_tx {
            self.max_tx = tx;
        }
    }
}

/// 滚动聚合器。
pub struct Aggregator {
    cur_min: i64,
    min_stats: HashMap<i64, BucketStat>,
    cur_ten: i64,
    ten_stats: HashMap<i64, BucketStat>,
}

impl Aggregator {
    pub fn new(ts: i64) -> Self {
        Self {
            cur_min: ts / 60,
            min_stats: HashMap::new(),
            cur_ten: ts / 600,
            ten_stats: HashMap::new(),
        }
    }

    /// 处理一条 A 表采样：累积到分钟/十分钟桶，跨界则先落库再重置。
    pub fn ingest(&mut self, conn: &Connection, dev_id: i64, ts: i64, rx: i64, tx: i64) -> rusqlite::Result<()> {
        let m = ts / 60;
        if m != self.cur_min {
            self.flush_min(conn)?;
            self.cur_min = m;
            self.min_stats.clear();
        }
        self.min_stats.entry(dev_id).or_insert_with(BucketStat::new).add(rx, tx);

        let t = ts / 600;
        if t != self.cur_ten {
            self.flush_ten(conn)?;
            self.cur_ten = t;
            self.ten_stats.clear();
        }
        self.ten_stats.entry(dev_id).or_insert_with(BucketStat::new).add(rx, tx);
        Ok(())
    }

    fn flush_min(&self, conn: &Connection) -> rusqlite::Result<()> {
        let ts = self.cur_min * 60;
        for (dev_id, st) in &self.min_stats {
            if st.count == 0 {
                continue;
            }
            db::insert_b(conn, *dev_id, ts, st.sum_rx / st.count, st.sum_tx / st.count, st.max_rx, st.max_tx)?;
        }
        Ok(())
    }

    fn flush_ten(&self, conn: &Connection) -> rusqlite::Result<()> {
        let ts = self.cur_ten * 600;
        for (dev_id, st) in &self.ten_stats {
            if st.count == 0 {
                continue;
            }
            db::insert_c(conn, *dev_id, ts, st.sum_rx / st.count, st.sum_tx / st.count, st.max_rx, st.max_tx)?;
        }
        Ok(())
    }

    /// 优雅退出时把当前未满的桶也落库，避免丢失。
    pub fn flush_all(&mut self, conn: &Connection) -> rusqlite::Result<()> {
        self.flush_min(conn)?;
        self.flush_ten(conn)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn minute_bucket_aggregates_average_and_max() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        let mut agg = Aggregator::new(0);
        agg.ingest(&conn, 1, 10, 100, 10).unwrap();
        agg.ingest(&conn, 1, 20, 200, 20).unwrap();
        agg.ingest(&conn, 1, 70, 300, 30).unwrap();
        let row: (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT rx_avg, tx_avg, rx_max, tx_max FROM b_min WHERE dev_id = 1 AND ts = 0",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (150, 15, 200, 20));
        let c_cnt: i64 = conn.query_row("SELECT COUNT(*) FROM c_10min", [], |r| r.get(0)).unwrap();
        assert_eq!(c_cnt, 0);
    }

    #[test]
    fn ten_minute_bucket_rolls_over() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        let mut agg = Aggregator::new(0);
        agg.ingest(&conn, 1, 10, 100, 10).unwrap();
        agg.ingest(&conn, 1, 70, 200, 20).unwrap();
        agg.ingest(&conn, 1, 610, 300, 30).unwrap();
        let row: (i64, i64) = conn
            .query_row("SELECT rx_avg, tx_avg FROM c_10min WHERE dev_id = 1 AND ts = 0", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(row, (150, 15));
    }
}