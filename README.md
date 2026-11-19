# koakuma

> 名字取自日语「小悪魔」（koakuma），意为"小恶魔"——一个安静的流量采集小助手，默默记录、不打扰宿主。

## 用途

`koakuma` 是一个面向 OpenWrt/嵌入式路由器（基于高通 IPQ 系列平台、musl 库、aarch64 架构）设计的**分设备网络流量采集守护进程**。

它复用路由器系统自带的流量统计后台（通过 `ubus` 的 `trafficd` 服务读取每个设备的实时收发速率），按固定周期采样并写入本地 SQLite 数据库，并提供三层不同精度的历史数据，供网页端查询与绘制趋势曲线。

核心特性：

- 按设备（MAC/IP）分账统计上传、下载速率；
- 采样数据分三层存储：原始细粒度、分钟级均值、十分钟级均值，兼顾"近期细查"与"长期趋势"；
- 各层数据独立设置保留时长，过期自动清理，存储占用可控；
- 单文件静态二进制，无运行时依赖，资源占用极低，适合内存与存储受限的嵌入式设备；
- 附带轻量 Web 页面：设备多选、分开/叠加曲线、明暗主题，资源内嵌、离线可用。

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
├── config.yaml.example        配置示例（复制为 config.yaml；含明文口令的 config.yaml 不入库）
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
│   ├── deploy.sh             部署脚本（目标地址等通过环境变量传入）
│   └── koakuma.init          procd 开机自启服务脚本
├── web/                      前端静态资源
│   ├── index.html
│   ├── app.js
│   └── style.css
└── docs/
    ├── design.md             存储设计与表结构说明
    └── frontend-design.md    前端设计规范（布局/字号/配色/圆角/动效）
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

## 运行与配置

二进制为单文件静态程序，部署后可直接运行；生产环境建议用 procd 托管实现开机自启。

### 配置文件（config.yaml）

支持通过 YAML 配置监听地址、数据库路径与网页登录账号密码等。默认读取当前目录的 `config.yaml`（可用 `--config <路径>` 或环境变量 `KOAKUMA_CONFIG` 指定）。**优先级：默认值 < config.yaml < 环境变量 < 命令行参数**。含明文口令的 `config.yaml` 已加入 `.gitignore`，请勿提交；示例见 `config.yaml.example`。

```yaml
server:
  listen: "0.0.0.0:8686"        # 监听 IP:端口
db:
  path: "/data/other_vol/koakuma/traffic.db"
auth:
  enabled: true                 # 开启后访问页面需登录
  username: "admin"
  password: "改成你的强密码"
sampling:
  interval: 10
retention:
  a_days: 3
  b_days: 35
  c_days: 180
aliases:
  path: "/data/other_vol/koakuma/aliases.conf"
```

启用 `auth` 后，网页会先弹出登录框；登录以服务端会话（Cookie）校验，未登录无法获取 `/api/*` 数据。

### 命令行参数

| 参数 | 说明 | 默认 |
|------|------|------|
| `--config <路径>` | 配置文件（YAML） | `config.yaml` |
| `--db <路径>` | 数据库文件路径 | `/data/other_vol/koakuma/traffic.db` |
| `--interval <秒>` | 采样周期 | `10` |
| `--listen <地址:端口>` | 内置 HTTP 监听地址 | `0.0.0.0:8686` |
| `--once` | 只采样一次并落库后退出（部署自检用） | 关闭 |
| `--ubus-json <路径>` | 从本地文件读取 trafficd JSON（本地调试用） | 无 |
| `--aliases <路径>` | 设备别名配置（MAC=名称） | `/data/other_vol/koakuma/aliases.conf` |

以上均可用环境变量覆盖：`KOAKUMA_DB`、`KOAKUMA_INTERVAL`、`KOAKUMA_LISTEN`、`KOAKUMA_A_RETAIN_DAYS`、`KOAKUMA_B_RETAIN_DAYS`、`KOAKUMA_C_RETAIN_DAYS`、`KOAKUMA_UBUS_JSON_FILE`、`KOAKUMA_ALIASES`、`KOAKUMA_CONFIG`、`KOAKUMA_AUTH_ENABLED`、`KOAKUMA_AUTH_USER`、`KOAKUMA_AUTH_PASSWORD`。

### 设备别名（显示友好名称）

设备名默认取 `trafficd` 的名称；取不到时会显示成 MAC 派生名（如 `74563cc98269`）。可配置别名文件，按 MAC 覆盖为友好名称（大小写不敏感，`#` 开头为注释，支持 `=`、Tab 或空格分隔）：

```conf
# MAC = 友好名称
AA:BB:CC:DD:EE:FF = 客厅电视
11:22:33:44:55:66  书房电脑
```

默认路径 `/data/other_vol/koakuma/aliases.conf`，可用 `--aliases <路径>` 或环境变量 `KOAKUMA_ALIASES` 指定；文件修改后约一个采样周期（10 秒）内自动生效，无需重启。示例见 `docs/aliases.conf.example`。

### procd 服务（OpenWrt）

仓库提供 `scripts/koakuma.init`，安装为开机自启服务：

```sh
cp scripts/koakuma.init /etc/init.d/koakuma
chmod +x /etc/init.d/koakuma
/etc/init.d/koakuma enable
/etc/init.d/koakuma start
```

### 访问

浏览器打开 `http://<路由器地址>:8686/` 即可查看分设备实时速率与历史曲线。

### 数据接口

- `GET /api/devices`：设备清单（id / mac / ip / name / last_seen）
- `GET /api/latest`：最近一次采样的每设备实时速率（字节/秒）
- `GET /api/series?dev=<id>&layer=<a|b|c>&from=<unix>&to=<unix>`：指定设备与精度层的历史序列
- `GET /api/session`：返回 `{authRequired, authenticated}`
- `POST /api/login`：JSON `{username,password}`，成功后下发会话 Cookie
- `POST /api/logout`：清除会话

启用鉴权时，除 `/api/session`、`/api/login`、`/api/logout` 外的数据接口在未登录时返回 `401`。

## Web 界面

浏览器打开 `http://<路由器地址>:8686/` 即可使用：

- 设备多选：按芯片勾选，支持全选/清空；自动标记「★本机」并默认选中；
- 实时速率：所选设备的下载/上传速率（多选显示合计），每 5 秒刷新；
- 历史曲线：分开查看（每设备一图）或叠加查看（同图多色），叠加时可按下载/上传过滤；
- 时间范围：预设范围自动刷新；也可自定义时间段查看（此时暂停自动刷新，点「重置」恢复）；
- 明暗主题：默认跟随系统，可手动切换并记忆。

界面的布局分区、字号、配色、圆角与动效统一遵循 [docs/frontend-design.md](./docs/frontend-design.md)；修改前端布局/样式前请先阅读该规范。

## 许可证

见 [LICENSE](./LICENSE)。