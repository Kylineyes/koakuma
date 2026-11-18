# koakuma 项目交接文档

> 面向接手的下一位代理/开发者。本文汇总截至当前的**已完成工作、已验证的技术事实、关键决策与后续待办**，使其无需重新探测即可继续开发。
>
> **注意：本文档遵循仓库约定，不包含任何具体网络信息（IP、密码、账号等）。所有涉及目标设备的信息以占位符表示，实际运行信息由用户在部署时通过环境变量提供。**

---

## 一、项目一句话概述

`koakuma`（小恶魔）是一个面向 OpenWrt/嵌入式路由器（高通 IPQ 平台、musl、aarch64）的**分设备网络流量采集守护进程**：复用系统自带的 `ubus trafficd` 服务读取每设备实时收发速率，按周期采样写入本地 SQLite，分三层精度（原始10秒 / 1分钟 / 10分钟）存储并提供网页查询与曲线。

仓库：`git@github.com:Kylineyes/koakuma.git`

---

## 二、已完成工作（当前状态）

### 2.1 仓库与分支
- `main`：仅初始提交，**代理不得直接改/合并/推送**（见 AGENTS.md 规则 7，由用户执行 dev→main）。
- `dev`：已从 main 建立并推送，包含项目初始化骨架。
- `feat/init-scaffold`：初始化工作分支，已合并进 dev（`--no-ff`）。
- 当前最新提交：
  ```
  e045e3c merge: 合并项目初始化骨架到 dev
  f80b829 chore(项目初始化): 搭建 koakuma 工程骨架与文档
  ```

### 2.2 已建立的骨架（均已验证可编译）
```
koakuma/
├── AGENTS.md            协作规范（用户提供，勿改）
├── README.md            用途/文件层级/编译方法
├── Cargo.toml           rusqlite(bundled)/serde/serde_json/tiny_http/libc；release: opt-level=z+LTO+strip+panic=abort
├── .cargo/config.toml   交叉编译目标说明
├── .gitignore           /target, *.db, *.db-wal, *.db-shm 等
├── docs/design.md       存储设计与表结构（已定稿）
├── src/
│   ├── main.rs          入口骨架（mod 声明 + 打印采样周期）
│   ├── config.rs        Config 结构体（默认值已定，见下）
│   ├── db.rs            空骨架（待实现）
│   ├── collector.rs     空骨架（待实现）
│   ├── aggregator.rs    空骨架（待实现）
│   ├── retention.rs     空骨架（待实现）
│   └── web.rs           空骨架（待实现）
├── scripts/
│   ├── build.sh         交叉编译脚本
│   └── deploy.sh        部署脚本（全部走环境变量，无硬编码信息）
└── web/
    ├── index.html       前端骨架
    ├── app.js
    └── style.css
```

### 2.3 已验证的构建结果
- `cargo check` 通过（仅有骨架的 dead_code 警告，属正常）。
- `cargo zigbuild --release --target aarch64-unknown-linux-musl` **成功**，产物 `target/aarch64-unknown-linux-musl/release/koakuma` 约 **300KB** 静态二进制。
- 测试二进制已在真实路由器上运行成功（含 `rusqlite` bundled SQLite，输出 `musl-sqlite-ok`）。

---

## 三、关键设计决策（已与用户确认，勿擅自更改）

### 3.1 数据来源：复用原生 `trafficd`（不重复抓包）
目标设备已有一个常驻守护 `trafficd`（实测 CPU 0.0% / 内存 0.2%），通过 ubus 提供每设备实时速率。**koakuma 只读它，不做 conntrack 抓包**，因此额外开销极小。

- 采集命令：`ubus call trafficd hw '{"tree":true}'` 获取全部设备；
- 单设备查询：`ubus call trafficd ip '{"ip":"<某IP>","debug":false}'`（**注意 debug 字段为布尔，缺省会报 `Failed to parse message data`，必须显式传 `false`**）；
- WAN 汇总：`ubus call trafficd wan '{}'`。

`hw` 返回 JSON 结构（每个 MAC 一项，`ip_list` 内含 IP 与速率）：
```json
{
  "<MAC>": {
    "hw": "<MAC>", "parent": "", "hostname": "<主机名或空>",
    "ifname": "", "assoc": 0,
    "ip_list": [
      {
        "ip": "<设备IP>", "ageing_timer": <秒>, "hw": "<MAC>",
        "rx_bytes": <累计>, "tx_bytes": <累计>,
        "rx_rate": <字节/秒>, "tx_rate": <字节/秒>,
        "max_rx_rate": <>, "max_tx_rate": <>
      }
    ]
  }
}
```
字段含义：`rx_rate/tx_rate` 为**实时速率（字节/秒）**，约每 2 秒刷新；`rx_bytes/tx_bytes` 为连接累计。

### 3.2 三层存储（用户指定）
| 表 | 粒度 | 保留 | 说明 |
|----|------|------|------|
| A `a_raw` | 每 10 秒采样 | 3 天 | 近期细查，滚动删除 |
| B `b_min` | 1 分钟均值 | 35 天 | 近一月曲线数据源 |
| C `c_10min` | 10 分钟均值 | 180 天 | 长期趋势 |

- 聚合方式：**持续滚动聚合**（每次采样累加进"当前分钟/当前十分钟"累加器，跨界即落库），**不是**每天批处理——更抗故障（用户已同意）。
- A 表**不存累计字节**（rx_bytes/tx_bytes），只存 rate；累计可由 rate 积分或向 trafficd 查（用户已同意）。

### 3.3 表结构（详见 docs/design.md）
- `devices(id INTEGER PK, mac UNIQUE, ip, name, first_seen, last_seen)` —— 字符串只存一次；
- `a_raw(dev_id, ts, rx_rate, tx_rate, PK(dev_id,ts)) WITHOUT ROWID`；
- `b_min(dev_id, ts, rx_avg, tx_avg, rx_max, tx_max, PK(dev_id,ts)) WITHOUT ROWID`；
- `c_10min` 同 b_min（粒度 10 分钟）。
- PRAGMA：`page_size=4096, journal_mode=WAL, auto_vacuum=INCREMENTAL, synchronous=NORMAL`。
- **只采集活跃设备**（有流量或 assoc=1），忽略历史残留与随机 MAC 空条目。

### 3.4 容量（已核算，放得下）
15 台活跃、A 表约 20 字节/行估算：A≈7.4MB + B≈14MB + C≈7MB ≈ **29MB**；30 台悲观约 58MB。可用约 114MB，占用 26%~51%，余量充足。

---

## 四、目标设备技术事实（已实测，供实现时参考）

| 项目 | 值 |
|------|-----|
| 系统 | OpenWrt 18.06-SNAPSHOT，内核 5.4.213 |
| 平台 | `ipq53xx/generic`（高通 IPQ5332） |
| 固件架构串 | `aarch64_cortex-a53_neon-vfpv4`（Xiaomi 私有） |
| libc | **musl 1.1.24**（`/lib/ld-musl-aarch64.so.1`）→ 交叉编译目标用 `aarch64-unknown-linux-musl` |
| CPU | 4 核，支持 fp/asimd/aes/crc32 |
| 内存 | 总 883MB，空闲约 78MB（紧张） |
| 根分区 `/` | 31.8MB，**100% 满（0KB）**，只读 squashfs —— **任何新文件都不能放根分区** |
| 可用存储 | **`/data/other_vol` 约 114MB 空闲**（ubi，可写）→ 数据库与二进制放这里 |
| `/tmp` | tmpfs 425MB 空闲（重启丢失，可用作运行时文件） |
| 已存在库 | `/usr/lib/libsqlite3.so.0`（可选动态链接，但推荐 bundled 静态） |
| 工具 | jsonfilter / awk / curl / wget / uclient-fetch / crontab 均在；**无 python、无 sqlite3 CLI** |
| init 体系 | procd（`/etc/init.d/`） |
| opkg | distfeeds 已被修正为公共 18.06 `aarch64_cortex-a53` feed；但**本项目不依赖 opkg 安装任何东西**（全静态二进制） |

**关键约束提醒：**
- 根分区 0KB，安装/写文件必须用 `/data/other_vol` 或 `/tmp`。
- `/tmp` 重启清空，数据库不能放这里（除非接受重启丢历史）。
- 设备长期运行（曾达 181 天 uptime）、负载偏高（load ≈1.0），实现要尽量省 CPU/内存。

---

## 五、交叉编译方法（已验证可用）

环境（当前开发机，Windows）：
- Rust stable（实测 1.97.1），已 `rustup target add aarch64-unknown-linux-musl`；
- 已安装 `cargo-zigbuild` 与 `zig`（scoop 0.17.1）；
- 命令：`cargo zigbuild --release --target aarch64-unknown-linux-musl`；
- 产物：`target/aarch64-unknown-linux-musl/release/koakuma`（静态，无动态依赖）。

> 本机**无** sftp-server 于目标端，`pscp`/sftp 上传会失败。上传二进制可用 **base64 经 ssh 管道**（`base64 -d` 解码到目标），或目标有 `wget/curl` 时走 HTTP。所有目标地址/账号用环境变量，不入库。

---

## 六、目标设备部署计划（待实现后执行）

1. 交叉编译静态二进制；
2. 在设备上建目录 `/data/other_vol/koakuma/`（存二进制 + `traffic.db`）；
3. 上传二进制、赋可执行权限；
4. 首次运行初始化数据库（创建表 + PRAGMA）；
5. 配置 procd 开机自启（`/etc/init.d/koakuma`，参照现有 init 脚本风格），或先手动 nohup 调试；
6. 验证：`ubus` 采样正常、A/B/C 三表按周期写入、过界聚合正确、过期清理生效；
7. Web 部分：内置 tiny_http 服务监听某端口，提供查询接口 + `web/` 静态页；**涉及 WebUI 必须实际启动并交互验证**（AGENTS.md 规则 10）。

---

## 七、待办（下一个分支的核心实现）

> 建议在 `dev` 上新开分支，例如 `feat/collector-core`，按 AGENTS.md 流程开发、验证、合并、推送。

- [ ] `config.rs`：完善配置（从命令行参数或配置文件读取，给出默认值，见下方"默认值"）。
- [ ] `db.rs`：建表、PRAGMA、连接管理、`devices` 表的 upsert（按 MAC 去重，更新 ip/name/last_seen）。
- [ ] `collector.rs`：调用 `ubus call trafficd hw '{"tree":true}'`，解析 JSON（serde），筛出活跃设备，产出 `(mac, ip, name, rx_rate, tx_rate)` 列表。
- [ ] `aggregator.rs`：写入 A 表；维护"当前分钟/当前十分钟"累加器，跨界落 B/C（含 avg 与 max）。
- [ ] `retention.rs`：按各表保留天数删除过期行 + 增量 vacuum。
- [ ] `main.rs`：主循环（每 `sample_interval_secs` 秒采集一次）、信号处理、日志、守护化。
- [ ] `web.rs`：内置 HTTP 服务——接口如 `/api/devices`、`/api/series?dev=&layer=&from=&to=`；静态页来自 `web/`。
- [ ] `web/`：Chart.js 曲线（近一月每分钟来自 B 表；近 3 天 10 秒细查来自 A 表）。
- [ ] 交叉编译 + 部署 + 在真实设备上实测（CPU/内存/落盘正确性）。
- [ ] 更新 README 的"运行/配置"章节。

### Config 默认值（已在 config.rs 定）
```
sample_interval_secs = 10
db_path = "/data/other_vol/koakuma/traffic.db"
a_retain_days = 3
b_retain_days = 35
c_retain_days = 180
```
（端口、监听地址等运行期参数**不要硬编码具体 IP**，建议监听 `0.0.0.0` 某端口或用可配置项。）

---

## 八、注意事项与坑（已踩过）

1. `ubus call trafficd ip` 必须带 `"debug":false`，否则解析失败。
2. `trafficd hw` 会返回大量**老化残留与随机 MAC 空条目**（曾见 72 条目但真实在线仅 6~8 台）——务必过滤活跃设备，否则表膨胀。
3. 根分区 0KB，**不要往 `/` 写任何东西**；数据库在 `/data/other_vol`。
4. 目标端无 sftp-server，`pscp` 不可用；用 base64-over-ssh 或 HTTP 上传。
5. musl 目标必须用 `--target aarch64-unknown-linux-musl`，否则动态链接的 glibc 二进制在目标上跑不起来。
6. 设备内存仅约 78MB 空闲，SQLite 缓存别开太大（默认 2MB page cache 足够），进程目标常驻 < 5MB。
7. 涉及 WebUI 的改动要**实际启动应用并交互验证**，不能只看编译通过（AGENTS.md 规则 10）。
8. 提交遵循 Conventional Commits + 简体中文；代码注释/日志/文案一律简体中文（AGENTS.md 规则 1、3）。

---

## 九、参考

- 存储/表结构细节：见仓库 `docs/design.md`。
- 协作规范：见仓库 `AGENTS.md`。
- 编译方法：见仓库 `README.md`。

---

_交接时间：本文件生成于项目初始化阶段完成时。接手者请先通读 AGENTS.md 与 docs/design.md，再在 dev 上开分支实现第七节待办。_
