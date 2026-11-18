# 存储与表结构设计

本文说明 koakuma 的数据模型、三层采样结构与存储容量估算。

## 数据来源

采样数据来自路由器系统自带的 `trafficd` 服务（通过 ubus 调用 `trafficd hw '{"tree":true}'` 获取）。该服务已经按设备维护了实时收发速率（字节/秒）与累计字节数，本进程只负责**按周期读取并持久化**，不重复进行内核层抓包，因此额外开销极小。

## 三层采样结构

| 表 | 粒度 | 保留时长 | 说明 |
|----|------|----------|------|
| A 表 `a_raw` | 原始采样（默认 10 秒） | 3 天 | 近期细查，滚动删除 |
| B 表 `b_min` | 1 分钟均值 | 35 天 | 覆盖近一个月曲线所需 |
| C 表 `c_10min` | 10 分钟均值 | 180 天 | 长期趋势 |

聚合方式为**持续滚动聚合**：每次采样时把当前值累加进"当前分钟"与"当前十分钟"的累加器，跨过边界即落库，避免"每日批处理任务失败导致丢数据"。

## 表结构

设备字典表只保存一次 MAC / IP / 主机名，采样表全部使用整数外键 `dev_id` 引用，避免每行重复存储字符串：

```sql
CREATE TABLE devices (
  id         INTEGER PRIMARY KEY,
  mac        TEXT UNIQUE,
  ip         TEXT,
  name       TEXT,
  first_seen INTEGER,
  last_seen  INTEGER
);

CREATE TABLE a_raw (
  dev_id  INTEGER NOT NULL,
  ts      INTEGER NOT NULL,   -- Unix 秒
  rx_rate INTEGER NOT NULL,   -- 字节/秒
  tx_rate INTEGER NOT NULL,
  PRIMARY KEY (dev_id, ts)
) WITHOUT ROWID;

CREATE TABLE b_min (
  dev_id INTEGER NOT NULL,
  ts     INTEGER NOT NULL,    -- 分钟起点 Unix 秒
  rx_avg INTEGER NOT NULL,
  tx_avg INTEGER NOT NULL,
  rx_max INTEGER,
  tx_max INTEGER,
  PRIMARY KEY (dev_id, ts)
) WITHOUT ROWID;

CREATE TABLE c_10min (
  dev_id INTEGER NOT NULL,
  ts     INTEGER NOT NULL,    -- 十分钟起点 Unix 秒
  rx_avg INTEGER NOT NULL,
  tx_avg INTEGER NOT NULL,
  rx_max INTEGER,
  tx_max INTEGER,
  PRIMARY KEY (dev_id, ts)
) WITHOUT ROWID;
```

## 精简存储的要点

1. **WITHOUT ROWID + 复合主键**：省去隐式 rowid，主键本身即索引，查询"某设备某时间段"无需额外索引。
2. **采样表只用整数 dev_id**：MAC（17 字节）、主机名等仅存于 devices 表。
3. **速率存整数字节/秒**：不存浮点或字符串。
4. **不存累计字节**：累计值可由速率积分或直接向 trafficd 查询，节省每行约 16 字节。
5. **时间戳为整型 Unix 秒**，SQLite 变长编码，较小数值占用少。
6. **PRAGMA**：`page_size=4096`、`journal_mode=WAL`、`auto_vacuum=INCREMENTAL`、`synchronous=NORMAL`。
7. **A 表不建二级索引**，过期清理使用 `DELETE ... WHERE ts < ?` 配合增量 vacuum。
8. **仅采集活跃设备**：只记录当前有流量或在线的设备，忽略历史残留与随机 MAC 造成的空条目。

## 容量估算

以约 15 台活跃设备、A 表每行约 20 字节估算：

| 表 | 每天 | 保留总量 |
|----|------|----------|
| A（10 秒 / 3 天） | ~2.5 MB | ~7.4 MB |
| B（1 分钟 / 35 天） | ~0.4 MB | ~14 MB |
| C（10 分钟 / 180 天） | ~0.04 MB | ~7 MB |
| **合计** | | **~29 MB** |

在 100 MB 级可用空间上占用约三成，余量充足；即使活跃设备翻倍至 30 台，总量也只有约 58 MB。
