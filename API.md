# Monitor API v1

状态：Phase 1 冻结  
日期：2026-09-06

## 1. 冻结范围

首版只提供下表接口。除修复安全/正确性问题外，Phase 2–7 不新增 endpoint；需要更多页面数据时，优先扩充既有响应中已经定义的对象，而不是增加逐节点请求。

| 分组 | Method | Path | 认证 | CSRF |
| --- | --- | --- | --- | --- |
| Public | GET | `/api/public/snapshot` | 无 | 否 |
| Public | GET | `/api/public/nodes/:id/history` | 无 | 否 |
| Public | GET | `/api/public/nodes/:id/ping` | 无 | 否 |
| Agent | POST | `/api/agent/report` | Agent Bearer | 否 |
| Agent | GET | `/api/agent/config` | Agent Bearer | 否 |
| Auth | POST | `/api/auth/login` | 无 | 否；校验同源 Origin |
| Auth | POST | `/api/auth/logout` | Admin session | 是 |
| Auth | GET | `/api/auth/me` | Admin session | 否 |
| Admin | GET | `/api/admin/nodes` | Admin session | 否 |
| Admin | POST | `/api/admin/nodes` | Admin session | 是 |
| Admin | PATCH | `/api/admin/nodes/:id` | Admin session | 是 |
| Admin | DELETE | `/api/admin/nodes/:id` | Admin session | 是 |
| Admin | POST | `/api/admin/nodes/:id/rotate-token` | Admin session | 是 |
| Admin | GET | `/api/admin/ping-targets` | Admin session | 否 |
| Admin | POST | `/api/admin/ping-targets` | Admin session | 是 |
| Admin | PATCH | `/api/admin/ping-targets/:id` | Admin session | 是 |
| Admin | DELETE | `/api/admin/ping-targets/:id` | Admin session | 是 |
| Admin | GET | `/api/admin/settings` | Admin session | 否 |
| Admin | PATCH | `/api/admin/settings` | Admin session | 是 |
| Admin | PATCH | `/api/admin/password` | Admin session | 是 |

`PATCH /api/admin/password` 是原始需求中“修改管理员密码”的唯一必要附加接口。不存在注册、用户、权限、通知、命令、Agent 更新或通用 token 管理 API。

## 2. 通用约定

### 2.1 表示

- JSON 使用 UTF-8；有 body 的请求必须发送 `Content-Type: application/json`。
- 字段名使用 `snake_case`。
- timestamp 是 Unix UTC 秒整数。浏览器负责按用户本地时区显示。
- `site_timezone` 是 IANA 名称；今日/计费周期边界按站点时区解释，再以 UTC timestamp 返回。
- 容量/流量是 bytes，速率是 bytes/second，延迟是 milliseconds，uptime/interval 是 seconds。
- 百分比在 API 中是 0–100 的有限 JSON number，不带 `%`。
- 所有整数必须在 `0..=9_007_199_254_740_991`（JavaScript safe integer）内；Server 内部还要满足 SQLite signed 64-bit。
- API 中的节点 `id` 是 32 位小写十六进制 public id，不暴露 SQLite row id。
- 可空业务值使用 JSON `null`。缺测/timeout 禁止用 0 伪装。
- write DTO 拒绝未知字段，防止拼写错误被静默接受；PATCH body 至少包含一个可修改字段。
- 字符串在校验前去除两端空白，日志不得包含密码、session、CSRF 或 Agent token。

### 2.2 Cache headers

- `/api/public/snapshot`：`Cache-Control: no-cache` + ETag；匹配 `If-None-Match` 可返回 304。
- Public history/ping：`Cache-Control: public, max-age=30` + ETag；匹配时可返回 304。
- Auth/Admin/Agent：`Cache-Control: no-store`。

304 response 无 body，不改变下文定义的 200 JSON 契约。

### 2.3 Admin session 与 CSRF

登录成功设置：

- `__Host-monitor_session`：HttpOnly、Secure、SameSite=Strict、Path=/；
- `__Host-monitor_csrf`：Secure、SameSite=Strict、Path=/，允许前端读取。

所有标记“CSRF：是”的请求必须同时满足：

1. 有效 session cookie；
2. `Origin` 与 Server 的有效 origin 一致；
3. `X-CSRF-Token` header 与 CSRF cookie 常量时间相等。

Agent Bearer 请求不使用 cookie，也不做 CSRF。

## 3. 统一错误格式

所有非 204/304 错误返回：

```json
{
  "error": {
    "code": "invalid_request",
    "message": "range must be one of 1h, 6h, 24h, 7d"
  }
}
```

不返回 SQL、Rust error chain、文件路径、hash 或 stack trace。

| HTTP | code | 使用场景 |
| --- | --- | --- |
| 400 | `invalid_request` | JSON/参数/字段组合不合法 |
| 401 | `unauthorized` | session 或 Agent token 缺失/无效 |
| 401 | `invalid_credentials` | 管理员密码错误 |
| 403 | `csrf_failed` | Origin/header/cookie 校验失败 |
| 404 | `not_found` | 节点或 Ping target 不存在 |
| 409 | `conflict` | 启用 Ping target 超过六个或状态冲突 |
| 413 | `payload_too_large` | body 超限 |
| 415 | `unsupported_media_type` | 非 JSON write 请求 |
| 429 | `rate_limited` | 登录失败次数超限，附 `Retry-After` |
| 500 | `internal_error` | 未分类内部错误；对外信息固定 |
| 503 | `service_unavailable` | SQLite mutation 暂时不可完成 |

同一错误路径只使用一个稳定 code。字段级细节写入 message 即可，不增加通用 validation error 树。

## 4. 共享对象

### 4.1 `PublicNode`

`snapshot.nodes[]` 一次包含首页卡片和详情页顶部所需的全部当前数据：

```json
{
  "id": "8db654de04e54cbcb667a388598b0e6a",
  "name": "DMIT",
  "region_code": "US",
  "sort_order": 0,
  "online": true,
  "first_seen_at": 1788630000,
  "last_seen_at": 1788664400,
  "system": {
    "hostname": "dmit-01",
    "os_name": "Debian",
    "os_version": "13",
    "kernel": "6.12.107+deb13-amd64",
    "architecture": "x86_64",
    "virtualization": "qemu",
    "agent_version": "0.1.0",
    "cpu_model": "AMD EPYC 9654",
    "cpu_cores": 1,
    "process_count": 109,
    "uptime_seconds": 363600
  },
  "metrics": {
    "cpu_usage": 0.8,
    "load_1": 0.01,
    "load_5": 0.0,
    "load_15": 0.0,
    "memory_total": 1011871744,
    "memory_used": 286492672,
    "swap_total": 0,
    "swap_used": 0,
    "disk_total": 21045339750,
    "disk_used": 2040109465,
    "current_rx_rate": 86,
    "current_tx_rate": 496
  },
  "traffic": {
    "today_rx": 145752064,
    "today_tx": 184549376,
    "cycle_rx": 4299161600,
    "cycle_tx": 4257218560,
    "total_rx": 956703965184,
    "total_tx": 944892805120,
    "limit": 1073741824000,
    "cycle_start_at": 1788192000,
    "cycle_end_at": 1790784000
  },
  "billing": {
    "price_micros": 39900000,
    "currency": "USD",
    "renewal_cycle": "annual",
    "expires_at": 1793836800
  }
}
```

规则：

- 节点从未上报时，`first_seen_at`、`last_seen_at`、`system`、`metrics` 为 null，`online=false`；traffic 总量为 0。
- 离线节点保留最后 `system`/`metrics`，只有 `online` 改为 false。
- `traffic.limit=null` 表示无限；此时仍返回 cycle RX/TX。
- 未配置价格时整个 `billing=null`；价格 0 表示免费；无到期日时 `expires_at=null`。
- flow quota 使用 `cycle_rx + cycle_tx`，前端使用 checked/safe integer 加法。

### 4.2 `AdminNode`

Admin 列表在 `PublicNode` 的配置/状态基础上增加 `last_ip`，但不返回 token hash、boot id、raw network counter 或 session 数据。

```json
{
  "id": "8db654de04e54cbcb667a388598b0e6a",
  "name": "DMIT",
  "region_code": "US",
  "sort_order": 0,
  "last_ip": "203.0.113.10",
  "online": true,
  "last_seen_at": 1788664400,
  "cycle_rx": 4299161600,
  "cycle_tx": 4257218560,
  "traffic_limit": 1073741824000,
  "price_micros": 39900000,
  "currency": "USD",
  "renewal_cycle": "annual",
  "expires_at": 1793836800
}
```

## 5. Public API

### 5.1 `GET /api/public/snapshot`

Authentication：无。  
CSRF：否。  
Request body：无。  
Query：无。

#### 200 response

```json
{
  "generated_at": 1788664402,
  "site": {
    "name": "Monitor",
    "timezone": "Asia/Shanghai",
    "theme_default": "system"
  },
  "summary": {
    "online_nodes": 8,
    "total_nodes": 8,
    "busiest_node": {
      "id": "a16bbef740274614b5bed1db91bcd6d8",
      "name": "CCS",
      "cpu_usage": 1.7
    },
    "today_rx": 27917287424,
    "today_tx": 28776280883,
    "total_rx": 4738883923476,
    "total_tx": 4563411546112,
    "current_rx_rate": 17818,
    "current_tx_rate": 5734,
    "network_rate_history": {
      "timestamps": [1788664282, 1788664284, 1788664286],
      "rx": [16400, 17120, 17818],
      "tx": [5200, 5510, 5734]
    }
  },
  "nodes": []
}
```

`nodes` 是按 `sort_order, internal_id` 排序的 `PublicNode[]`。

#### 语义与校验

- `online_nodes` 只统计 `generated_at - last_seen_at <= offline_after_seconds` 的节点。
- `busiest_node` 只在在线节点中按 CPU 取最大；没有在线且有当前数据的节点时为 null；并列按 sort order。
- today 使用 `site.timezone` 当前自然日；total 是所有节点 all-time RX/TX；current rate 只合计在线节点。
- `network_rate_history` 是 Server 启动后内存中的全站速率 ring buffer，最长约 120 秒；三个数组等长、时间递增。Server 刚启动时可以少于 60 点。
- snapshot 从预序列化内存 cache 返回。生成 cache 时复制一次所有 NodeSnapshot/traffic/meta；不为 HTTP 请求查询数据库，更不允许 per-node query。

#### Status

- 200：返回完整 snapshot。
- 304：ETag 未变化，无 body。
- 500：仅 cache 尚未成功构建的启动异常；正常运行保留最后成功 cache。

### 5.2 `GET /api/public/nodes/:id/history`

Authentication：无。  
CSRF：否。  
Request body：无。  
Query：必填 `range=1h|6h|24h|7d`，不得有其他参数。

#### 200 response

响应为已经对齐的 dense parallel arrays，可直接组成 uPlot `AlignedData`：

```json
{
  "node_id": "8db654de04e54cbcb667a388598b0e6a",
  "from": 1788642800,
  "to": 1788664400,
  "step": 60,
  "series": {
    "timestamp": [1788642840, 1788642900, 1788642960],
    "cpu": [0.8, null, 1.1],
    "memory": [286492672, null, 287313920],
    "disk": [2040109465, null, 2040117760],
    "rx_rate": [231, null, 880],
    "tx_rate": [638, null, 1250]
  }
}
```

规则：

- 1h/6h/24h 的 `step=60`；7d 的 `step=300`。
- `from`/`to` 是请求窗口边界；timestamp 是按 step 对齐的 UTC 秒并严格递增。
- 六个数组长度完全一致。没有该 bucket 时五项都为 null，禁止填 0。
- CPU 是 percent；memory/disk 是 used bytes；rx/tx rate 是 bytes/second。
- 7d 在单次 SQL 中聚合为 5 分钟桶；不返回数据库 basis points/milli 字段名或 row object 数组。

#### Status

- 200：成功；没有数据时仍返回时间轴与全 null series。
- 304：ETag 未变化。
- 400 `invalid_request`：缺失/非法/重复 range 或未知 query。
- 404 `not_found`：节点不存在。

### 5.3 `GET /api/public/nodes/:id/ping`

Authentication：无。  
CSRF：否。  
Request body：无。  
Query：必填 `range=1h|6h|24h|7d`。

#### 200 response

```json
{
  "node_id": "8db654de04e54cbcb667a388598b0e6a",
  "from": 1788642800,
  "to": 1788664400,
  "step": 60,
  "targets": [
    { "id": 1, "name": "联通 v6", "ip_family": 6, "sort_order": 0 },
    { "id": 2, "name": "移动 v4", "ip_family": 4, "sort_order": 1 }
  ],
  "timestamps": [1788642840, 1788642900, 1788642960],
  "series": [
    { "target_id": 1, "latency": [142.1, null, 145.7] },
    { "target_id": 2, "latency": [168.4, 170.2, null] }
  ]
}
```

规则：

- step 与 resource history 相同。
- `targets` 和 `series` 顺序一致，按 target `sort_order, id`。
- 每条 `latency` 与 `timestamps` 等长；全失败、timeout、未上报、已断开区间均为 null，不得为 0。
- 分钟桶有部分成功时 latency 为成功样本平均值；7d 的 5 分钟桶按 success count 加权。
- “削峰”不作为 API 参数。浏览器只裁剪传给 uPlot 的副本，response 和 tooltip 原始值不变。
- 一次 SQL JOIN 返回指定节点全部 targets 的范围数据，不逐 target 查询。

#### Status

- 200、304、400、404：语义同 resource history。

## 6. Agent API

### 6.1 Bearer authentication

Header：`Authorization: Bearer <64-char-lowercase-hex-token>`。token 包含 32 random bytes，数据库只保存 SHA-256。Server 对格式明显错误和 hash 未命中统一返回 401，不泄漏节点是否存在。

### 6.2 `GET /api/agent/config`

Authentication：Agent Bearer。  
CSRF：否。  
Request body/query：无。  
Request header：`X-Monitor-Config-Version: 2`（可选）。只有值恰好为 `2` 时返回 config protocol 2；缺失、其它值或 `02` 都按 protocol 1 处理。

#### 200 response（protocol 1，默认）

```json
{
  "protocol_version": 1,
  "report_interval_seconds": 2,
  "ping_interval_seconds": 15,
  "targets": [
    { "id": 1, "name": "电信 v4", "host": "203.0.113.1", "ip_family": 4 },
    { "id": 2, "name": "电信 v6", "host": "2001:db8::1", "ip_family": 6 }
  ]
}
```

protocol 1 只返回 `probe_kind = icmp` 的 target，且 target 对象不含 `probe_kind`、`port` 或任何占位字段——老 Agent 用 `deny_unknown_fields` 解析，多一个字段就会失败。

#### 200 response（protocol 2）

```json
{
  "protocol_version": 2,
  "report_interval_seconds": 2,
  "ping_interval_seconds": 15,
  "targets": [
    { "id": 1, "name": "电信 v4", "host": "203.0.113.1", "ip_family": 4, "probe_kind": "icmp", "port": null },
    { "id": 3, "name": "HTTPS", "host": "203.0.113.9", "ip_family": 4, "probe_kind": "tcp", "port": 443 }
  ]
}
```

两种版本都只返回 enabled targets，最多 6 个，按 sort order。配置来自内存 `AgentConfigCache`，不为每个 Agent 请求 SQLite。report 线协议与 config 协议无关，始终为 1。

Status：200；401 `unauthorized`。

### 6.3 `POST /api/agent/report`

Authentication：Agent Bearer。  
CSRF：否。  
Body limit：32 KiB。  
Response body：无。

#### Request

```json
{
  "protocol_version": 1,
  "agent_version": "0.1.0",
  "boot_id": "37e3a670-fdde-4173-8728-a7d6b727b590",
  "hostname": "dmit-01",
  "os": {
    "name": "Debian",
    "version": "13",
    "kernel": "6.12.107+deb13-amd64",
    "architecture": "x86_64",
    "virtualization": "qemu"
  },
  "cpu": {
    "model": "AMD EPYC 9654",
    "cores": 1,
    "usage": 0.8,
    "load_1": 0.01,
    "load_5": 0.0,
    "load_15": 0.0
  },
  "memory": {
    "total": 1011871744,
    "used": 286492672,
    "swap_total": 0,
    "swap_used": 0
  },
  "disk": {
    "total": 21045339750,
    "used": 2040109465
  },
  "network": {
    "rx_bytes": 4589392184,
    "tx_bytes": 3285011440,
    "rx_rate": 86,
    "tx_rate": 496
  },
  "uptime_seconds": 363600,
  "process_count": 109,
  "pings": [
    { "target_id": 1, "success": true, "latency_ms": 142.1 },
    { "target_id": 2, "success": false, "latency_ms": null }
  ]
}
```

#### Validation

- `protocol_version` 必须为 1；未知字段拒绝。
- hostname 1–255；OS/kernel/model 采用 DATABASE.md 上限；architecture 只接受 `x86_64`、`aarch64`。
- CPU usage 0–100；load/latency 必须 finite 且非负；cores 1–4096。
- used 不得大于 total；bytes/seconds/process count 必须是 safe non-negative integer。
- `boot_id` 1–64；Agent Linux 实现发送 `/proc/sys/kernel/random/boot_id`。
- `pings` 0–6 项、target id 不重复，必须属于当前 enabled config。
- `success=true` 当且仅当 `latency_ms` 是非负数；`success=false` 当且仅当 latency 为 null。
- Server 使用 receipt time 作为 `last_seen_at`/history bucket，不接受 Agent 自报时间。

#### Status

- 204：已更新内存 snapshot/traffic/history accumulator。
- 400 `invalid_request`：协议/字段/target 不合法。
- 401 `unauthorized`：Bearer 无效。
- 413 `payload_too_large`。
- 415 `unsupported_media_type`。

报告成功不代表 SQLite 已同步；分钟 writer 异步 checkpoint。204 前不会执行 SQL。

## 7. Auth API

### 7.1 `POST /api/auth/login`

Authentication：无。  
CSRF：否；要求 JSON 和同源 Origin。  
Body limit：4 KiB。

Request：

```json
{ "password": "correct horse battery staple" }
```

200 response：

```json
{
  "authenticated": true,
  "expires_at": 1789269200
}
```

Validation：password 1–1024 bytes；不 trim 密码；只允许 password 一个字段。成功设置两个 cookie。失败统一 401，不区分“尚未设置密码”和“密码错误”；Server 日志可提示管理员运行 CLI，但不能记录 password。

Status：200；400；401 `invalid_credentials`；415；429 `rate_limited`。

### 7.2 `POST /api/auth/logout`

Authentication：Admin session。  
CSRF：是。  
Request body：无。  
Response：204，并删除 SQLite session、清空两个 cookie。

Status：204；401；403。

### 7.3 `GET /api/auth/me`

Authentication：Admin session。  
CSRF：否。  
Request body/query：无。

200 response：

```json
{
  "authenticated": true,
  "expires_at": 1789269200
}
```

Status：200；401 `unauthorized`。不返回管理员 id、用户名或 password 元数据，因为系统没有这些概念。

## 8. Admin Nodes API

### 8.1 `GET /api/admin/nodes`

Authentication：Admin session。  
CSRF：否。  
Request body/query：无。

200 response：

```json
{ "nodes": [] }
```

`nodes` 为 `AdminNode[]`，按 sort order。Server 使用 DATABASE.md 定义的一条 JOIN SQL，然后与一次性复制的内存 online snapshot 合并；禁止 per-node traffic/status query。

Status：200；401；503。

### 8.2 `POST /api/admin/nodes`

Authentication：Admin session。  
CSRF：是。  
Body limit：8 KiB。

Request：

```json
{
  "name": "DMIT",
  "region_code": "US",
  "traffic_limit": 1073741824000,
  "traffic_reset_day": 1,
  "price_micros": 39900000,
  "currency": "USD",
  "renewal_cycle": "annual",
  "expires_at": 1793836800
}
```

`traffic_limit`、`price_micros`、`currency`、`renewal_cycle`、`expires_at` 可为 null。省略 `traffic_reset_day` 时使用 settings 默认值。sort order 自动追加到末尾。

201 response：

```json
{
  "node": {
    "id": "8db654de04e54cbcb667a388598b0e6a",
    "name": "DMIT",
    "region_code": "US",
    "sort_order": 7
  },
  "agent_token": "8f3175c6c1a44f5da88bc96883524ce359cd9222cb61b653ad61da78e6f20c44"
}
```

前端使用当前站点 origin 和一次性 token 生成安装命令；API 不保存或再次返回命令/token。

Validation：name 1–64；region 为两个大写 ASCII 字母；limit 为 null 或正整数；reset day 1–31；price 为 null 或非负整数；price 与 currency 必须同时为 null/非 null；currency 为三位大写 ASCII；renewal cycle 枚举；expires 为 null 或 UTC 秒。

节点名称允许重复，以 public id 区分；API 不为名称增加唯一性规则。Status：201；400；401；403；413；415；503。

### 8.3 `PATCH /api/admin/nodes/:id`

Authentication：Admin session。  
CSRF：是。

Request 可包含创建字段以及 `sort_order`；省略表示不变，显式 null 只用于可空字段：

```json
{
  "traffic_limit": null,
  "expires_at": 1793836800,
  "sort_order": 2
}
```

修改 sort order 时 Server 在单事务内移动受影响节点，最终 order 连续。修改 reset day 从下一个 report 开始使用新周期归属，不改写 total。

200 response：更新后的精简 node 配置对象（与 create 的 `node` 相同并包含全部可编辑字段）。

Status：200；400；401；403；404；413；415；503。

### 8.4 `DELETE /api/admin/nodes/:id`

Authentication：Admin session。  
CSRF：是。  
Request body：无。  
Response：204。

前端确认 Dialog 必须显示节点名和“历史/流量不可恢复”。API 删除 nodes 并级联相关数据，随后清 cache。

Status：204；401；403；404；503。

### 8.5 `POST /api/admin/nodes/:id/rotate-token`

Authentication：Admin session。  
CSRF：是。  
Request body：无。

200 response：

```json
{ "agent_token": "39d8d2ac996a302014fde6c88fe76271972d550c167c425153dc7acefacf83df" }
```

新 hash commit 后旧 token 立即失效；response/日志/后续 GET 不得返回旧或新 token。前端必须在请求前确认现有 Agent 将停止上报。

Status：200；401；403；404；503。

## 9. Admin Ping Target API

### 9.1 对象

```json
{
  "id": 1,
  "name": "电信 v4",
  "host": "203.0.113.1",
  "port": null,
  "ip_family": 4,
  "probe_kind": "icmp",
  "enabled": true,
  "sort_order": 0
}
```

`probe_kind` 为 `icmp` 或 `tcp`。`icmp` 的 `port` 必须为 null；`tcp` 的 `port` 必须为 1–65535。`host` 只能是 IP literal 或合法 DNS hostname，不允许 scheme、path、空白或 shell 字符语义；Agent 使用 socket API，不拼接命令行，也不调用 `ping`/`nc`/`curl` 等外部程序。

写入用单个 `target` 字符串，由 Server 唯一解析：`icmp` 只接受裸 host（含裸 IPv6 literal，不接受方括号）；`tcp` 接受 `host:port`，IPv6 literal 必须写作 `[2001:db8::1]:443`。响应把它拆成 `host` 与 `port` 返回。

### 9.2 `GET /api/admin/ping-targets`

Authentication：Admin session。CSRF：否。Request：无。  
200 response：`{"targets": [...]}`，含 enabled/disabled，按 sort order。  
Status：200；401；503。

### 9.3 `POST /api/admin/ping-targets`

Authentication：Admin session。CSRF：是。

Request：

```json
{
  "name": "HTTPS",
  "target": "203.0.113.9:443",
  "probe_kind": "tcp",
  "ip_family": 4,
  "enabled": true
}
```

sort order 自动追加。201 response：`{"target": {...}}`。

Validation：name 1–64；`probe_kind` 为 `icmp`/`tcp`；`target` 不接受首尾空白，必须能按该 probe kind 与 family 解析；family 4/6；enabled boolean。启用后全站 enabled target 总数不得超过 6。同一 host 上不同 TCP port、或同一 host 同时配置 ICMP 与 TCP，都是不同 target。

Status：201；400；401；403；409；413；415；503。

### 9.4 `PATCH /api/admin/ping-targets/:id`

Authentication：Admin session。CSRF：是。  
Request：`name`、`target`、`probe_kind`、`ip_family`、`enabled`、`sort_order` 中任意一个或多个；null 不合法。  
校验按“改完之后的最终状态”进行：只改 `probe_kind` 而留下不匹配的 host/port 会返回 400，例如 ICMP 目标直接切成 TCP 却没有给出端口。  
200 response：`{"target": {...}}`。

修改/禁用后同步更新 AgentConfigCache；既有历史仍按同一 target id 展示。Status：200；400；401；403；404；409；413；415；503。

### 9.5 `DELETE /api/admin/ping-targets/:id`

Authentication：Admin session。CSRF：是。Request body：无。Response：204。  
删除会级联该 target 的 Ping 历史，前端必须确认。Status：204；401；403；404；503。

## 10. Admin Settings API

### 10.1 `GET /api/admin/settings`

Authentication：Admin session。CSRF：否。Request：无。

200 response：

```json
{
  "site_name": "Monitor",
  "site_timezone": "Asia/Shanghai",
  "theme_default": "system",
  "history_retention_days": 30,
  "agent_report_interval_seconds": 2,
  "ping_interval_seconds": 15,
  "offline_after_seconds": 10,
  "default_traffic_reset_day": 1
}
```

Status：200；401；503。

### 10.2 `PATCH /api/admin/settings`

Authentication：Admin session。CSRF：是。

Request 是上述字段的任意非空子集。示例：

```json
{
  "site_timezone": "Asia/Shanghai",
  "offline_after_seconds": 12
}
```

Validation：

- site name 1–64；
- timezone 1–64 且必须存在于 Server 内嵌 IANA TZDB；
- theme 为 `light|dark|system`；
- history 1–30；report 2–60；ping 10–300；offline 5–600 且大于 report interval；reset day 1–31。

时区或 reset day 修改不重写历史 total；新边界从下一份 report 开始生效。响应为更新后的完整 settings 对象，并同步 settings/config/public cache。

Status：200；400；401；403；413；415；503。

### 10.3 `PATCH /api/admin/password`

Authentication：Admin session。CSRF：是。

Request：

```json
{
  "current_password": "old password",
  "new_password": "new password"
}
```

Validation：两个密码 1–1024 bytes，不 trim；必须不同。Server 验证 current 后生成 Argon2id hash，在同一事务中更新 admin 并删除全部 sessions。

Response：204，并清空当前 cookie；前端跳回登录页。

Status：204；400；401（current 错误统一 `invalid_credentials`）；403；413；415；503。

## 11. 数据访问与 N+1 约束

| Endpoint | 数据来源 | 最大 SQL 次数 |
| --- | --- | --- |
| public snapshot | `PublicSnapshotCache` bytes | 0 |
| resource history | 单个 node existence/range query，可并入一条 | 1 |
| Ping history | ping_history JOIN targets | 1 |
| Agent report | token/config/snapshot/traffic 内存 | 0 |
| Agent config | `AgentConfigCache` | 0 |
| Admin nodes | nodes + last_state + traffic 的单次 JOIN，再合并内存 | 1 |
| Admin ping targets/settings | 单表查询 | 1 |

任何 `for node { db.query(...) }` 或 `for target { db.query(...) }` 都是违反冻结契约的回归。测试使用 SQLite trace/query counter 固定上述上限。

## 12. 明确不存在的 API

首版没有：SSE、WebSocket、GraphQL、RPC、批量节点、流量历史、告警、通知、用户、角色、注册、远程命令、Agent 更新、文件、日志、进程、服务、Docker、插件、主题上传、自定义 CSS/JS、token 列表或 refresh token endpoint。
