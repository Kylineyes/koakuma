//! 过期数据清理。

use rusqlite::{params, Connection};

use crate::config::Config;

/// 按各层保留天数删除过期数据，并做增量 vacuum。
/// 返回 (A 表删除行数, B 表删除行数, C 表删除行数)。
///
/// 采用“按 dev_id + ts”利用复合主键删除，避免对整表做全扫描。
pub fn run(conn: &Connection, now: i64, cfg: &Config) -> rusqlite::Result<(usize, usize, usize)> {
    let a_cut = now - cfg.a_retain_days as i64 * 86400;
    let b_cut = now - cfg.b_retain_days as i64 * 86400;
    let c_cut = now - cfg.c_retain_days as i64 * 86400;

    let dev_ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM devices")?;
        let it = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        let mut v = Vec::new();
        for x in it {
            v.push(x?);
        }
        v
    };

    // 用一个事务包裹所有删除，避免逐条自动提交带来的开销
    let tx = conn.unchecked_transaction()?;
    let mut da = 0;
    let mut db_del = 0;
    let mut dc = 0;
    for id in &dev_ids {
        da += tx.execute("DELETE FROM a_raw WHERE dev_id = ?1 AND ts < ?2", params![id, a_cut])?;
        db_del += tx.execute("DELETE FROM b_min WHERE dev_id = ?1 AND ts < ?2", params![id, b_cut])?;
        dc += tx.execute("DELETE FROM c_10min WHERE dev_id = ?1 AND ts < ?2", params![id, c_cut])?;
    }
    tx.commit()?;

    // PRAGMA incremental_vacuum 每次只回收一页并逐行返回，必须遍历完结果集才会彻底回收空闲页。
    // 仅当数据库 auto_vacuum=INCREMENTAL 时有效（本程序新建的库均满足）。
    {
        let mut stmt = conn.prepare("PRAGMA incremental_vacuum")?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
    }
    Ok((da, db_del, dc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db;
    use rusqlite::Connection;

    #[test]
    fn deletes_only_expired_rows() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        let now = 10_000_000i64;
        let cfg = Config::default();
        let did = db::upsert_device(&conn, "aa:bb", "1.1.1.1", "n", now).unwrap();
        let old = now - 4 * 86400;
        db::insert_a(&conn, did, old, 1, 1).unwrap();
        db::insert_a(&conn, did, now, 2, 2).unwrap();
        db::insert_b(&conn, did, old, 1, 1, 1, 1).unwrap();
        db::insert_b(&conn, did, now, 2, 2, 2, 2).unwrap();
        // 旧数据为 4 天前：A 保留 3 天 -> 应删除；B 保留 35 天、C 保留 180 天 -> 均应保留
        let (da, dbn, dc) = run(&conn, now, &cfg).unwrap();
        assert_eq!((da, dbn, dc), (1, 0, 0));
        let a_cnt: i64 = conn.query_row("SELECT COUNT(*) FROM a_raw", [], |r| r.get(0)).unwrap();
        assert_eq!(a_cnt, 1, "A 只应删除过期行，保留新行");
        let b_cnt: i64 = conn.query_row("SELECT COUNT(*) FROM b_min", [], |r| r.get(0)).unwrap();
        assert_eq!(b_cnt, 2, "B 保留 35 天，两行都应保留");
    }
}