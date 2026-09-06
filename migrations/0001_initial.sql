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
