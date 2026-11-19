//! SQLite 连接、表结构与 PRAGMA 设置。

use rusqlite::{params, Connection, Result};

/// 打开数据库并按需设置连接级 PRAGMA。
///
/// 说明：
/// - page_size 与 auto_vacuum 只对新建数据库生效，须在建表前设置；
/// - journal_mode=WAL 支持读写连接并发，适合“采集写入 + Web 只读查询”同时进行。
pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA page_size = 4096;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA auto_vacuum = INCREMENTAL;",
    )?;
    // journal_mode 会返回一行结果，用 query_row 读取；其余设置 PRAGMA 无返回行
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

/// 建表（幂等）。
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS devices (
            id          INTEGER PRIMARY KEY,
            mac         TEXT UNIQUE NOT NULL,
            ip          TEXT,
            name        TEXT,
            name_custom INTEGER NOT NULL DEFAULT 0,
            first_seen  INTEGER NOT NULL,
            last_seen   INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS a_raw (
            dev_id  INTEGER NOT NULL,
            ts      INTEGER NOT NULL,
            rx_rate INTEGER NOT NULL,
            tx_rate INTEGER NOT NULL,
            PRIMARY KEY (dev_id, ts)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS b_min (
            dev_id INTEGER NOT NULL,
            ts     INTEGER NOT NULL,
            rx_avg INTEGER NOT NULL,
            tx_avg INTEGER NOT NULL,
            rx_max INTEGER NOT NULL,
            tx_max INTEGER NOT NULL,
            PRIMARY KEY (dev_id, ts)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS c_10min (
            dev_id INTEGER NOT NULL,
            ts     INTEGER NOT NULL,
            rx_avg INTEGER NOT NULL,
            tx_avg INTEGER NOT NULL,
            rx_max INTEGER NOT NULL,
            tx_max INTEGER NOT NULL,
            PRIMARY KEY (dev_id, ts)
        ) WITHOUT ROWID;",
    )?;
    // 迁移旧库：补充 name_custom 列（已存在则忽略错误）
    let _ = conn.execute(
        "ALTER TABLE devices ADD COLUMN name_custom INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(())
}

/// 设备字典 upsert：按 MAC 去重，更新 ip/last_seen，空主机名不覆盖旧值，返回 dev_id。
pub fn upsert_device(conn: &Connection, mac: &str, ip: &str, name: &str, now: i64) -> Result<i64> {
    conn.execute(
        "INSERT INTO devices (mac, ip, name, first_seen, last_seen, name_custom)
         VALUES (?1, ?2, ?3, ?4, ?4, 0)
         ON CONFLICT(mac) DO UPDATE SET
            ip = excluded.ip,
            name = CASE WHEN devices.name_custom = 0 AND excluded.name <> '' THEN excluded.name ELSE devices.name END,
            last_seen = excluded.last_seen",
        params![mac, ip, name, now],
    )?;
    let id: i64 = conn.query_row("SELECT id FROM devices WHERE mac = ?1", params![mac], |r| r.get(0))?;
    Ok(id)
}

/// 修改设备显示名（写入数据库并标记为自定义名称，避免被后续采样覆盖）。
/// 传入空白名称表示取消自定义、恢复自动名称。
pub fn rename_device(conn: &Connection, id: i64, name: &str) -> Result<bool> {
    let trimmed = name.trim();
    let custom = if trimmed.is_empty() { 0 } else { 1 };
    let affected = conn.execute(
        "UPDATE devices SET name = ?1, name_custom = ?2 WHERE id = ?3",
        params![trimmed, custom, id],
    )?;
    Ok(affected > 0)
}

/// 写入一条 A 表原始采样。
pub fn insert_a(conn: &Connection, dev_id: i64, ts: i64, rx: i64, tx: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO a_raw (dev_id, ts, rx_rate, tx_rate) VALUES (?1, ?2, ?3, ?4)",
        params![dev_id, ts, rx, tx],
    )?;
    Ok(())
}

/// 写入一条 B 表（1 分钟）聚合。
pub fn insert_b(conn: &Connection, dev_id: i64, ts: i64, rx_avg: i64, tx_avg: i64, rx_max: i64, tx_max: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO b_min (dev_id, ts, rx_avg, tx_avg, rx_max, tx_max) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![dev_id, ts, rx_avg, tx_avg, rx_max, tx_max],
    )?;
    Ok(())
}

/// 写入一条 C 表（10 分钟）聚合。
pub fn insert_c(conn: &Connection, dev_id: i64, ts: i64, rx_avg: i64, tx_avg: i64, rx_max: i64, tx_max: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO c_10min (dev_id, ts, rx_avg, tx_avg, rx_max, tx_max) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![dev_id, ts, rx_avg, tx_avg, rx_max, tx_max],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_device_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let id1 = upsert_device(&conn, "aa:bb:cc:dd:ee:ff", "10.0.0.1", "手机", 100).unwrap();
        let id2 = upsert_device(&conn, "aa:bb:cc:dd:ee:ff", "10.0.0.2", "", 200).unwrap();
        assert_eq!(id1, id2, "同一 MAC 应复用同一 dev_id");
        let (ip, name, last): (String, String, i64) = conn
            .query_row("SELECT ip, name, last_seen FROM devices WHERE id = ?1", params![id1], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(ip, "10.0.0.2", "IP 应更新为新值");
        assert_eq!(name, "手机", "空主机名不应覆盖旧值");
        assert_eq!(last, 200);
    }

    #[test]
    fn open_creates_file_db_and_schema() {
        let mut path = std::env::temp_dir();
        path.push(format!("koakuma_test_{}.db", std::process::id()));
        let p = path.to_str().unwrap().to_string();
        let _ = std::fs::remove_file(&p);
        {
            let conn = open(&p).expect("open 应成功");
            init_schema(&conn).expect("建表应成功");
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0);
        }
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(format!("{}-wal", p));
        let _ = std::fs::remove_file(format!("{}-shm", p));
    }
}