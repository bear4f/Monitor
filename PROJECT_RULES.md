# Monitor 项目规则

状态：冻结基线  
日期：2026-09-06

## 1. 产品边界

Monitor 是从零设计的自用型 Linux VPS 监控系统，不 fork Komari，不兼容 Komari API 或 Agent。设计优先级固定为：安全、极简、高效。

首版功能只包含：节点基本信息、CPU/内存/磁盘/网络状态、历史资源曲线、三网 IPv4/IPv6 延迟、总流量与月流量、价格和到期时间、单管理员后台、浅色/深色/跟随系统主题。没有明确列入冻结规格的功能不实现。

## 2. 架构纪律

- Server 使用 Rust stable、Tokio、Axum、Serde、rusqlite/SQLite；不使用 ORM、Redis、消息队列、gRPC 或复杂 RPC。
- Frontend 使用 React、TypeScript、Vite、Tailwind CSS、uPlot、lucide-react；生产环境不依赖 Node.js 或外部 CDN。
- 最终部署物为一个 Server 二进制和一个 SQLite 数据库；Agent 只支持 Linux。
- Agent 只能读取本机状态、执行 ICMP 探测并上报；不存在远程命令、文件、更新或 shell 协议。
- 当前数据从内存 snapshot 提供；2 秒样本不得逐条永久写入 SQLite；历史资源最多每 60 秒一条。
- 数据库和 cache 同时参与的 mutation 必须先提交 SQLite transaction，再更新 cache。
- 不为未来需求预建层次、crate、接口、配置或扩展点；优先删除不必要的依赖和抽象。

## 3. 数据与安全

- 数据库时间统一为 UTC epoch；“今日”和计费周期边界按 `site_timezone` 解释，默认 `Asia/Shanghai`；浏览器用本地时区显示。
- 数据库存 bytes、milliseconds、seconds 和整数比例，不存格式化单位字符串。
- Agent token 至少 256 bit，数据库只存安全 hash；管理员密码使用 Argon2id；浏览器认证使用 SQLite session cookie，不使用 JWT。
- 禁止提交真实密码、session、Agent token、API key、私钥、生产数据库或其他 secret。

## 4. 视觉规则

视觉优先级固定为：`docs/reference/` 截图、冻结视觉描述、LuminaPlus、shadcn 默认样式。界面保持黑白灰、高信息密度和克制，不使用渐变、Glow、玻璃拟态、营销布局、夸张阴影或装饰动画。

## 5. 阶段规则

严格按冻结 Phase 顺序实现。每个 Phase 只包含该阶段授权范围，并在格式化、lint、测试和自审全部通过后创建一个阶段 commit 并推送。不得使用 WIP commit、force push、rebase 已推送历史，或把后续阶段功能混入当前提交。
