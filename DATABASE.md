# Monitor SQLite 设计

状态：Phase 1 基线  
日期：2026-09-06

## 1. 原则

- SQLite 只保存配置、认证、最后离线回显状态和按分钟聚合的历史；2 秒实时 snapshot 在内存中。
- 所有时间为 Unix UTC 秒；流量日期/周期边界先按 `site_timezone` 解释，再以对应 UTC epoch 秒保存。
- 容量和流量统一为 bytes；速率为 bytes/second；延迟为 milliseconds；uptime/interval 为 seconds。
- SQLite `INTEGER` 是 signed 64-bit。Rust 收到 `u64` 后必须校验 `<= i64::MAX`，禁止无检查 cast。
- 百分比使用 basis points（0–10000，等于 0.00%–100.00%）；load 使用千分值，避免历史表 REAL 浮点异常。
- 价格使用 `price_micros`（一个货币单位的百万分之一），避免二进制浮点；API/UI 再按货币显示。
- 不保存带单位字符串、格式化日期、在线/离线布尔缓存或明文 secret。
- 外键开启，节点删除使用级联；删除 UI 必须确认。

## 2. 连接与 PRAGMA

数据库首次创建和每次连接初始化：

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -2048;       -- 约 2 MiB page cache
PRAGMA wal_autocheckpoint = 1000;
```

`page_size = 4096` 只在空数据库首次 migration 前设置。单进程单 connection 已避免多数 busy 情况；`busy_timeout` 是异常并发/备份时的保险，不引入连接池。

每小时清理历史后可执行 `PRAGMA wal_checkpoint(PASSIVE)`。正常运行不执行 `VACUUM`，不在每分钟 flush 后 checkpoint。手工维护才允许离线 VACUUM。

schema 版本使用 `PRAGMA user_version`。每个 migration 在 `BEGIN IMMEDIATE` 事务中执行；失败必须回滚并阻止 Server 开始监听。

## 3. 首版 schema

以下 SQL 是 Phase 2 migration 的契约。实现时只能为 SQLite 语法/测试修正，不得无规格增加业务表。

```sql
CREATE TABLE settings (
    id                              INTEGER PRIMARY KEY CHECK (id = 1),
    site_name                       TEXT NOT NULL CHECK (length(site_name) BETWEEN 1 AND 64),
    site_timezone                   TEXT NOT NULL DEFAULT 'Asia/Shanghai'
                                      CHECK (length(site_timezone) BETWEEN 1 AND 64),
    theme_default                   TEXT NOT NULL DEFAULT 'system'
                                      CHECK (theme_default IN ('light', 'dark', 'system')),
    history_retention_days          INTEGER NOT NULL DEFAULT 30
                                      CHECK (history_retention_days BETWEEN 1 AND 30),
    agent_report_interval_seconds   INTEGER NOT NULL DEFAULT 2
                                      CHECK (agent_report_interval_seconds BETWEEN 2 AND 60),
    ping_interval_seconds           INTEGER NOT NULL DEFAULT 15
                                      CHECK (ping_interval_seconds BETWEEN 10 AND 300),
    offline_after_seconds           INTEGER NOT NULL DEFAULT 10
                                      CHECK (offline_after_seconds BETWEEN 5 AND 600),
    default_traffic_reset_day       INTEGER NOT NULL DEFAULT 1
                                      CHECK (default_traffic_reset_day BETWEEN 1 AND 31),
    updated_at                      INTEGER NOT NULL CHECK (updated_at >= 0),
    CHECK (offline_after_seconds > agent_report_interval_seconds)
) STRICT;

INSERT INTO settings (
    id, site_name, site_timezone, theme_default, history_retention_days,
    agent_report_interval_seconds, ping_interval_seconds,
    offline_after_seconds, default_traffic_reset_day, updated_at
) VALUES (1, 'Monitor', 'Asia/Shanghai', 'system', 30, 2, 15, 10, 1, unixepoch());

CREATE TABLE admin (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash   TEXT NOT NULL CHECK (length(password_hash) BETWEEN 32 AND 512),
    updated_at      INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;

CREATE TABLE sessions (
    token_hash      BLOB PRIMARY KEY CHECK (length(token_hash) = 32),
    created_at      INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at      INTEGER NOT NULL CHECK (expires_at > created_at)
) STRICT, WITHOUT ROWID;

CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);

CREATE TABLE nodes (
    id                      INTEGER PRIMARY KEY,
    public_id               TEXT NOT NULL UNIQUE
                              CHECK (length(public_id) = 32)
                              CHECK (public_id NOT GLOB '*[^0-9a-f]*'),
    name                    TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    region_code             TEXT NOT NULL
                              CHECK (length(region_code) = 2)
                              CHECK (region_code = upper(region_code)),
    sort_order              INTEGER NOT NULL DEFAULT 0 CHECK (sort_order >= 0),
    traffic_limit_bytes     INTEGER CHECK (traffic_limit_bytes IS NULL OR traffic_limit_bytes > 0),
    traffic_reset_day       INTEGER NOT NULL CHECK (traffic_reset_day BETWEEN 1 AND 31),
    price_micros            INTEGER CHECK (price_micros IS NULL OR price_micros >= 0),
    currency                TEXT
                              CHECK (currency IS NULL OR (length(currency) = 3 AND currency = upper(currency))),
    renewal_cycle           TEXT
                              CHECK (renewal_cycle IS NULL OR renewal_cycle IN
                                ('monthly', 'quarterly', 'semiannual', 'annual', 'biennial', 'custom')),
    expires_at              INTEGER CHECK (expires_at IS NULL OR expires_at >= 0),
    first_seen_at           INTEGER CHECK (first_seen_at IS NULL OR first_seen_at >= 0),
    created_at              INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at              INTEGER NOT NULL CHECK (updated_at >= created_at),
    CHECK ((price_micros IS NULL AND currency IS NULL) OR
           (price_micros IS NOT NULL AND currency IS NOT NULL))
) STRICT;

CREATE INDEX nodes_sort_order_idx ON nodes (sort_order, id);

CREATE TABLE node_tokens (
    node_id         INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    token_hash      BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    created_at      INTEGER NOT NULL CHECK (created_at >= 0)
) STRICT;

CREATE TABLE node_last_state (
    node_id                 INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    hostname                TEXT NOT NULL CHECK (length(hostname) BETWEEN 1 AND 255),
    os_name                 TEXT NOT NULL CHECK (length(os_name) BETWEEN 1 AND 128),
    os_version              TEXT NOT NULL CHECK (length(os_version) <= 128),
    kernel                  TEXT NOT NULL CHECK (length(kernel) BETWEEN 1 AND 255),
    architecture            TEXT NOT NULL CHECK (architecture IN ('x86_64', 'aarch64')),
    cpu_model               TEXT NOT NULL CHECK (length(cpu_model) BETWEEN 1 AND 255),
    cpu_cores               INTEGER NOT NULL CHECK (cpu_cores BETWEEN 1 AND 4096),
    virtualization          TEXT NOT NULL CHECK (length(virtualization) BETWEEN 1 AND 64),
    agent_version           TEXT NOT NULL CHECK (length(agent_version) BETWEEN 1 AND 32),
    boot_id                 TEXT NOT NULL CHECK (length(boot_id) BETWEEN 1 AND 64),
    cpu_usage_bp            INTEGER NOT NULL CHECK (cpu_usage_bp BETWEEN 0 AND 10000),
    load_1_milli            INTEGER NOT NULL CHECK (load_1_milli >= 0),
    load_5_milli            INTEGER NOT NULL CHECK (load_5_milli >= 0),
    load_15_milli           INTEGER NOT NULL CHECK (load_15_milli >= 0),
    memory_total_bytes      INTEGER NOT NULL CHECK (memory_total_bytes >= 0),
    memory_used_bytes       INTEGER NOT NULL CHECK (memory_used_bytes BETWEEN 0 AND memory_total_bytes),
    swap_total_bytes        INTEGER NOT NULL CHECK (swap_total_bytes >= 0),
    swap_used_bytes         INTEGER NOT NULL CHECK (swap_used_bytes BETWEEN 0 AND swap_total_bytes),
    disk_total_bytes        INTEGER NOT NULL CHECK (disk_total_bytes >= 0),
    disk_used_bytes         INTEGER NOT NULL CHECK (disk_used_bytes BETWEEN 0 AND disk_total_bytes),
    rx_rate_bytes_per_sec   INTEGER NOT NULL CHECK (rx_rate_bytes_per_sec >= 0),
    tx_rate_bytes_per_sec   INTEGER NOT NULL CHECK (tx_rate_bytes_per_sec >= 0),
    uptime_seconds          INTEGER NOT NULL CHECK (uptime_seconds >= 0),
    process_count           INTEGER NOT NULL CHECK (process_count >= 0),
    last_ip                 TEXT NOT NULL CHECK (length(last_ip) BETWEEN 2 AND 64),
    last_seen_at            INTEGER NOT NULL CHECK (last_seen_at >= 0),
    persisted_at            INTEGER NOT NULL CHECK (persisted_at >= last_seen_at)
) STRICT;

CREATE TABLE traffic_totals (
    node_id                 INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    rx_total_bytes          INTEGER NOT NULL DEFAULT 0 CHECK (rx_total_bytes >= 0),
    tx_total_bytes          INTEGER NOT NULL DEFAULT 0 CHECK (tx_total_bytes >= 0),
    last_rx_counter_bytes   INTEGER CHECK (last_rx_counter_bytes IS NULL OR last_rx_counter_bytes >= 0),
    last_tx_counter_bytes   INTEGER CHECK (last_tx_counter_bytes IS NULL OR last_tx_counter_bytes >= 0),
    last_boot_id            TEXT CHECK (last_boot_id IS NULL OR length(last_boot_id) BETWEEN 1 AND 64),
    updated_at              INTEGER NOT NULL CHECK (updated_at >= 0),
    CHECK ((last_rx_counter_bytes IS NULL AND last_tx_counter_bytes IS NULL AND last_boot_id IS NULL) OR
           (last_rx_counter_bytes IS NOT NULL AND last_tx_counter_bytes IS NOT NULL AND last_boot_id IS NOT NULL))
) STRICT;

CREATE TABLE traffic_daily (
    node_id             INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    day_start_utc       INTEGER NOT NULL CHECK (day_start_utc >= 0),
    rx_bytes            INTEGER NOT NULL DEFAULT 0 CHECK (rx_bytes >= 0),
    tx_bytes            INTEGER NOT NULL DEFAULT 0 CHECK (tx_bytes >= 0),
    updated_at          INTEGER NOT NULL CHECK (updated_at >= day_start_utc),
    PRIMARY KEY (node_id, day_start_utc)
) STRICT, WITHOUT ROWID;

CREATE TABLE traffic_cycles (
    node_id             INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    cycle_start_utc     INTEGER NOT NULL CHECK (cycle_start_utc >= 0),
    cycle_end_utc       INTEGER NOT NULL CHECK (cycle_end_utc > cycle_start_utc),
    rx_bytes            INTEGER NOT NULL DEFAULT 0 CHECK (rx_bytes >= 0),
    tx_bytes            INTEGER NOT NULL DEFAULT 0 CHECK (tx_bytes >= 0),
    updated_at          INTEGER NOT NULL CHECK (updated_at >= cycle_start_utc),
    PRIMARY KEY (node_id, cycle_start_utc)
) STRICT, WITHOUT ROWID;

CREATE TABLE node_history (
    node_id                 INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    bucket_ts               INTEGER NOT NULL CHECK (bucket_ts >= 0 AND bucket_ts % 60 = 0),
    sample_count            INTEGER NOT NULL CHECK (sample_count > 0),
    cpu_usage_bp            INTEGER NOT NULL CHECK (cpu_usage_bp BETWEEN 0 AND 10000),
    load_1_milli            INTEGER NOT NULL CHECK (load_1_milli >= 0),
    load_5_milli            INTEGER NOT NULL CHECK (load_5_milli >= 0),
    load_15_milli           INTEGER NOT NULL CHECK (load_15_milli >= 0),
    memory_used_bytes       INTEGER NOT NULL CHECK (memory_used_bytes >= 0),
    swap_used_bytes         INTEGER NOT NULL CHECK (swap_used_bytes >= 0),
    disk_used_bytes         INTEGER NOT NULL CHECK (disk_used_bytes >= 0),
    rx_rate_bytes_per_sec   INTEGER NOT NULL CHECK (rx_rate_bytes_per_sec >= 0),
    tx_rate_bytes_per_sec   INTEGER NOT NULL CHECK (tx_rate_bytes_per_sec >= 0),
    PRIMARY KEY (node_id, bucket_ts)
) STRICT, WITHOUT ROWID;

CREATE TABLE ping_targets (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    host            TEXT NOT NULL CHECK (length(host) BETWEEN 1 AND 253),
    ip_family       INTEGER NOT NULL CHECK (ip_family IN (4, 6)),
    enabled         INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    sort_order      INTEGER NOT NULL DEFAULT 0 CHECK (sort_order >= 0),
    created_at      INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at      INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;

CREATE INDEX ping_targets_order_idx ON ping_targets (enabled DESC, sort_order, id);

CREATE TABLE ping_history (
    node_id             INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    bucket_ts           INTEGER NOT NULL CHECK (bucket_ts >= 0 AND bucket_ts % 60 = 0),
    target_id           INTEGER NOT NULL REFERENCES ping_targets(id) ON DELETE CASCADE,
    sample_count        INTEGER NOT NULL CHECK (sample_count > 0),
    success_count       INTEGER NOT NULL CHECK (success_count BETWEEN 0 AND sample_count),
    latency_avg_ms      REAL,
    latency_min_ms      REAL,
    latency_max_ms      REAL,
    PRIMARY KEY (node_id, bucket_ts, target_id),
    CHECK (
      (success_count = 0 AND latency_avg_ms IS NULL AND latency_min_ms IS NULL AND latency_max_ms IS NULL)
      OR
      (success_count > 0 AND latency_avg_ms >= 0 AND latency_min_ms >= 0 AND latency_max_ms >= 0
       AND latency_min_ms <= latency_avg_ms AND latency_avg_ms <= latency_max_ms)
    )
) STRICT, WITHOUT ROWID;
```

## 4. 表职责与保留策略

| 表 | 职责 | 写入频率 | 保留 |
| --- | --- | --- | --- |
| `settings` | 唯一站点/运行设置 | 管理员修改 | 永久 |
| `admin` | 唯一 Argon2id hash | CLI/修改密码 | 永久 |
| `sessions` | 登录 session token hash | 登录/登出 | 过期即删 |
| `nodes` | 节点人工配置 | 管理员修改 | 至节点删除 |
| `node_tokens` | 每节点一个 bearer hash | 创建/轮换 | 至轮换/删除 |
| `node_last_state` | 重启后离线回显 | 最多每分钟 | 覆盖写 |
| `traffic_totals` | 永不按周期清零的总量与 raw baseline | 最多每分钟/重启事件 | 覆盖写 |
| `traffic_daily` | 站点时区自然日用量，边界存 UTC epoch | 最多每分钟 | 保留最近 2 个站点自然日 |
| `traffic_cycles` | billing cycle 用量 | 最多每分钟 | 保留当前及前一周期 |
| `node_history` | 资源分钟桶 | 每分钟批量 | 设置值，最多 30 日 |
| `ping_targets` | 最多六个探测目标 | 管理员修改 | 至删除 |
| `ping_history` | Ping 分钟桶 | 每分钟批量 | 固定 7 日 |

`traffic_daily`/`traffic_cycles` 的旧数据没有对应 UI，不把它们扩展成流量历史报表。清理保留少量前一 bucket 只是为了边界写入和问题诊断，不形成第十项功能。

## 5. 初始化与节点生命周期

### 5.1 新节点

单事务执行：

1. 生成随机 `public_id` 与 32-byte Agent token；
2. 插入 `nodes`；
3. 插入 `node_tokens` 的 SHA-256；
4. 插入零值 `traffic_totals`，三个 baseline 字段为 NULL；
5. commit 后更新 metadata/token cache；
6. HTTP response 只在这一次返回明文 token。

若 response 在 commit 后丢失，Server 也不能恢复明文；管理员使用显式 token 轮换。

### 5.2 首次 report

首次 report 将 raw RX/TX 保存为 baseline，delta 为 0；更新 `nodes.first_seen_at`（只在 NULL 时）、内存 snapshot 和 traffic state。`node_last_state`/`traffic_totals` 在后台 flush 持久化。

### 5.3 token 轮换

生成新 token，在事务中替换 `node_tokens.token_hash`。commit 后以新 hash 原子替换内存 cache，返回明文一次。旧 Agent 下个 report 得到 401。

### 5.4 删除

删除 `nodes` 一行，由外键级联删除 token、最后状态、traffic 和 history。删除前端必须显示节点名和不可恢复提示。Server commit 后清除所有相关 cache/accumulator。

## 6. Traffic 持久化不变量

内存 traffic state 是当前权威值，数据库是可恢复 checkpoint。每个节点有递增 `generation`：report 成功处理后加一；writer 捕获同一锁下的完整绝对值和 generation。

数据库使用绝对值 UPSERT，不使用 `rx_bytes = rx_bytes + ?`：

```sql
INSERT INTO traffic_totals (
  node_id, rx_total_bytes, tx_total_bytes,
  last_rx_counter_bytes, last_tx_counter_bytes, last_boot_id, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(node_id) DO UPDATE SET
  rx_total_bytes = excluded.rx_total_bytes,
  tx_total_bytes = excluded.tx_total_bytes,
  last_rx_counter_bytes = excluded.last_rx_counter_bytes,
  last_tx_counter_bytes = excluded.last_tx_counter_bytes,
  last_boot_id = excluded.last_boot_id,
  updated_at = excluded.updated_at;
```

daily/cycle 同样写内存绝对值。这样“事务已 commit、进程尚未清 dirty 标记就崩溃”的重试仍幂等。commit 成功后记录 `persisted_generation = captured_generation`；若当前 generation 更大，下一 tick 继续写。

写入前必须断言：

- 新 total 不小于该进程从 DB 装载的已提交 total；
- raw 与 boot 三字段要么全 NULL，要么全非 NULL；
- daily/cycle 不为负；
- cycle start/end 由统一纯函数计算。

## 7. 分钟聚合 UPSERT

资源和 Ping 的 `(node, minute[, target])` 是天然幂等键。writer 重试时覆盖同一桶，不插入重复行。

资源聚合规则：

- `sample_count`：本分钟有效 report 数；
- CPU、load、RX/TX rate：有效值算术平均后取整；
- memory/swap/disk：本分钟最后一个有效值；
- bucket_ts：`received_at - received_at % 60`。

Ping 聚合规则：

- 失败也增加 `sample_count`，不增加 `success_count`；
- 全失败则三个 latency 字段均 NULL；
- 部分失败只用成功样本计算 latency，丢包由 counts 表达；
- 删除 target 会删除对应 history；不保留“幽灵 legend”。

## 8. 关键查询

### 8.1 后台节点列表：严格一次查询

`GET /api/admin/nodes` 先按 `settings.site_timezone` 计算当前本地自然日对应的 UTC `day_start`，并取得 UTC `now`，再执行一次 JOIN：

```sql
SELECT
  n.*,
  s.hostname, s.last_ip, s.agent_version, s.last_seen_at,
  t.rx_total_bytes, t.tx_total_bytes,
  COALESCE(d.rx_bytes, 0) AS today_rx_bytes,
  COALESCE(d.tx_bytes, 0) AS today_tx_bytes,
  COALESCE(c.rx_bytes, 0) AS cycle_rx_bytes,
  COALESCE(c.tx_bytes, 0) AS cycle_tx_bytes
FROM nodes AS n
LEFT JOIN node_last_state AS s ON s.node_id = n.id
LEFT JOIN traffic_totals AS t ON t.node_id = n.id
LEFT JOIN traffic_daily AS d
  ON d.node_id = n.id AND d.day_start_utc = ?1
LEFT JOIN traffic_cycles AS c
  ON c.node_id = n.id AND c.cycle_start_utc <= ?2 AND c.cycle_end_utc > ?2
ORDER BY n.sort_order, n.id;
```

禁止先查 nodes 再对每节点查 traffic/status。公开首页不执行这条查询，而读取内存预序列化快照。

### 8.2 资源历史

1h/6h/24h：

```sql
SELECT *
FROM node_history
WHERE node_id = ?1 AND bucket_ts >= ?2 AND bucket_ts < ?3
ORDER BY bucket_ts;
```

7d 使用 `bucket_ts - bucket_ts % 300` 分组。SQL 返回固定字段顺序；handler 在 blocking thread 内完成 row mapping，再释放 connection。

### 8.3 Ping 历史

主键 `(node_id, bucket_ts, target_id)` 同时支持一个节点的时间范围和同一分钟的全部 targets：

```sql
SELECT h.*, t.name, t.ip_family, t.sort_order
FROM ping_history AS h
JOIN ping_targets AS t ON t.id = h.target_id
WHERE h.node_id = ?1 AND h.bucket_ts >= ?2 AND h.bucket_ts < ?3
ORDER BY h.bucket_ts, t.sort_order, t.id;
```

不为每个 target 单独查询。

## 9. 清理

每小时由一个 worker：

1. 读取 settings retention；
2. 列出 node ids；
3. 每个节点按主键前缀分批删除过期 `node_history`；
4. 同法删除 7 天前 `ping_history`；
5. 删除 2 个站点自然日以前的 `traffic_daily`；
6. 每节点保留当前和前一 `traffic_cycles`；
7. 删除过期 `sessions`；
8. commit 后执行 passive checkpoint。

每批建议最多 5,000 行，批间释放事务，避免长时间阻塞登录/后台写入。没有全局 `bucket_ts` cleanup index，是为了不为两个最大的历史表增加一份长期索引；按 `node_id` 删除可使用各自主键。

## 10. 容量预算

默认 7 节点：

- `node_history`：`7 × 30 × 24 × 60 = 302,400` 行；
- `ping_history`（每节点 6 target、7 天、60 秒桶）：`7 × 6 × 7 × 24 × 60 = 423,360` 行；
- 两张主要历史表合计约 725,760 行；
- traffic/config/auth 行数相对可忽略。

使用复合主键 `WITHOUT ROWID` 避免历史表额外 rowid/index。实际大小受文本、page fill、WAL 和 SQLite 版本影响，不能把估算写成 benchmark。Phase 7 在稳态 checkpoint 后记录 `page_count × page_size`、WAL 大小和各表行数，目标为几十 MiB；若超出，先检查 schema/index/保留策略，不降低正确性或偷偷删除必要维度。

## 11. 备份与恢复

- 最简单可靠的方式是停止 Server 后复制 `monitor.db`，备份产物仍只有这一个文件。
- 需要在线备份时，使用系统 `sqlite3 monitor.db ".backup monitor-backup.db"` 生成单文件一致性副本，不为 Monitor 增加备份子命令。
- Server 运行且 WAL 活跃时，禁止只复制主文件；手工备份必须使用 SQLite `.backup`/backup API。
- 恢复时停止 Server、替换数据库、确认文件 owner/mode，然后启动；migration 只向前运行。
- 数据库建议 mode 0600，父目录仅 Server 用户可写。

## 12. 数据库测试清单

- 空库 migration、重复启动、`user_version`、`PRAGMA foreign_key_check`。
- 所有 CHECK 边界：bytes 溢出、NaN/Infinity 在入库前拒绝、reset day 1/31、错误 region/currency。
- 节点删除的所有级联表。
- token 只保存 32-byte hash，查询/API/log 不出现明文。
- traffic 同 boot delta、不同 boot、counter 回退、Server 重启、失败重试、幂等重复 flush。
- `site_timezone` 必须能由内嵌 IANA TZDB 解析；API 层拒绝未知名称后才允许写入 settings。
- 站点自然日切换与 billing cycle：1、15、28、29、30、31，含二月、闰年和 DST 跳变/重复时刻。
- history 同分钟 UPSERT、全失败 Ping latency NULL、部分丢包 counts。
- admin node list 使用单条 SQL；测试 connection trace/query counter，阻止 N+1 回归。
- 30 日/7 日清理边界与分批删除。
- passive checkpoint 后测真实数据库大小。

## 13. Index 审计

首版必须存在且足够的索引如下：

| 查询 | 使用的索引 |
| --- | --- |
| public id 查 node | `nodes.public_id` UNIQUE 自动索引 |
| Agent bearer hash | `node_tokens.token_hash` UNIQUE 自动索引 |
| Session auth | `sessions` PRIMARY KEY |
| Session 清理 | `sessions_expires_at_idx` |
| 首页/后台排序 | `nodes_sort_order_idx` |
| Node/resource range | `node_history(node_id, bucket_ts)` PRIMARY KEY |
| Node/Ping range + targets | `ping_history(node_id, bucket_ts, target_id)` PRIMARY KEY |
| Ping target 配置排序 | `ping_targets_order_idx` |
| 今日流量 join | `traffic_daily(node_id, day_start_utc)` PRIMARY KEY |
| 当前周期 join | `traffic_cycles(node_id, cycle_start_utc)` PRIMARY KEY；每节点只留两周期，end filter 无需额外索引 |
| last state/total join | 两表各自 `node_id` PRIMARY KEY |

不建立 `node_history(bucket_ts)` 或 `ping_history(bucket_ts)` 全局索引；它们会显著增加最大两表体积。清理按 node id 分批，能使用现有复合主键。Phase 7 只有在 `EXPLAIN QUERY PLAN` 或真实清理耗时证明必要时才增加索引。
