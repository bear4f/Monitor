# Monitor 产品与界面规格

状态：Phase 1 基线  
日期：2026-09-06  
适用仓库：`bear4f/Monitor`

## 1. 产品边界

Monitor 是从零实现的、自用型 Linux VPS 监控系统。它不是 Komari fork，不兼容 Komari API、Agent、数据库或主题格式。

所有设计按以下顺序裁决冲突：

1. `docs/reference/` 中的截图；
2. 本规格中的明确要求；
3. LuminaPlus 中与截图一致的排版经验；
4. shadcn/ui 的中性组件尺度。

项目只有九类功能：

1. 节点基本信息；
2. CPU、内存、磁盘、网络状态；
3. 历史资源曲线；
4. 电信、联通、移动的 IPv4/IPv6 延迟；
5. 不因 Agent 或 VPS 重启清零的累计流量；
6. 今日流量与当前计费周期流量；
7. 节点价格与到期时间；
8. 单管理员后台；
9. 浅色、深色、跟随系统三种显示模式。

未列出的功能默认不做。尤其不实现远程命令、Shell、文件管理、Agent 自动升级、插件、通知、告警中心、多用户、RBAC、注册、OAuth、第三方登录、容器/进程/服务管理、日志查看或任何远程机器变更能力。

## 2. 交付形态与平台

- Server：Linux x86_64、Linux arm64；单个 `monitor-server` 二进制，运行时只需要可写的 SQLite 文件。
- Agent：仅 Linux，重点验证 Debian、Ubuntu、Alpine；单个 `monitor-agent` 二进制。
- Web：React + TypeScript + Vite + Tailwind CSS 的构建产物嵌入 Server，不依赖生产环境 Node.js、外部字体或 CDN。
- TLS：由 Nginx 或 Caddy 终止；Server 默认监听回环地址，不实现 ACME。
- 数据：一个 SQLite 数据库。备份对象只有该数据库；使用 SQLite 在线备份或短暂停服复制，不能只复制仍在活跃写入的主文件而忽略 WAL。

## 3. 角色与核心流程

### 3.1 访客

访客无需登录即可查看状态首页、节点当前详情、资源历史和 Ping 历史。所有首页数据通过一次 `GET /api/public/snapshot` 获得。

### 3.2 管理员

系统只有一个管理员、一个密码、一个后台会话模型。管理员可以：

- 新增、编辑、排序、删除节点；
- 新增、编辑、启停、删除 Ping Target；
- 设置站点名称、站点时区、默认主题和五项运行参数；
- 修改管理员密码；
- 创建节点后一次性取得 Agent token 和安装命令；
- 丢失 token 时显式轮换 token，再取得新的安装命令。

数据库永不保存 Agent 明文 token。因此节点创建弹窗关闭后不能恢复旧 token；后台的后续“重新生成安装命令”操作会撤销旧 token，并必须确认这一影响。

### 3.3 Agent

Agent 只读取本机状态、执行 ICMP 探测、读取 Server 配置并上报。Agent 不接受 Server 主动命令，不暴露监听端口，不保存历史数据。

## 4. Agent 采集规格

默认每 2 秒采集并上报：

- hostname；
- OS 名称与版本；
- kernel；
- architecture；
- CPU model、逻辑核心数、CPU usage；
- load average（1、5、15 分钟）；
- memory total/used；
- swap total/used；
- 根文件系统 disk total/used；
- 默认路由网卡的 RX/TX 原始累计字节与字节/秒；
- uptime；
- process count；
- virtualization；
- Linux boot ID；
- Agent version。

采集优先读取 `/proc/stat`、`/proc/meminfo`、`/proc/loadavg`、`/proc/uptime`、`/proc/net/dev`、`/proc/cpuinfo`、`/proc/sys/kernel/random/boot_id`、`/etc/os-release` 与 `/sys/`。根文件系统容量使用 `statvfs`。网络接口优先取默认路由接口；无默认路由时才合计除 `lo` 外的接口，避免桥接/VETH 重复计数。

CPU 使用率和网络速率由相邻采样的 counter delta 计算。首次采样、计数器回退或计时异常时，速率显示为 0，本次不产生负值。

所有数字在 Agent 和 Server 边界做范围校验。Server 以收到报告的 UTC 时间作为在线判定和资源历史的权威时间，避免 Agent 时钟漂移污染历史。

## 5. Ping 规格

- 最多配置 6 个启用目标，语义为电信 v4/v6、联通 v4/v6、移动 v4/v6；不实现复杂规则、探测组或告警。
- Target 字段：显示名称、IP/Host、IP family（4 或 6）、启用状态、排序。
- 默认每 15 秒探测；允许设置为 10 秒或更慢。
- Agent 使用 ICMP Echo；IPv4/IPv6 family 必须与 Target 配置一致。
- 上报项只包含 `target_id`、`success`、`latency_ms`。成功时 latency 为非负数；失败/超时时必须是 `null`，禁止用 `0` 代替。
- Agent 不排队补传历史 Ping。Server 按收到时间写入当前分钟的内存聚合器，每分钟持久化平均/最小/最大延迟与成功/总样本数。
- 图表断点使用 `null`，uPlot 的 `spanGaps` 保持关闭，失败区间不能连到 0 ms。
- “削峰”仅在浏览器显示层生成裁剪后的 series，API 和数据库原始聚合值不变；tooltip 同时保留原始值标识。

## 6. 流量语义

全项目方向命名固定如下：

| 语义 | Linux counter | API/数据库前缀 | UI |
| --- | --- | --- | --- |
| 下载/入站 | RX | `rx_` | `↓` |
| 上传/出站 | TX | `tx_` | `↑` |

每个节点显示：实时下载/上传速度、今日下载/上传、当前计费周期下载/上传、历史累计下载/上传、额度及当前周期占比。

- `traffic_limit_bytes = null` 表示无限，UI 显示 `∞`，不绘制百分比数字。
- 额度使用量固定为当前 billing cycle 的 `rx_bytes + tx_bytes`；后台表格和节点卡片的单一“流量”进度均采用这个合计值，今日/累计仍分别显示 RX 与 TX。
- 总流量只增不减，计费周期切换不修改总流量。
- `site_timezone` 使用 IANA 时区名，默认 `Asia/Shanghai`。所有 timestamp 仍以 UTC epoch 保存。
- “今日”按 `site_timezone` 的本地自然日计算；日开始/结束先在该时区解释，再转换为 UTC timestamp 查询与持久化。
- 月重置日允许 1–31。某月没有该日时使用该月最后一天；billing period 的本地午夜边界同样按 `site_timezone` 解释后转为 UTC。
- 浏览器中的日期和时刻按用户本地时区显示，但“今日流量”和计费周期的归属口径始终使用站点时区，避免不同访客看到不同用量。
- 不提供“清零总流量”功能。删除整个节点会级联删除其数据，并必须经过确认。

## 7. 在线与离线

- Server 的 `last_seen_at` 使用报告接收时间。
- 默认超过 10 秒未收到报告即离线；比较条件为 `now - last_seen_at > offline_after_seconds`。
- 在线时长展示 Agent uptime，而不是从首次连接时间推算。
- `first_seen_at` 与 `last_seen_at` 分开保存。
- 离线节点保留最后一份已知数据，整体降低不透明度/对比度，但名称、地区、最后资源数据和累计流量仍可读。

## 8. 公开 API 契约

所有时间戳为 Unix UTC 秒，bytes 为非负整数，速率为 bytes/second，延迟为 milliseconds，百分比不带 `%` 字符串。

### 8.1 `GET /api/public/snapshot`

一次返回：

- 站点名称、默认主题、Server 时间；
- 在线/总节点数；
- 在线节点中 CPU 最高的节点；
- 全节点今日 RX/TX、历史总 RX/TX；
- 在线节点实时 RX/TX 总速率；
- 最近约 120 秒的全局速率 sparkline；
- 按 `sort_order, id` 排序的所有节点元数据、最后状态、流量状态和在线状态。

该响应从内存中的预序列化快照读取，不为每个浏览器查询 SQLite。首页默认每 2 秒只轮询这一接口；Phase 1 不引入 SSE。

### 8.2 历史接口

- `GET /api/public/nodes/:id/history?range=1h|6h|24h|7d`
- `GET /api/public/nodes/:id/ping?range=1h|6h|24h|7d`

1h、6h、24h 返回 60 秒桶；7d 在查询时聚合为 5 分钟桶，将单条 series 控制在约 2,016 点。不存在的样本为 `null`。

## 9. Agent、认证与后台 API

### Agent

- `POST /api/agent/report`
- `GET /api/agent/config`

两者使用 `Authorization: Bearer <node_token>`。报告 body 有固定版本号和大小上限；配置从内存缓存读取。

### Auth

- `POST /api/auth/login`
- `POST /api/auth/logout`
- `GET /api/auth/me`

登录只提交密码。成功后设置 `__Host-monitor_session` HttpOnly/Secure/SameSite=Strict cookie 和非 HttpOnly 的 `__Host-monitor_csrf` Secure/SameSite=Strict cookie。所有已登录 mutation 同时校验 Origin、CSRF cookie 与 `X-CSRF-Token`。

### Admin

- `GET /api/admin/nodes`
- `POST /api/admin/nodes`
- `PATCH /api/admin/nodes/:id`
- `DELETE /api/admin/nodes/:id`
- `POST /api/admin/nodes/:id/rotate-token`（必要的显式 token 轮换）
- `GET /api/admin/ping-targets`
- `POST /api/admin/ping-targets`
- `PATCH /api/admin/ping-targets/:id`
- `DELETE /api/admin/ping-targets/:id`
- `GET /api/admin/settings`
- `PATCH /api/admin/settings`
- `PATCH /api/admin/password`

拖动排序通过节点 `PATCH` 的目标 `sort_order` 完成，Server 在单个事务内移动受影响记录，不增加批量排序接口。

## 10. 前端路由与加载边界

| 路由 | 页面 | 加载策略 |
| --- | --- | --- |
| `/` | 状态首页 | 初始 bundle |
| `/nodes/:id` | 节点详情 | lazy |
| `/login` | 管理员登录 | lazy |
| `/admin/nodes` | 节点管理 | lazy，共享后台 shell |
| `/admin/ping-targets` | 延迟监控 | lazy |
| `/admin/theme` | 默认主题 | lazy |
| `/admin/settings` | 设置与密码 | lazy |

uPlot 只由节点详情 chunk 导入。后台、详情页和 mock 数据不得进入首页初始 chunk。开发环境 `?mock=1` 提供正常、高负载、离线、无限流量、即将到期和不同地区节点；mock 模块受 `import.meta.env.DEV` 静态条件保护，生产构建不得生成 mock chunk。

只实现实际使用的 Button、Card、Badge、Input、Dialog、Tabs、Table、Tooltip、Dropdown、Switch。优先原生元素：Dialog 使用 `<dialog>`，Tooltip 使用 CSS + ARIA，Table 使用原生表格；不安装完整 shadcn、Radix、CVA、TanStack Query、Zod 或大型状态库。

## 11. 参考图视觉基线

权威截图：

- `docs/reference/01-overview.png`：公开首页，2048×932；
- `docs/reference/02-node-resource-top.png`：节点详情顶部与 CPU/内存，2047×941；
- `docs/reference/03-node-resource-bottom.png`：网络与磁盘图表，2047×938；
- `docs/reference/04-node-latency.png`：Ping 图表，2048×942；
- `docs/reference/05-admin-nodes.png`：后台节点表格，1660×948。

### 11.1 通用画布

- 系统字体栈：`-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, "PingFang SC", "Microsoft YaHei", sans-serif`。
- 全局启用 tabular numerals；不下载 Inter 或 Google Fonts。
- 浅色背景接近纯白，正文近黑，辅助文字中灰，边框浅灰；颜色只承载状态和低饱和 Ping series。
- 深色模式使用中性黑灰反转层级，不使用蓝紫大底、Glow、玻璃、纹理或渐变。
- 页面不出现营销 hero、巨大标题、装饰插画、浮动动画或大面积阴影。
- 交互动画限制为 100–150ms 的颜色/不透明度变化；遵守 `prefers-reduced-motion`。

### 11.2 可量化尺寸

以下是依据截图测得的实现起点；Phase 4 以同尺寸截图逐像素复核后可小幅修正：

| 部位 | 桌面基线 |
| --- | --- |
| 公开页 header | 约 86–90px 高，1px 底边 |
| 公开页水平留白 | 2048px 视口约 64px |
| 首页栅格 gap | 约 18px |
| Summary card | 四列，约 160px 高，16–18px 圆角，18px 内边距 |
| Node card | 四列时约 470px 宽、400px 高，16–18px 圆角，24px 内边距 |
| Node card 指标 | 两列，列间约 22px；6–8px 高进度条 |
| Header/卡片主标题 | 22–24px / 22–24px，600–700 字重 |
| 正文/辅助文字 | 16–18px / 14–16px，行高紧凑 |
| Badge | 30–32px 高或内容自适应，圆角 pill，1px 边框 |
| 详情页主内容 | 约 64px 左右留白，图表无外层 Card |
| 图表 | 1–1.5px 线，浅灰面积，1px 虚线 grid，无入场动画 |
| 后台 | 约 1400px 居中内容，约 190px sidebar，主表格单面板 |

### 11.3 首页

- Header 左侧只显示站点名；右侧是登录（lucide Wrench/Login 图标语义）和主题按钮。
- 顶部四卡固定顺序：节点、最忙节点、今日流量、实时网速。
- Summary 以数字和紧凑两列数据为主，不增加趋势百分比、迷你说明卡或装饰图。
- 节点卡按 4/3/2/1 列响应：建议断点为 ≥1440、1024–1439、640–1023、<640。
- 卡片头部：名称 + 地区 Badge；右侧在线 Badge。第二行是 OS · virtualization · arch 与到期倒计时。
- 指标区严格为 CPU/Load、内存、磁盘、流量四项；底部分隔后只放实时 RX/TX 与累计 RX/TX。
- 离线样式不得隐藏最后数据，也不使用大红色背景。
- 价格为 0 显示“免费”；无到期日显示 `∞`；有到期日只显示距离天数，详情页才显示金额、周期和完整日期。

### 11.4 节点详情与图表

- 顶部标题行依次为节点名、地区、在线时长、Agent version。
- 信息区为三列，每列两组 label/value；窄屏按两列、单列折叠。
- 资源/网络延迟主 Tab 和时间范围控制使用近黑激活底、白字；不使用彩色 underline。
- 资源图表顺序固定为 CPU、内存、网络速率、硬盘。
- uPlot tooltip 为白/深灰实色小面板、1px 边框、无圆角夸张阴影；时间在上、series 值在下。
- Ping 最多六条低饱和灰蓝/灰绿/灰褐线，以实线/虚线/点线共同区分，不能只靠颜色。
- Legend 位于图表下方居中，采用 outline Badge 外观。

### 11.5 后台

- Header 左侧“Monitor 后台”，右侧仅状态面板、主题、退出。
- Sidebar 只有节点、延迟监控、主题、设置，当前项用很浅的中性底强调。
- 节点表头固定为名称、IP、状态、流量、价格、到期、操作；名称前有 drag handle。
- 操作区只提供安装、编辑、删除；安装 Dropdown 内含复制/重新生成安装命令及 Agent 下载链接。删除为唯一红色操作，并使用确认 Dialog。
- 小屏允许表格横向滚动，不为移动端另造一套信息卡。

## 12. 主题 token

首版只定义语义 token，不开放自定义 CSS/配色编辑器：

| Token | Light | Dark |
| --- | --- | --- |
| canvas | `#fafafa` | `#111113` |
| surface | `#ffffff` | `#18181b` |
| text | `#171717` | `#fafafa` |
| muted text | `#737373` | `#a3a3a3` |
| border/grid | `#e5e5e5` | `#2f2f2f` |
| track/fill | `#ededed` / `#111111` | `#2a2a2a` / `#e5e5e5` |

在线状态使用当前文本色的小圆点，离线使用低饱和红且降低整卡强调。Ping 六色必须在浅/深两套背景上通过对比度检查。

## 13. 性能与容量验收

- Server 空载 RSS 目标：尽可能低于 15 MiB；Agent RSS 目标：尽可能低于 10 MiB。文档只记录实测值。
- 100 节点每 2 秒上报时，报告热路径不访问 SQLite、不执行 Argon2、不重建全站查询。
- 公开快照正常 p50 目标低于 10ms；响应来自预序列化内存数据。
- 节点资源历史最多每 60 秒一行，保留默认/最大 30 天。
- Ping 原始 10/15 秒样本在内存按分钟聚合，持久化 60 秒桶，保留固定 7 天。
- 7 节点、每节点 6 Ping target 时，数据库以几十 MiB 为目标；Phase 7 必须用真实文件验证，而非估算后宣称达标。
- 静态 hashed assets 使用 `public, max-age=31536000, immutable`；`index.html` 使用 `no-cache`；优先发送构建期 `.br`，其次 `.gz`，最后原文件。
- 首页 initial chunk 不包含 uPlot、后台、详情页和开发 mock。

## 14. 必须通过的测试

- Rust 单元测试：Linux parser、CPU delta、网络 delta、流量 reset、计费周期边界、离线判定、输入校验。
- API 测试：登录/限流/cookie/CSRF、Node CRUD、token hash/轮换、report auth/body limit、一次性公开快照、历史 range。
- 关键不变量：同 boot counter 增加只累加 delta；counter 回退或 boot ID 改变时累计总量加当前 counter，永不减少；Server 重启后从已持久化 raw counter 继续。
- 月重置测试覆盖 1 日、15 日、29–31 日、闰年和 DST 切换；周期切换只切换周期用量，不影响总量。
- 离线测试使用 Server 接收时间，不依赖 Agent 时钟。
- 前端视觉测试至少覆盖 2048×932、1660×948、1440×900、1024×768、390×844 以及浅/深主题。

## 15. 开发阶段闸门

1. Phase 1：只提交本规格、架构、数据库设计和权威参考图。
2. Phase 2：完成 workspace、SQLite、单管理员认证、Node CRUD、report、snapshot、traffic 与测试；不做 Agent 采集和正式 UI。
3. Phase 3：完成 Linux Agent 与采集/ICMP 可靠性。
4. Phase 4：只把公开首页做到与参考图高度一致。
5. Phase 5：节点详情、资源图和 Ping 图。
6. Phase 6：四个后台页面。
7. Phase 7：性能、数据库、bundle、安全审计，删除未用依赖、抽象和重复代码，再运行真实 benchmark。

每个阶段未通过测试和审计前，不提前展开后续阶段。

## 16. 参考实现审查结论

Phase 1 已检查 shadcn/ui 的 Button、Card、Badge、Tabs、Table 等 current registry 源码，以及 LuminaPlus 的 home/node card tokens、NodeCard、Instance/Ping chart 与依赖清单。

- 从 shadcn/ui 只采用：原生语义、可见 focus、紧凑 32/36px 控件、单层 border 和一致的 disabled 状态。
- 不采用 shadcn/ui 的完整 registry、Radix 聚合包、CVA 变体依赖或默认 `rounded-xl` 大卡尺度；截图优先覆盖其默认样式。
- 从 LuminaPlus 只采用：高信息密度、tabular numerals、按路由拆分 uPlot、离线保留数据和 Ping null-gap 的实现经验。
- 不采用 LuminaPlus 的 Komari schema/API/WebSocket、React Query、Zod、外部 Inter、背景图/视频、透明卡、彩色指标、氛围渐变、主题配置面板或额外资产/流量页面。

参考仓库只用于阅读，不复制其业务协议或成套组件代码。
