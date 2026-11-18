# koakuma

> 名字取自"小恶魔"——一个安静的流量采集小助手，默默记录、不打扰宿主。

## 用途

`koakuma` 是一个面向 OpenWrt/嵌入式路由器（基于高通 IPQ 系列平台、musl 库、aarch64 架构）设计的**分设备网络流量采集守护进程**。

它复用路由器系统自带的流量统计后台（通过 `ubus` 的 `trafficd` 服务读取每个设备的实时收发速率），按固定周期采样并写入本地 SQLite 数据库，并提供三层不同精度的历史数据，供网页端查询与绘制趋势曲线。

核心特性：

- 按设备（MAC/IP）分账统计上传、下载速率；
- 采样数据分三层存储：原始细粒度、分钟级均值、十分钟级均值，兼顾"近期细查"与"长期趋势"；
- 各层数据独立设置保留时长，过期自动清理，存储占用可控；
- 单文件静态二进制，无运行时依赖，资源占用极低，适合内存与存储受限的嵌入式设备；
- 附带轻量 Web 页面，可查看实时速率与历史曲线。

## 文件层级

```
koakuma/
├── AGENTS.md                 协作与提交规范
├── README.md                 本文件：用途、文件层级、编译方法
├── LICENSE
├── Cargo.toml                Rust 工程清单与依赖
├── .cargo/
│   └── config.toml           交叉编译链接器配置
├── .gitignore
├── src/
│   ├── main.rs               程序入口：初始化数据库、日志、信号处理、主循环
│   ├── config.rs             运行配置：采集周期、数据库路径、各层保留时长
│   ├── collector.rs          调用 ubus 的 trafficd 服务采集各设备当前速率
│   ├── db.rs                 SQLite 连接、表结构定义、PRAGMA 设置
│   ├── aggregator.rs         A→B→C 三层滚动聚合逻辑
│   ├── retention.rs          过期数据清理
│   └── web.rs                内置轻量 HTTP 服务：数据查询接口与静态页面
├── scripts/
│   ├── build.sh              交叉编译脚本（输出 aarch64-musl 静态二进制）
│   └── deploy.sh             部署脚本（目标地址等通过环境变量传入）
├── web/                      前端静态资源
│   ├── index.html
│   ├── app.js
│   └── style.css
└── docs/
    └── design.md             存储设计与表结构说明
```

## 编译方法

### 依赖

- Rust 工具链（stable）；
- `cargo-zigbuild` 与 `zig`：用于从任意宿主平台交叉编译到 `aarch64-unknown-linux-musl`；
- 目标三元组：`aarch64-unknown-linux-musl`（musl 静态链接，产物无动态库依赖）。

安装交叉工具链：

```bash
rustup target add aarch64-unknown-linux-musl
cargo install cargo-zigbuild
# 另需在系统中安装 zig（https://ziglang.org/）
```

### 构建

```bash
# 交叉编译出面向路由器的静态二进制
cargo zigbuild --release --target aarch64-unknown-linux-musl

# 产物位于：
# target/aarch64-unknown-linux-musl/release/koakuma
```

也可直接使用脚本：

```bash
bash scripts/build.sh
```

> 说明：默认启用 `rusqlite` 的 `bundled` 特性，SQLite 会被静态编入二进制，因此最终产物为单个自包含文件，便于直接拷贝到路由器运行。

## 部署（示例）

部署与运行涉及的具体主机地址、路径、账号等均通过环境变量传入，**不在仓库中硬编码任何具体网络信息**：

```bash
KOAKUMA_HOST="<目标主机>" \
KOAKUMA_USER="<用户名>" \
bash scripts/deploy.sh
```

## 许可证

见 [LICENSE](./LICENSE)。
