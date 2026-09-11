# Monitor 技术架构

状态：Phase 1 基线  
日期：2026-09-06

## 1. 架构目标

Monitor 采用一个 Server、多个只出站连接的 Agent、一个嵌入式前端和一个 SQLite 文件。架构优先级为：安全边界清楚、热路径无数据库、组件少、失败行为可解释。当前需求不能证明必要的层次和服务一律不引入。

明确不使用微服务、Redis、消息队列、gRPC、protobuf、WebSocket Agent 通道、ORM、SQLx 宏、插件、任务执行协议或多租户抽象。

## 2. 总体结构

```text
Linux VPS                           Monitor Server
┌──────────────────┐               ┌─────────────────────────────────┐
│ monitor-agent    │  HTTPS JSON   │ Axum router                     │
│ /proc + /sys     ├──────────────►│  agent auth + validation        │
│ ICMP / TCP probe │               │              │                  │
└──────────────────┘               │              ▼                  │
                                   │ SnapshotStore + traffic state   │
Browser                            │      │             │             │
┌──────────────────┐  HTTP        │      │ 2s          │ 60s        │
│ embedded React   ├──────────────►│ public JSON   history writer     │
│ polling + uPlot  │               │ cache              │             │
└──────────────────┘               │                    ▼             │
                                   │              SQLite (WAL)       │
                                   └─────────────────────────────────┘
```

Agent 与 Browser 都不能直接访问数据库。Server 没有连接 Agent 的反向通道，所以从协议层面不存在远程命令入口。

## 3. 建议仓库布局

```text
Monitor/
├── Cargo.toml
├── Cargo.lock
├── crates/
│   ├── monitor-common/    # 仅共享两个 Rust binary 实际共用的类型
│   ├── monitor-server/
│   │   └── src/
│   └── monitor-agent/     # Phase 3 增加
│       └── src/linux/
├── migrations/
├── web/                   # React/Vite/Tailwind
│   └── src/
├── docs/reference/        # 权威截图
├── packaging/
│   ├── install.sh
│   ├── monitor-server.service
│   └── monitor-agent.service
├── scripts/
│   ├── bench-report.sh
│   └── bench-status.sh
├── SPEC.md
├── ARCHITECTURE.md
├── DATABASE.md
└── README.md
```

`monitor-common` 不是通用 SDK；只有 Server 和 Agent 确实共同使用的 wire types 才放入。数据库 row、前端 view model 和业务 service 不放入该 crate。Phase 2A 尚无跨 binary 类型，因此该 crate 不提前加入 DTO 或依赖。

## 4. Rust 模块

### 4.1 `monitor-server`

```text
main.rs                 CLI、启动、优雅关闭
config.rs               --listen、--db 与固定安全上限
app.rs                  AppState 组装和后台 worker 生命周期
http/
  mod.rs                Router、body limit、错误映射
  public.rs             snapshot/history/ping
  agent.rs              bearer auth、config、report
  auth.rs               login/logout/me、cookie、rate limit
  admin.rs              nodes、ping targets、settings、password
auth.rs                 Argon2id、session、CSRF、Origin 校验
db/
  mod.rs                rusqlite 连接、PRAGMA、migration
  nodes.rs              节点与后台单查询
  history.rs            批量历史读写
  traffic.rs            流量持久化
snapshot.rs             NodeSnapshot、预序列化 public cache
traffic.rs              counter delta 与计费周期
history.rs              分钟资源/Ping 聚合器和清理 worker
static_files.rs         嵌入资源、ETag、br/gzip 协商、SPA fallback
time.rs                 UTC 分桶、IANA 站点时区与计费周期边界
error.rs                小型统一错误类型
```

模块按真实数据边界拆分，不再增加 repository/service/controller 三层包装。HTTP handler 调用明确的数据库函数或内存组件即可。

### 4.2 `monitor-agent`

```text
main.rs                 CLI、信号与主循环
config.rs               --server、--token；Server config cache
collector.rs            一次采样的组合
linux/
  proc_stat.rs
  meminfo.rs
  loadavg.rs
  netdev.rs
  cpuinfo.rs
  os_release.rs
  disk.rs
  virtualization.rs
ping.rs                 ICMP v4/v6 与 TCP connect 探测，单轮统一超时
reporter.rs             单一 keep-alive HTTP client、重试
```

Linux parser 接受字符串/reader 输入，便于用固定 fixture 单元测试。生产代码不为 Windows/macOS/FreeBSD 提供 trait 或 stub。

## 5. Server 状态模型

`AppState` 只持有以下长期对象：

- `Db`：专用 database worker thread 独占一个 rusqlite connection；Tokio 侧通过有界 channel 提交具体数据库操作并异步等待结果。
- `NodeTokenCache`：`SHA-256(token) -> node_id`，启动时一次装载，Node 创建/删除/token 轮换时同步更新。
- `NodeMetaCache`：公开页面需要的节点配置；只在后台 mutation 后更新。
- `SnapshotStore`：`RwLock<HashMap<NodeId, NodeSnapshot>>`，每次 report 做短临界区更新。
- `TrafficState`：每节点 raw counter、累计值、今日值、当前周期值和待持久化 delta。
- `HistoryAccumulator`：当前分钟的资源与 Ping 小型聚合器。
- `PublicSnapshotCache`：已序列化 JSON bytes、ETag、生成时间。
- `AgentConfigCache`：报告间隔、Ping 间隔和最多六个启用 target。
- `LoginLimiter`：按来源 IP 的小型内存失败计数表。

不把 30 天历史载入内存。允许的短期内存只有当前 snapshot、当前分钟聚合、全站约 60 个 sparkline 点和必要配置。

选择标准库 `RwLock/Mutex`，先避免 DashMap、ArcSwap 等额外依赖。报告临界区内不做 await、JSON 序列化或 SQLite 操作；若 Phase 7 的真实 benchmark 证明锁竞争是瓶颈，再用数据证明替换。

## 6. Tokio 与 SQLite

- Axum 运行在 Tokio runtime 上；SQLite 操作不占用 runtime worker。
- rusqlite connection 只存在于一个专用 OS thread。一个容量受限的 Tokio channel 接收具体持久化命令，oneshot 返回结果；不引入连接池、通用 executor abstraction 或 `Arc<Mutex<Connection>>`。
- handler 不跨 await 持有数据库或 snapshot 锁。
- 数据库函数一次完成完整查询或事务；尤其 `GET /api/admin/nodes` 必须用一个 JOIN 查询取回节点、最后状态、总流量、今日与当前周期数据。
- Agent report 热路径不触发 SQLite。token 校验、snapshot、traffic delta 和分钟聚合都在内存完成。

Phase 2A 的启动顺序固定为：worker 打开 connection、集中设置 PRAGMA、事务执行 migration、确保默认 settings；随后 Tokio 侧依次读取 settings、node metadata、traffic recovery state 并构造 `AppState`，最后才 bind/serve HTTP。未来同时修改 SQLite 与 cache 的操作必须先 commit transaction，再更新 cache。

如果数据库暂时写失败，当前 snapshot 仍继续服务；writer 保留未提交 traffic delta 并退避重试。认证/后台 mutation 的数据库失败直接返回 503，不能假装成功。

## 7. Agent report 热路径

处理顺序固定：

1. 限制 request body 大小（首版 32 KiB）；
2. 从 Authorization 取 token，SHA-256 后在内存 cache 查找；
3. 解析固定版本 JSON；
4. 校验字符串长度、枚举、数值范围、有限浮点、Ping target 所属和最多 6 项；
5. 取 Server `received_at` 与来源 IP；
6. 在短锁内计算 traffic delta、更新 `NodeSnapshot`、分钟资源聚合与 Ping 聚合；
7. 标记 public cache dirty；
8. 返回 204。

不在该路径执行：Argon2、SQL、DNS、Ping、历史查询、静态文件处理或全站 JSON 序列化。

Server 默认使用 socket peer IP。只有直接 peer 是 loopback 时才接受反向代理写入的单值 `X-Real-IP`，防止公网直连时伪造地址。

## 8. 流量累计算法

每个方向独立处理。持久状态包含累计值 `total`、上次 Agent raw counter `last_raw` 与 `last_boot_id`。

```text
没有 last_raw:
    delta = 0                    # 首次安装从零开始计
同 boot_id 且 raw >= last_raw:
    delta = raw - last_raw
boot_id 改变，或 raw < last_raw:
    delta = raw                  # 重启/计数器 reset 后已产生的流量

total = checked_add(total, delta)
last_raw = raw
last_boot_id = current_boot_id
```

所有加法使用 checked arithmetic，并在进入 SQLite 前限制到 signed 64-bit 正整数范围。任何异常都不得写入比旧 total 更小的值。

这个模型也能跨 Server 重启恢复：若 Server 在一分钟持久化前崩溃，数据库仍保留较旧 `last_raw`；重启后新 raw 与旧 raw 的差正好重新覆盖未提交区间，不丢失、不重复。检测到 boot ID 变化时额外唤醒 traffic writer，缩短连续多次重启的风险窗口。

### 8.1 并发持久化

每个节点 traffic state 带递增 `generation`。writer 在锁内复制一代完整绝对值、对应 raw/boot、日/周期 key 与 generation 后立即解锁，再以绝对值 UPSERT。事务成功后记录 `persisted_generation = captured_generation`；期间若新 report 已令 generation 更大，节点保持 dirty 并在下一次继续写。事务失败则 persisted generation 不动。绝对值 UPSERT 使“事务已提交、进程尚未更新 dirty 状态”的重复写仍然幂等。

同一事务更新：

- `traffic_totals` 的累计值与最后 raw/boot；
- 当前站点时区自然日的 `traffic_daily`（边界保存为 UTC epoch）；
- 当前 billing period 的 `traffic_cycles`；
- `node_last_state` 的最后离线回显数据。

站点时区的日/周期边界到来时立即切换内存 bucket 并唤醒 writer。修改节点 reset day 或站点时区只影响变更后的新归属，不改写已有累计总量。

## 9. 历史聚合

### 9.1 资源

每个 report 更新当前 UTC 分钟 accumulator：CPU/load/network rate 累计求平均，memory/swap/disk 保留本分钟最后值。每分钟一次事务批量 upsert 所有节点的 `node_history`，而不是把每 2 秒样本全部写入。

资源历史默认且最多保留 30 天。清理 worker 每小时按 node 主键范围删除过期行，每批限制行数，避免长事务；不自动 VACUUM。

### 9.2 Ping

Agent 10/15 秒采样只进入当前分钟 accumulator。每个 `(node_id, target_id, minute)` 保存：成功延迟 sum/min/max、success_count、sample_count。持久化时：

- `success_count > 0`：写平均/最小/最大 milliseconds；
- `success_count = 0`：三个 latency 字段均写 NULL；
- 丢包率由 `(sample_count - success_count) / sample_count` 查询时计算，作为 0..1 比例返回，不是百分比整数。5 分钟降采样先累加两个 count 再除一次，不取各分钟比例的平均值。
- ping history 响应还合入仍在累积的当前分钟：`HistoryAccumulator` 提供一个只复制计数值的短锁方法，锁不跨 SQLite await 或响应构造，当前分钟也不会为了可查询而落盘。

Ping 固定保留 7 天，覆盖最长 UI range，同时把典型 7 节点 × 6 target 的数据库控制在几十 MiB 量级。不增加 Ping 保留期设置。

### 9.3 查询降采样

1h/6h/24h 直接读 60 秒桶。7d 在 SQLite 单次查询中按 300 秒分组；CPU/load/rate/latency 用有效样本加权平均，memory/swap/disk 用最后或平均的稳定规则，Ping 全失败桶保持 NULL。前端不下载 10,080 个点后再盲目处理。

## 10. Public snapshot cache

每 2 秒或 dirty 后的下一 tick：

1. 短暂读锁复制节点 snapshot、metadata、traffic current values；
2. 在锁外计算在线状态、summary 与排序；
3. 追加全站 RX/TX rate 到 60 点 ring buffer；
4. 序列化一次 JSON；
5. 以新的 immutable bytes + ETag 替换 cache。

`GET /api/public/snapshot` 只复制 bytes 引用并设置 header，不进入 SQLite，也不为每个请求重新遍历节点。100 个浏览器刷新不会把数据库工作放大 100 倍。

首页先显示结构完全一致的 skeleton/shell，再开始 2 秒轮询。浏览器标签隐藏时可降到 10 秒；恢复可见后立即刷新。此优化只影响 Browser，不改变 Agent 或 Server 数据频率。

## 11. API 错误与兼容策略

- API 前缀固定 `/api`，首版 report body 含 `version: 1`。
- 不承诺 Komari 或第三方兼容。
- 成功创建节点返回 201 和一次性明文 token；后续 GET 永不返回 token/hash。
- 验证错误使用 400；未认证 401；无 CSRF/Origin 403；不存在 404；冲突 409；限流 429；数据库暂不可用 503。
- 错误 JSON 只有稳定的短 `code` 和用户可读 `message`，不回传 SQL、文件路径、hash 或内部栈。
- Public 资源只允许 GET/HEAD；Admin mutation 只允许 JSON 并校验 Content-Type。

## 12. 认证与安全边界

### 12.1 Admin 密码

- 首次运行无密码时，后台登录始终拒绝，并提示在 Server 主机执行 `monitor-server --db ./monitor.db admin set-password`。
- 密码只以 Argon2id PHC 字符串保存；参数在实现时用目标 Server 实测，目标校验耗时约 100–250ms，不能在 report 路径使用。
- 修改密码在一个事务中更新 hash 并删除所有 session，当前浏览器也需重新登录。

### 12.2 Session 与 CSRF

- Session token 为至少 256 bit CSPRNG 随机值；数据库只保存 SHA-256 hash，CSRF 比较使用 `subtle`。
- Cookie 使用 `__Host-` 前缀、Secure、Path=/、无 Domain、HttpOnly（session）和 SameSite=Strict。
- CSRF 使用双提交 cookie：非 HttpOnly CSRF cookie 必须与 `X-CSRF-Token` 常量时间比较，同时 Origin 必须与有效 Host 一致。
- Session 采用固定 7 天过期，不在每个请求滚动写 SQLite；过期清理每小时运行。
- 登录 limiter 默认同一来源 5 次失败后冷却 5 分钟，成功即清除；表只在内存中且定期清理。

### 12.3 Agent token

- token 为至少 32 random bytes，使用 64 位小写 hex 编码，避免增加 base64 依赖。
- 数据库和 cache 只持有 SHA-256；高熵 token 不需要 Argon2。
- hash 查找后仍按固定字节比较；日志永不记录 Authorization 或安装命令。
- token 轮换是单事务：先写新 hash，再更新 cache；旧 token 随即失效，明文只返回一次。

### 12.4 HTTP 与进程

- 所有请求设置 body limit、合理 header timeout（由反向代理和 Server 层共同负责）与安全响应 header。
- HTML 设置严格 CSP：script 默认只允许 self，唯一主题启动脚本以构建期固定 SHA-256 hash 放行；不允许其他 inline script、外部字体或第三方 analytics。
- Server 不以 root 运行。Agent 使用专用低权限用户，仅授予 ICMP 所需 `CAP_NET_RAW`；TCP 探测不需要额外能力。
- Agent systemd hardening 至少启用 `NoNewPrivileges=true`、`PrivateTmp=true`、`ProtectSystem=strict`、`ProtectHome=true`、`ProtectKernelTunables=true`、`ProtectKernelModules=true`、`ProtectControlGroups=true`；通过 `ReadOnlyPaths=/proc /sys /etc` 不应阻止读取，实际在 Debian/Ubuntu/Alpine 验证。

## 13. 前端架构

### 13.1 状态

不引入全局状态库或查询库：

- Public snapshot：一个 `useSyncExternalStore` 小型 fetch store，负责去重、轮询、可见性和最后成功值。
- Auth：React context，只包含 `me`、login/logout 与 CSRF header helper。
- 主题：localStorage 中用户选择（light/dark/system）；没有选择时用 Server `theme_default`，system 监听 `matchMedia`。
- 页面表单：组件本地 state。

### 13.2 Routing 与 chunks

七个固定路由由本地小型 History API router 管理（`pushState`、`popstate`、固定 pattern match），不引入 `react-router-dom`。`React.lazy` 切分详情、登录和后台；uPlot 直接在一个本地 React wrapper 中实例化，不增加 `uplot-react`。lucide 只做具名 icon import。

Vite 输出带 hash 文件名，并在构建阶段生成 Brotli/Gzip 文件。Server 嵌入 `web/dist`，根据 `Accept-Encoding` 选择预压缩变体；不运行时压缩。

### 13.3 UI primitives

参考 shadcn 的可访问状态、focus ring 和紧凑尺寸，但代码在本仓库手写：

- Button：default/outline/ghost/destructive，32/36px 两种高度；
- Card：单层 border + 极轻 shadow；
- Badge：outline/default/status；
- Input：36px；
- Dialog：原生 `<dialog>`；
- Tabs：button + ARIA tab semantics；
- Table：原生 table + overflow container；
- Tooltip：hover/focus CSS，必要时 portal；
- Dropdown：button + 小型受控 menu；
- Switch：button role=switch。

不复制 shadcn registry 依赖链。LuminaPlus 中的外部 Inter、React Query、Zod、背景/透明度系统、指标彩虹色、Komari schema/WebSocket 代码全部不采用。

## 14. 静态资源与缓存

- `index.html`：`Cache-Control: no-cache`，ETag；所有非 API 前端路由 fallback 到它。
- hashed JS/CSS：`public, max-age=31536000, immutable`。
- `.br` 优先于 `.gz`；返回正确 `Content-Encoding`、`Content-Type`、`Vary: Accept-Encoding`。
- favicon 等非 hashed 小文件使用短 cache + ETag。
- API：Public snapshot 可用 `no-cache` + ETag；认证和后台响应 `no-store`。
- 不嵌入 source map 到 release 二进制。

## 15. 依赖预算

首版预期直接依赖控制如下，具体版本在 Phase 2 锁定并审计：

### Server/runtime

- `tokio`、`axum`；
- `serde`、`serde_json`；
- `rusqlite`（bundled SQLite，保证单二进制部署）；
- `argon2`、`getrandom`、`sha2`、`subtle`；
- `jiff`（启用 `tzdb-bundle-always`，用于 `site_timezone` 的自然日和计费周期边界，避免依赖宿主 `/usr/share/zoneinfo`）；
- `rust-embed`（仅编译期嵌入 `web/dist`）；
- `tracing`、`tracing-subscriber`（只启用 fmt 所需 feature）。

不先加入数据库 pool、ORM、tower-http 全套 feature、chrono、chrono-tz、uuid、rand、base64、hex、cookie、CVA 等。随机字节直接取 `getrandom`，hex/cookie header 用很小的本地函数；外部 node id 使用 CSPRNG 生成的 16-byte hex。时间戳仍是 UTC epoch，只有流量日/周期边界通过 `jiff` 解释 IANA 站点时区。

### Agent/runtime

- `serde`、`serde_json`；
- `ureq`（只启用 keep-alive HTTPS/rustls 所需 feature）；
- `socket2`/`libc` 用于 ICMP、TCP 探测与 `statvfs`；
- CSPRNG 不是 Agent 必需依赖。

Agent 主循环可用阻塞 I/O 和单线程定时，不为简单周期任务引入 Tokio。HTTP client 必须复用连接并关闭 HTTP/2/不必要 feature。Phase 3 以 Alpine musl 与 Debian glibc 的 RSS/二进制体积实测决定具体 client，不能凭假 benchmark 宣称达标。

### Frontend/runtime 与 build

- 浏览器 runtime：`react`、`react-dom`、`uplot`、`lucide-react`；
- build-only：`typescript`、`vite`、`tailwindcss`、`@tailwindcss/vite`、`@types/react`、`@types/react-dom`；
- test-only：`vitest`。

不使用 `react-router-dom`、`uplot-react`、React Query、Zod、clsx/CVA、Radix、完整 shadcn 包、CSS-in-JS 或压缩插件。预压缩由 Node 内置 `zlib` 的仓库脚本完成，生产不存在 Node。

### Rust test-only

- `tower` 的 `util` feature 用于对真实 Axum Router 发测试请求；
- `tempfile` 用于隔离并行 SQLite/API tests。

test/build dependency 不进入 release 运行路径；Phase 7 仍审计 lockfile 和重复 transitive crate。

## 16. 运行与关闭

Server 启动顺序：解析 CLI → 打开 DB/PRAGMA → migration → 装载并验证站点时区/其他设置/节点/token/最后状态 → 构建初始 snapshot → 启动 workers → 开始监听。

优雅关闭时：停止接收新请求 → 最后一次 traffic/history flush → passive WAL checkpoint → 关闭连接。若最后 flush 失败，以非零状态退出并记录不含 secret 的错误。

Agent 启动：读取参数 → 读取静态系统信息 → 拉取 config → 建立采样 baseline → 定时采集/上报与 Ping。网络失败使用有上限指数退避（2s 到 60s）并带小随机抖动；成功后恢复正常 2 秒节奏。Agent 不缓存待补传历史，避免磁盘状态和重复上报。

## 17. 测试与审计结构

- parser 单测使用 `crates/agent/tests/fixtures/` 的最小 Linux 样本。
- traffic/time 为纯函数，表驱动覆盖 reset、Server restart、溢出与 1–31 日。
- API tests 用临时 SQLite 文件启动真实 Router，不能 mock 掉 auth/transaction。
- migration test 从空 DB 创建并检查 `foreign_key_check`、index 和 `user_version`。
- 前端格式化、range、offline、cut-peak 为纯函数测试；视觉以权威 viewport screenshot diff 人工复核。
- `scripts/bench-report.sh` 模拟 200 节点/2 秒与独立 1000 reports/sec；`bench-status.sh` 测 snapshot requests/sec、p50/p95/p99、RSS、CPU。脚本输出真实环境信息和原始结果，不把目标当结果。

Phase 7 的删除审计逐项检查 Cargo direct dependency、Vite chunk、未引用 primitive、重复 DTO 与无意义 wrapper。只有数据证明必要时才增加更复杂并发结构。

## 18. 已接受的限制

- timestamp、资源/Ping 分钟桶仍使用 UTC；今日与 billing boundary 使用唯一 `site_timezone`（默认 `Asia/Shanghai`）解释，浏览器仅负责本地时间显示。
- 公开首页采用 2 秒单接口轮询；首版不做 SSE。
- 只监控根文件系统与默认路由网络接口；不提供磁盘/网卡选择 UI。
- Ping 数据只保留最长页面范围所需的 7 天；资源保留期最多 30 天。
- 明文 Agent token 只显示一次；再次取得安装命令需要轮换 token。
- 没有告警、通知、远程动作和 Agent 自动更新；手动替换 Agent 二进制是唯一升级方式。
