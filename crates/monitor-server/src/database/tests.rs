use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rusqlite::{Connection, params};

use super::*;

static TEST_DATABASE_ID: AtomicU64 = AtomicU64::new(0);

struct TestDatabasePath(PathBuf);

impl TestDatabasePath {
    fn new(label: &str) -> Self {
        let id = TEST_DATABASE_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!("monitor-{label}-{}-{id}.db", std::process::id());
        Self(std::env::temp_dir().join(name))
    }

    fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDatabasePath {
    fn drop(&mut self) {
        for suffix in ["", "-shm", "-wal"] {
            let path = PathBuf::from(format!("{}{}", self.0.display(), suffix));
            if let Err(error) = fs::remove_file(path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                panic!("remove test database: {error}");
            }
        }
    }
}

const EXPECTED_TABLES: [&str; 12] = [
    "admin",
    "node_history",
    "node_last_state",
    "node_tokens",
    "nodes",
    "ping_history",
    "ping_targets",
    "sessions",
    "settings",
    "traffic_cycles",
    "traffic_daily",
    "traffic_totals",
];

#[tokio::test]
async fn creates_exact_schema_with_pragmas_defaults_and_indexes() {
    let path = TestDatabasePath::new("schema");
    let database = Database::open(path.as_path()).expect("open fresh database");

    let pragmas = database.read_pragmas().await.expect("read pragmas");
    assert_eq!(pragmas.foreign_keys, 1);
    assert_eq!(pragmas.journal_mode, "wal");
    assert_eq!(pragmas.busy_timeout_ms, 5_000);
    assert_eq!(pragmas.synchronous, 1);
    assert_eq!(pragmas.temp_store, 2);
    assert_eq!(pragmas.cache_size, -2_048);
    assert_eq!(pragmas.wal_autocheckpoint, 1_000);
    assert_eq!(pragmas.page_size, 4_096);
    assert_eq!(
        database.schema_version().await.expect("schema version"),
        CURRENT_SCHEMA_VERSION
    );

    let settings = database.load_settings().await.expect("default settings");
    assert_eq!(settings.site_name, "Monitor");
    assert_eq!(settings.site_timezone, "Asia/Shanghai");
    assert_eq!(settings.theme_default, "system");
    assert_eq!(settings.history_retention_days, 30);
    assert_eq!(settings.agent_report_interval_seconds, 2);
    assert_eq!(settings.ping_interval_seconds, 15);
    assert_eq!(settings.offline_after_seconds, 10);
    assert_eq!(settings.default_traffic_reset_day, 1);

    database.shutdown().await.expect("shutdown database");

    let connection = Connection::open(path.as_path()).expect("inspect database");
    assert_eq!(table_names(&connection), EXPECTED_TABLES);
    assert_primary_keys(&connection);
    assert_required_indexes(&connection);
}

#[tokio::test]
async fn applying_migrations_twice_is_idempotent() {
    let path = TestDatabasePath::new("idempotent");

    Database::open(path.as_path())
        .expect("first migration")
        .shutdown()
        .await
        .expect("first shutdown");
    let database = Database::open(path.as_path()).expect("second migration");
    assert_eq!(
        database.schema_version().await.expect("schema version"),
        CURRENT_SCHEMA_VERSION
    );
    database.shutdown().await.expect("second shutdown");

    let connection = Connection::open(path.as_path()).expect("inspect database");
    assert_eq!(table_names(&connection), EXPECTED_TABLES);
}

#[tokio::test]
async fn startup_restores_missing_default_settings_without_rerunning_migration() {
    let path = TestDatabasePath::new("settings-default");
    Database::open(path.as_path())
        .expect("initial migration")
        .shutdown()
        .await
        .expect("initial shutdown");

    let connection = Connection::open(path.as_path()).expect("open database fixture");
    connection
        .execute("DELETE FROM settings WHERE id = 1", [])
        .expect("delete settings fixture");
    drop(connection);

    let database = Database::open(path.as_path()).expect("reopen database");
    let settings = database.load_settings().await.expect("restored settings");
    assert_eq!(settings.site_name, "Monitor");
    assert_eq!(settings.site_timezone, "Asia/Shanghai");
    assert_eq!(
        database.schema_version().await.expect("schema version"),
        CURRENT_SCHEMA_VERSION
    );
    database.shutdown().await.expect("shutdown database");
}

#[test]
fn failed_migration_rolls_back_schema_and_version() {
    let path = TestDatabasePath::new("rollback");
    let mut connection = Connection::open(path.as_path()).expect("open database");
    configure_connection(&connection).expect("configure database");

    let result = migrations::apply_single_migration(
        &mut connection,
        1,
        "CREATE TABLE rollback_probe (id INTEGER PRIMARY KEY) STRICT;
         INSERT INTO table_that_does_not_exist VALUES (1);",
    );
    assert!(result.is_err());

    let table_exists: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'rollback_probe')",
            [],
            |row| row.get(0),
        )
        .expect("inspect rollback");
    assert_eq!(table_exists, 0);
    assert_eq!(migrations::schema_version(&connection).unwrap(), 0);
}

#[tokio::test]
async fn settings_upsert_and_startup_hydration_use_persisted_state() {
    let path = TestDatabasePath::new("hydration");
    let database = Database::open(path.as_path()).expect("open database");
    let mut settings = database.load_settings().await.expect("load settings");
    settings.site_name = "Private Monitor".into();
    settings.site_timezone = "Europe/Berlin".into();
    settings.updated_at += 1;
    database
        .upsert_settings(settings.clone())
        .await
        .expect("upsert settings");
    database.shutdown().await.expect("shutdown database");

    let connection = Connection::open(path.as_path()).expect("open for fixture insert");
    connection
        .execute(
            "INSERT INTO nodes (
                public_id, name, region_code, sort_order, traffic_limit_bytes,
                traffic_reset_day, price_micros, currency, renewal_cycle,
                expires_at, first_seen_at, created_at, updated_at
             ) VALUES (?1, 'Test node', 'US', 0, NULL, 1, NULL, NULL, NULL, NULL, NULL, 1, 1)",
            ["0123456789abcdef0123456789abcdef"],
        )
        .expect("insert node fixture");
    let node_id = connection.last_insert_rowid();
    connection
        .execute(
            "INSERT INTO traffic_totals (
                node_id, rx_total_bytes, tx_total_bytes, last_rx_counter_bytes,
                last_tx_counter_bytes, last_boot_id, updated_at
             ) VALUES (?1, 100, 200, 10, 20, 'fake-boot-id', 1)",
            params![node_id],
        )
        .expect("insert traffic fixture");
    drop(connection);

    let database = Database::open(path.as_path()).expect("reopen database");
    let hydration = hydrate_startup(&database).await.expect("hydrate startup");
    assert_eq!(hydration.settings, settings);
    assert_eq!(hydration.nodes.len(), 1);
    assert_eq!(hydration.nodes[0].name, "Test node");
    assert_eq!(hydration.traffic_recovery.len(), 1);
    assert_eq!(hydration.traffic_recovery[0].rx_total_bytes, 100);
    assert_eq!(
        hydration.traffic_recovery[0].last_boot_id.as_deref(),
        Some("fake-boot-id")
    );
    database.shutdown().await.expect("shutdown database");
}

#[tokio::test]
async fn session_persistence_supports_lookup_and_targeted_cleanup() {
    let path = TestDatabasePath::new("sessions");
    let database = Database::open(path.as_path()).expect("open database");
    let admin_hash = "x".repeat(32);
    database
        .set_admin_password(admin_hash.clone(), 1)
        .await
        .expect("set administrator fixture");

    let expired_hash = [1_u8; 32];
    let active_hash = [2_u8; 32];
    assert!(
        database
            .create_session(admin_hash.clone(), expired_hash, 1, 2)
            .await
            .expect("create expired session")
    );
    assert!(
        database
            .create_session(admin_hash.clone(), active_hash, 1, 20)
            .await
            .expect("create active session")
    );
    assert_eq!(
        database
            .find_session(active_hash)
            .await
            .expect("find active session"),
        Some(SessionRow {
            created_at: 1,
            expires_at: 20,
        })
    );
    assert_eq!(
        database
            .delete_expired_sessions(10)
            .await
            .expect("delete expired sessions"),
        1
    );
    assert_eq!(
        database
            .find_session(expired_hash)
            .await
            .expect("look up expired session"),
        None
    );
    assert!(
        database
            .delete_session(active_hash)
            .await
            .expect("delete active session")
    );

    assert!(
        database
            .create_session(admin_hash, [3_u8; 32], 1, 20)
            .await
            .expect("create final session")
    );
    assert_eq!(
        database
            .delete_all_sessions()
            .await
            .expect("delete all sessions"),
        1
    );
    database.shutdown().await.expect("shutdown database");
}

#[tokio::test]
async fn session_creation_rejects_a_stale_admin_password_hash() {
    let path = TestDatabasePath::new("stale-session-password");
    let database = Database::open(path.as_path()).expect("open database");
    let old_password_hash = "o".repeat(32);
    let new_password_hash = "n".repeat(32);
    database
        .set_admin_password(old_password_hash.clone(), 1)
        .await
        .expect("set administrator fixture");
    assert!(
        database
            .change_admin_password(old_password_hash.clone(), new_password_hash.clone(), 2)
            .await
            .expect("change administrator password")
    );

    let stale_session_hash = [8_u8; 32];
    assert!(
        !database
            .create_session(old_password_hash, stale_session_hash, 3, 20)
            .await
            .expect("reject stale session")
    );
    assert_eq!(
        database
            .find_session(stale_session_hash)
            .await
            .expect("look up rejected session"),
        None
    );
    assert!(
        database
            .create_session(new_password_hash, [9_u8; 32], 3, 20)
            .await
            .expect("create current session")
    );

    database.shutdown().await.expect("shutdown database");
}

#[test]
fn password_change_rolls_back_if_session_invalidation_fails() {
    let path = TestDatabasePath::new("password-rollback");
    let mut connection = open_ready_connection(path.as_path()).expect("open database");
    let old_password_hash = "o".repeat(32);
    let new_password_hash = "n".repeat(32);
    persistence::set_admin_password(&mut connection, &old_password_hash, 1)
        .expect("set administrator fixture");
    assert!(
        persistence::create_session(&connection, &old_password_hash, &[7_u8; 32], 1, 20,)
            .expect("create session fixture")
    );
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER fail_session_invalidation
             BEFORE DELETE ON sessions
             BEGIN
                 SELECT RAISE(ABORT, 'forced session delete failure');
             END;",
        )
        .expect("create failure trigger");

    let result = persistence::change_admin_password(
        &mut connection,
        &old_password_hash,
        &new_password_hash,
        2,
    );
    assert!(result.is_err());
    assert_eq!(
        persistence::load_admin_password_hash(&connection).expect("load administrator"),
        Some(old_password_hash)
    );
    assert!(
        persistence::find_session(&connection, &[7_u8; 32])
            .expect("load session")
            .is_some()
    );
}

/// The guarantee that keeps the placeholder window out of `TrafficState`: startup
/// hydration replaces any recovered window that is not the node's current cycle
/// with the configured one and zeroes its bytes, whether the stored window was a
/// different cycle or missing entirely.
#[tokio::test]
async fn unmatched_recovery_cycles_are_normalized_before_the_state_is_built() {
    const ANCIENT: i64 = 1_000_000;
    const RESET_DAY: i64 = 15;

    let path = TestDatabasePath::new("cycle-normalization");
    let database = Database::open(path.as_path()).expect("open database");
    let node = database
        .create_node(
            NewNodeRow {
                public_id: "cccccccccccccccccccccccccccccccc".into(),
                name: "Recovery node".into(),
                region_code: "US".into(),
                traffic_limit_bytes: None,
                traffic_reset_day: RESET_DAY,
                traffic_reset_mode: "monthly".to_owned(),
                price_micros: None,
                currency: None,
                renewal_cycle: None,
                expires_at: None,
            },
            [3; 32],
            1,
        )
        .await
        .expect("create node");
    database
        .persist_traffic_batch(
            vec![TrafficCheckpointRow {
                node_id: node.id,
                captured_generation: 1,
                rx_total_bytes: 500,
                tx_total_bytes: 400,
                last_rx_counter_bytes: 500,
                last_tx_counter_bytes: 400,
                last_boot_id: "boot".into(),
                day_start_utc: ANCIENT,
                today_rx_bytes: 500,
                today_tx_bytes: 400,
                cycle_start_utc: ANCIENT,
                cycle_end_utc: ANCIENT + 1_000,
                cycle_rx_bytes: 500,
                cycle_tx_bytes: 400,
                previous_day: None,
                previous_cycle: None,
                snapshot: checkpoint_snapshot(),
            }],
            ANCIENT + 900,
        )
        .await
        .expect("persist an ancient cycle");

    let now = crate::auth::unix_timestamp().expect("clock");
    let hydration = hydrate_startup(&database).await.expect("hydrate startup");
    assert_eq!(hydration.traffic_recovery.len(), 1);
    let recovered = &hydration.traffic_recovery[0];
    let expected = crate::time::billing_cycle(now, &hydration.settings.site_timezone, RESET_DAY)
        .expect("current cycle");
    assert_eq!(
        (recovered.cycle_start_utc, recovered.cycle_end_utc),
        (expected.start_utc, expected.end_utc),
        "the recovered window is the one the node is configured for"
    );
    assert!(recovered.cycle_start_utc >= 0);
    assert_eq!((recovered.cycle_rx_bytes, recovered.cycle_tx_bytes), (0, 0));
    // Lifetime totals and the counter baseline survive untouched.
    assert_eq!(
        (recovered.rx_total_bytes, recovered.tx_total_bytes),
        (500, 400)
    );
    assert_eq!(recovered.last_rx_counter_bytes, Some(500));
    database.shutdown().await.expect("shutdown database");
}

fn checkpoint_snapshot() -> crate::snapshot::NodeSnapshot {
    crate::snapshot::NodeSnapshot {
        live_since_start: true,
        first_seen_at: 900,
        last_seen_at: 950,
        last_ip: "127.0.0.1".parse().expect("IP"),
        hostname: "rollback".into(),
        os_name: "Debian".into(),
        os_version: "13".into(),
        kernel: "6.12".into(),
        architecture: "x86_64".into(),
        virtualization: "qemu".into(),
        agent_version: "0.1.0".into(),
        cpu_model: "CPU".into(),
        cpu_cores: 1,
        cpu_usage: 1.0,
        load_1: 0.1,
        load_5: 0.1,
        load_15: 0.1,
        memory_total: 100,
        memory_used: 50,
        swap_total: 0,
        swap_used: 0,
        disk_total: 100,
        disk_used: 50,
        rx_counter_bytes: 150,
        tx_counter_bytes: 275,
        rx_rate_bytes_per_sec: 1,
        tx_rate_bytes_per_sec: 1,
        uptime_seconds: 10,
        process_count: 2,
        boot_id: "boot".into(),
    }
}

/// A node with no stored bucket for the cycle it is now in -- the Server was down
/// across the boundary -- recovers with the `UNKNOWN_CYCLE_UTC` placeholder.
/// `hydrate_startup` replaces that window before any state is built from it (see
/// `unmatched_recovery_cycles_are_normalized_before_the_state_is_built`), so this
/// covers the layer beneath that: even if the placeholder did reach the state, it
/// is not a window `traffic_cycles` accepts and must never be handed off to it.
#[test]
fn a_placeholder_recovery_cycle_is_never_written_to_traffic_cycles() {
    const AUGUST: i64 = 1_000_000;
    const SEPTEMBER: i64 = 2_000_000;
    const OCTOBER: i64 = 3_000_000;
    const RESET_DAY: i64 = 15;

    let path = TestDatabasePath::new("traffic-unstored-cycle");
    let mut connection = open_ready_connection(path.as_path()).expect("open database");
    let node = persistence::create_node(
        &mut connection,
        &NewNodeRow {
            public_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            name: "Restart node".into(),
            region_code: "US".into(),
            traffic_limit_bytes: None,
            traffic_reset_day: RESET_DAY,
            traffic_reset_mode: "monthly".to_owned(),
            price_micros: None,
            currency: None,
            renewal_cycle: None,
            expires_at: None,
        },
        &[2; 32],
        1,
    )
    .expect("create node");
    let snapshot = checkpoint_snapshot();
    persistence::persist_traffic_batch(
        &mut connection,
        &[TrafficCheckpointRow {
            node_id: node.id,
            captured_generation: 1,
            rx_total_bytes: 500,
            tx_total_bytes: 400,
            last_rx_counter_bytes: 500,
            last_tx_counter_bytes: 400,
            last_boot_id: "boot".into(),
            day_start_utc: AUGUST + 500,
            today_rx_bytes: 500,
            today_tx_bytes: 400,
            cycle_start_utc: AUGUST,
            cycle_end_utc: SEPTEMBER,
            cycle_rx_bytes: 500,
            cycle_tx_bytes: 400,
            previous_day: None,
            previous_cycle: None,
            snapshot: snapshot.clone(),
        }],
        AUGUST + 900,
    )
    .expect("persist the August cycle");

    // Restart inside the September cycle, which has no stored bucket yet.
    let mut cycle_starts = [0_i64; crate::time::RESET_DAY_COUNT];
    cycle_starts[RESET_DAY as usize - 1] = SEPTEMBER;
    let recovery = persistence::load_traffic_recovery(&connection, SEPTEMBER + 500, &cycle_starts)
        .expect("load traffic recovery");
    assert_eq!(recovery.len(), 1);
    assert_eq!(recovery[0].cycle_rx_bytes, 0);
    assert_eq!(
        recovery[0].cycle_start_utc,
        crate::traffic::UNKNOWN_CYCLE_UTC
    );

    let traffic = crate::traffic::TrafficState::from_recovery(recovery);
    traffic
        .update(
            node.id,
            crate::traffic::TrafficSample {
                rx_counter_bytes: 700,
                tx_counter_bytes: 600,
                boot_id: "boot",
                day_start_utc: SEPTEMBER + 500,
                billing_cycle: crate::time::BillingCycle {
                    start_utc: SEPTEMBER,
                    end_utc: OCTOBER,
                },
            },
        )
        .expect("first report after the restart");
    let snapshots = std::collections::HashMap::from([(node.id, snapshot)]);
    let rows = traffic.capture_dirty(&snapshots);
    assert_eq!(rows.len(), 1);

    persistence::persist_traffic_batch(&mut connection, &rows, SEPTEMBER + 900)
        .expect("checkpoint after restarting into an unstored cycle");

    let cycles: Vec<(i64, i64, i64)> = connection
        .prepare(
            "SELECT cycle_start_utc, cycle_end_utc, rx_bytes FROM traffic_cycles
                  WHERE node_id = ?1 ORDER BY cycle_start_utc",
        )
        .expect("prepare cycle query")
        .query_map([node.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query cycles")
        .collect::<Result<_, _>>()
        .expect("read cycles");
    // The August bucket is untouched and September holds only the new delta; no
    // phantom bucket is invented for the window the restart never observed.
    assert_eq!(
        cycles,
        vec![(AUGUST, SEPTEMBER, 500), (SEPTEMBER, OCTOBER, 200)]
    );
}

#[test]
fn traffic_checkpoint_rolls_back_all_tables_on_late_failure() {
    let path = TestDatabasePath::new("traffic-rollback");
    let mut connection = open_ready_connection(path.as_path()).expect("open database");
    let node = persistence::create_node(
        &mut connection,
        &NewNodeRow {
            public_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            name: "Rollback node".into(),
            region_code: "US".into(),
            traffic_limit_bytes: None,
            traffic_reset_day: 1,
            traffic_reset_mode: "monthly".to_owned(),
            price_micros: None,
            currency: None,
            renewal_cycle: None,
            expires_at: None,
        },
        &[1; 32],
        1,
    )
    .expect("create node");
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER fail_last_state
             BEFORE INSERT ON node_last_state
             BEGIN
                 SELECT RAISE(ABORT, 'forced last-state failure');
             END;",
        )
        .expect("create checkpoint failure trigger");
    let checkpoint = TrafficCheckpointRow {
        node_id: node.id,
        captured_generation: 1,
        rx_total_bytes: 50,
        tx_total_bytes: 75,
        last_rx_counter_bytes: 150,
        last_tx_counter_bytes: 275,
        last_boot_id: "boot".into(),
        day_start_utc: 900,
        today_rx_bytes: 50,
        today_tx_bytes: 75,
        cycle_start_utc: 800,
        cycle_end_utc: 2_000,
        cycle_rx_bytes: 50,
        cycle_tx_bytes: 75,
        previous_day: None,
        previous_cycle: None,
        snapshot: checkpoint_snapshot(),
    };
    assert!(persistence::persist_traffic_batch(&mut connection, &[checkpoint], 1_000).is_err());
    let totals: (i64, i64, Option<i64>) = connection
        .query_row(
            "SELECT rx_total_bytes, tx_total_bytes, last_rx_counter_bytes
             FROM traffic_totals WHERE node_id = ?1",
            [node.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("traffic totals after rollback");
    assert_eq!(totals, (0, 0, None));
    for table in ["traffic_daily", "traffic_cycles", "node_last_state"] {
        let count: i64 = connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE node_id = ?1"),
                [node.id],
                |row| row.get(0),
            )
            .expect("count rolled back rows");
        assert_eq!(count, 0, "{table}");
    }
}

fn table_names(connection: &Connection) -> Vec<String> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .expect("prepare table query");
    statement
        .query_map([], |row| row.get(0))
        .expect("query tables")
        .collect::<Result<Vec<_>, _>>()
        .expect("read tables")
}

fn assert_primary_keys(connection: &Connection) {
    let expected = [
        ("settings", &["id"][..]),
        ("admin", &["id"][..]),
        ("sessions", &["token_hash"][..]),
        ("nodes", &["id"][..]),
        ("node_tokens", &["node_id"][..]),
        ("node_last_state", &["node_id"][..]),
        ("traffic_totals", &["node_id"][..]),
        ("traffic_daily", &["node_id", "day_start_utc"][..]),
        ("traffic_cycles", &["node_id", "cycle_start_utc"][..]),
        ("node_history", &["node_id", "bucket_ts"][..]),
        ("ping_targets", &["id"][..]),
        ("ping_history", &["node_id", "bucket_ts", "target_id"][..]),
    ];

    for (table, columns) in expected {
        assert_eq!(primary_key_columns(connection, table), columns, "{table}");
    }
}

fn assert_required_indexes(connection: &Connection) {
    assert!(has_unique_index(connection, "nodes", &["public_id"]));
    assert!(has_unique_index(connection, "node_tokens", &["token_hash"]));
    assert_eq!(
        index_columns(connection, "sessions_expires_at_idx"),
        ["expires_at"]
    );
    assert_eq!(
        index_columns(connection, "nodes_sort_order_idx"),
        ["sort_order", "id"]
    );
    assert_eq!(
        primary_key_columns(connection, "node_history"),
        ["node_id", "bucket_ts"]
    );
    assert_eq!(
        primary_key_columns(connection, "ping_history"),
        ["node_id", "bucket_ts", "target_id"]
    );
    assert_eq!(
        index_columns(connection, "ping_targets_order_idx"),
        ["enabled", "sort_order", "id"]
    );
    assert_eq!(
        index_descending_flags(connection, "ping_targets_order_idx"),
        [true, false, false]
    );
    assert_eq!(
        primary_key_columns(connection, "traffic_daily"),
        ["node_id", "day_start_utc"]
    );
    assert_eq!(
        primary_key_columns(connection, "traffic_cycles"),
        ["node_id", "cycle_start_utc"]
    );
}

fn primary_key_columns(connection: &Connection, table: &str) -> Vec<String> {
    let sql = format!("PRAGMA table_info('{table}')");
    let mut statement = connection.prepare(&sql).expect("prepare table_info");
    let mut columns = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
        })
        .expect("query table_info")
        .collect::<Result<Vec<_>, _>>()
        .expect("read table_info");
    columns.retain(|(position, _)| *position > 0);
    columns.sort_by_key(|(position, _)| *position);
    columns.into_iter().map(|(_, name)| name).collect()
}

fn has_unique_index(connection: &Connection, table: &str, columns: &[&str]) -> bool {
    let sql = format!("PRAGMA index_list('{table}')");
    let mut statement = connection.prepare(&sql).expect("prepare index_list");
    let indexes = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })
        .expect("query index_list")
        .collect::<Result<Vec<_>, _>>()
        .expect("read index_list");

    indexes
        .into_iter()
        .any(|(name, unique)| unique == 1 && index_columns(connection, &name) == columns)
}

fn index_columns(connection: &Connection, index: &str) -> Vec<String> {
    let sql = format!("PRAGMA index_info('{index}')");
    let mut statement = connection.prepare(&sql).expect("prepare index_info");
    statement
        .query_map([], |row| row.get(2))
        .expect("query index_info")
        .collect::<Result<Vec<_>, _>>()
        .expect("read index_info")
}

fn index_descending_flags(connection: &Connection, index: &str) -> Vec<bool> {
    let sql = format!("PRAGMA index_xinfo('{index}')");
    let mut statement = connection.prepare(&sql).expect("prepare index_xinfo");
    statement
        .query_map([], |row| {
            let is_key: i64 = row.get(5)?;
            let descending: i64 = row.get(3)?;
            Ok((is_key, descending != 0))
        })
        .expect("query index_xinfo")
        .collect::<Result<Vec<_>, _>>()
        .expect("read index_xinfo")
        .into_iter()
        .filter_map(|(is_key, descending)| (is_key == 1).then_some(descending))
        .collect()
}

fn history_node(public_id: &str) -> NewNodeRow {
    NewNodeRow {
        public_id: public_id.to_owned(),
        name: "History node".into(),
        region_code: "US".into(),
        traffic_limit_bytes: None,
        traffic_reset_day: 1,
        traffic_reset_mode: "monthly".to_owned(),
        price_micros: None,
        currency: None,
        renewal_cycle: None,
        expires_at: None,
    }
}

#[tokio::test]
async fn maintenance_cleans_daily_cycles_and_expired_sessions() {
    let path = TestDatabasePath::new("maintenance");
    let database = Database::open(path.as_path()).expect("open database");
    let node = database
        .create_node(history_node(&"c".repeat(32)), [4; 32], 1)
        .await
        .expect("create node");
    database.shutdown().await.expect("close for fixture");

    let connection = Connection::open(path.as_path()).expect("open fixture connection");
    for day in [1_000_i64, 3_000, 4_000] {
        connection
            .execute(
                "INSERT INTO traffic_daily
                 (node_id, day_start_utc, rx_bytes, tx_bytes, updated_at)
                 VALUES (?1, ?2, 1, 2, ?2)",
                params![node.id, day],
            )
            .expect("insert daily fixture");
    }
    for cycle in [100_i64, 200, 300, 400] {
        connection
            .execute(
                "INSERT INTO traffic_cycles
                 (node_id, cycle_start_utc, cycle_end_utc, rx_bytes, tx_bytes, updated_at)
                 VALUES (?1, ?2, ?3, 1, 2, ?2)",
                params![node.id, cycle, cycle + 50],
            )
            .expect("insert cycle fixture");
    }
    connection
        .execute(
            "INSERT INTO sessions (token_hash, created_at, expires_at)
             VALUES (?1, 1, 10), (?2, 1, 30)",
            params![vec![1_u8; 32], vec![2_u8; 32]],
        )
        .expect("insert session fixtures");
    drop(connection);

    let database = Database::open(path.as_path()).expect("reopen database");
    let result = database
        .cleanup_maintenance_batch(20, 3_000, 5_000)
        .await
        .expect("run maintenance cleanup");
    assert_eq!(result.expired_sessions, 1);
    assert_eq!(result.traffic_daily, 1);
    assert_eq!(result.traffic_cycles, 2);
    database.shutdown().await.expect("close database");

    let connection = Connection::open(path.as_path()).expect("inspect cleanup");
    let days = query_i64s(
        &connection,
        "SELECT day_start_utc FROM traffic_daily ORDER BY day_start_utc",
    );
    assert_eq!(days, [3_000, 4_000]);
    let cycles = query_i64s(
        &connection,
        "SELECT cycle_start_utc FROM traffic_cycles ORDER BY cycle_start_utc",
    );
    assert_eq!(cycles, [300, 400]);
    let expirations = query_i64s(
        &connection,
        "SELECT expires_at FROM sessions ORDER BY expires_at",
    );
    assert_eq!(expirations, [30]);
}

#[tokio::test]
async fn maintenance_cleanup_respects_combined_batch_limit() {
    let path = TestDatabasePath::new("maintenance-limit");
    Database::open(path.as_path())
        .expect("open database")
        .shutdown()
        .await
        .expect("close database");
    let mut connection = Connection::open(path.as_path()).expect("open fixture connection");
    let transaction = connection.transaction().expect("begin fixture");
    for value in 0_u8..10 {
        transaction
            .execute(
                "INSERT INTO sessions (token_hash, created_at, expires_at) VALUES (?1, 1, 2)",
                [vec![value; 32]],
            )
            .expect("insert expired session");
    }
    transaction.commit().expect("commit fixture");
    drop(connection);

    let database = Database::open(path.as_path()).expect("reopen database");
    let result = database
        .cleanup_maintenance_batch(3, 0, 3)
        .await
        .expect("run bounded cleanup");
    assert_eq!(result.total(), 3);
    database.shutdown().await.expect("close database");
    let connection = Connection::open(path.as_path()).expect("inspect cleanup");
    let remaining: i64 = connection
        .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
        .expect("count sessions");
    assert_eq!(remaining, 7);
}

#[tokio::test]
async fn passive_wal_checkpoint_succeeds_in_wal_mode() {
    let path = TestDatabasePath::new("passive-checkpoint");
    let database = Database::open(path.as_path()).expect("open database");
    assert_eq!(
        database
            .read_pragmas()
            .await
            .expect("read pragmas")
            .journal_mode,
        "wal"
    );
    let checkpoint = database
        .passive_wal_checkpoint()
        .await
        .expect("passive checkpoint");
    assert!(checkpoint.busy >= 0);
    assert!(checkpoint.log_frames >= 0);
    assert!(checkpoint.checkpointed_frames >= 0);
    database.shutdown().await.expect("close database");
}

fn query_i64s(connection: &Connection, sql: &str) -> Vec<i64> {
    connection
        .prepare(sql)
        .expect("prepare query")
        .query_map([], |row| row.get(0))
        .expect("query values")
        .collect::<Result<Vec<_>, _>>()
        .expect("read values")
}

fn resource_history_row(
    node_id: i64,
    bucket_ts: i64,
    sample_count: i64,
    cpu_usage_bp: i64,
) -> ResourceHistoryWriteRow {
    ResourceHistoryWriteRow {
        node_id,
        bucket_ts,
        sample_count,
        cpu_usage_bp,
        load_1_milli: 100,
        load_5_milli: 200,
        load_15_milli: 300,
        memory_used_bytes: bucket_ts + 10,
        swap_used_bytes: 0,
        disk_used_bytes: bucket_ts + 20,
        rx_rate_bytes_per_sec: cpu_usage_bp,
        tx_rate_bytes_per_sec: cpu_usage_bp * 2,
    }
}

#[tokio::test]
async fn history_batch_upsert_is_absolute_and_queries_weighted_five_minute_data() {
    let path = TestDatabasePath::new("history-upsert");
    let database = Database::open(path.as_path()).expect("open database");
    let node = database
        .create_node(history_node(&"a".repeat(32)), [1; 32], 1)
        .await
        .expect("create node");
    database
        .shutdown()
        .await
        .expect("close before target fixture");
    let connection = Connection::open(path.as_path()).expect("open fixture connection");
    connection
        .execute(
            "INSERT INTO ping_targets
             (id, name, host, ip_family, enabled, sort_order, created_at, updated_at)
             VALUES (1, 'enabled', '127.0.0.1', 4, 1, 1, 1, 1),
                    (2, 'disabled', '::1', 6, 0, 0, 1, 1)",
            [],
        )
        .expect("insert target fixtures");
    drop(connection);

    let database = Database::open(path.as_path()).expect("reopen database");
    let resources = vec![
        resource_history_row(node.id, 0, 1, 1_000),
        resource_history_row(node.id, 60, 3, 3_000),
    ];
    let pings = vec![
        PingHistoryWriteRow {
            node_id: node.id,
            bucket_ts: 0,
            target_id: 2,
            sample_count: 2,
            success_count: 1,
            latency_avg_ms: Some(10.0),
            latency_min_ms: Some(10.0),
            latency_max_ms: Some(10.0),
        },
        PingHistoryWriteRow {
            node_id: node.id,
            bucket_ts: 60,
            target_id: 2,
            sample_count: 4,
            success_count: 3,
            latency_avg_ms: Some(30.0),
            latency_min_ms: Some(20.0),
            latency_max_ms: Some(40.0),
        },
    ];
    database
        .persist_history_batch(resources.clone(), pings.clone())
        .await
        .expect("persist history");
    database
        .persist_history_batch(resources, pings)
        .await
        .expect("retry same absolute history");

    let minute = database
        .query_resource_history(node.id, 0, 300, 60)
        .await
        .expect("query minute history");
    assert_eq!(minute.len(), 2);
    let five_minute = database
        .query_resource_history(node.id, 0, 300, 300)
        .await
        .expect("query five-minute history");
    assert_eq!(five_minute.len(), 1);
    assert_eq!(five_minute[0].cpu_usage, 25.0);
    assert_eq!(five_minute[0].memory_used_bytes, 40);

    let ping = database
        .query_ping_history(node.id, 0, 300, 300)
        .await
        .expect("query ping history");
    assert_eq!(
        ping.len(),
        2,
        "enabled empty and disabled historical targets"
    );
    assert_eq!(ping[0].target_id, 2);
    assert_eq!(ping[0].latency_ms, Some(25.0));
    assert_eq!(ping[1].target_id, 1);
    assert_eq!(ping[1].bucket_ts, None);
    // A target with no bucket carries no counts at all, which is how packet loss
    // stays "unknown" instead of collapsing into "no samples lost".
    assert_eq!(ping[1].sample_count, None);
    assert_eq!(ping[1].success_count, None);

    database.shutdown().await.expect("shutdown database");
}

#[tokio::test]
async fn ping_history_counts_are_summed_before_any_ratio_is_taken() {
    let path = TestDatabasePath::new("ping-loss-counts");
    let database = Database::open(path.as_path()).expect("open database");
    let node = database
        .create_node(history_node(&"c".repeat(32)), [3; 32], 1)
        .await
        .expect("create node");
    database
        .shutdown()
        .await
        .expect("close before target fixture");
    let connection = Connection::open(path.as_path()).expect("open fixture connection");
    connection
        .execute(
            "INSERT INTO ping_targets
             (id, name, host, ip_family, enabled, sort_order, created_at, updated_at)
             VALUES (1, 'target', '127.0.0.1', 4, 1, 0, 1, 1)",
            [],
        )
        .expect("insert target fixture");
    drop(connection);
    let database = Database::open(path.as_path()).expect("reopen database");
    database
        .persist_history_batch(
            Vec::new(),
            vec![
                // Deliberately unequal minutes inside one five-minute bucket.
                PingHistoryWriteRow {
                    node_id: node.id,
                    bucket_ts: 0,
                    target_id: 1,
                    sample_count: 1,
                    success_count: 0,
                    latency_avg_ms: None,
                    latency_min_ms: None,
                    latency_max_ms: None,
                },
                PingHistoryWriteRow {
                    node_id: node.id,
                    bucket_ts: 60,
                    target_id: 1,
                    sample_count: 9,
                    success_count: 9,
                    latency_avg_ms: Some(5.0),
                    latency_min_ms: Some(5.0),
                    latency_max_ms: Some(5.0),
                },
            ],
        )
        .await
        .expect("persist ping fixtures");

    let minutes = database
        .query_ping_history(node.id, 0, 300, 60)
        .await
        .expect("query minute ping history");
    assert_eq!(
        minutes
            .iter()
            .map(|point| (point.bucket_ts, point.sample_count, point.success_count))
            .collect::<Vec<_>>(),
        vec![(Some(0), Some(1), Some(0)), (Some(60), Some(9), Some(9)),],
        "each stored minute carries its own counts"
    );

    let five_minutes = database
        .query_ping_history(node.id, 0, 300, 300)
        .await
        .expect("query five-minute ping history");
    assert_eq!(five_minutes.len(), 1);
    // (1 - 0)/1 = 1.0 and (9 - 9)/9 = 0.0, so averaging the per-minute ratios
    // would say 0.5. The summed counts say (10 - 9)/10 = 0.1.
    assert_eq!(five_minutes[0].sample_count, Some(10));
    assert_eq!(five_minutes[0].success_count, Some(9));
    let loss = (10.0 - 9.0) / 10.0;
    assert_eq!(loss, 0.1);
    // Latency stays success-weighted across the summed minutes.
    assert_eq!(five_minutes[0].latency_ms, Some(5.0));
    database.shutdown().await.expect("shutdown database");
}

#[tokio::test]
async fn all_failure_ping_stays_null_and_history_cleanup_respects_batch_limit() {
    let path = TestDatabasePath::new("history-cleanup");
    let database = Database::open(path.as_path()).expect("open database");
    let node = database
        .create_node(history_node(&"b".repeat(32)), [2; 32], 1)
        .await
        .expect("create node");
    database
        .shutdown()
        .await
        .expect("close before target fixture");
    let connection = Connection::open(path.as_path()).expect("open fixture connection");
    connection
        .execute(
            "INSERT INTO ping_targets
             (id, name, host, ip_family, enabled, sort_order, created_at, updated_at)
             VALUES (1, 'target', '127.0.0.1', 4, 1, 0, 1, 1)",
            [],
        )
        .expect("insert target fixture");
    drop(connection);
    let database = Database::open(path.as_path()).expect("reopen database");
    let mut resources: Vec<_> = (0..4)
        .map(|minute| resource_history_row(node.id, minute * 60, 1, 100))
        .collect();
    resources.push(resource_history_row(node.id, 20_040, 1, 200));
    database
        .persist_history_batch(
            resources,
            vec![
                PingHistoryWriteRow {
                    node_id: node.id,
                    bucket_ts: 0,
                    target_id: 1,
                    sample_count: 3,
                    success_count: 0,
                    latency_avg_ms: None,
                    latency_min_ms: None,
                    latency_max_ms: None,
                },
                PingHistoryWriteRow {
                    node_id: node.id,
                    bucket_ts: 20_040,
                    target_id: 1,
                    sample_count: 1,
                    success_count: 1,
                    latency_avg_ms: Some(5.0),
                    latency_min_ms: Some(5.0),
                    latency_max_ms: Some(5.0),
                },
            ],
        )
        .await
        .expect("persist fixtures");
    let ping = database
        .query_ping_history(node.id, 0, 300, 300)
        .await
        .expect("query failure ping");
    assert_eq!(ping[0].latency_ms, None);
    // A bucket whose samples all failed is real data: three samples, none
    // successful, which is total loss rather than an absent bucket.
    assert_eq!(
        (ping[0].sample_count, ping[0].success_count),
        (Some(3), Some(0))
    );

    assert_eq!(
        database
            .cleanup_history_batch(10_000, 10_000, 3)
            .await
            .expect("first cleanup"),
        3
    );
    let remaining = database
        .query_resource_history(node.id, 0, 300, 60)
        .await
        .expect("remaining resource rows");
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        database
            .cleanup_history_batch(10_000, 10_000, 3)
            .await
            .expect("second cleanup"),
        2
    );
    assert_eq!(
        database
            .query_resource_history(node.id, 20_000, 21_000, 60)
            .await
            .expect("retained resource row")
            .len(),
        1
    );
    let retained_ping = database
        .query_ping_history(node.id, 20_000, 21_000, 60)
        .await
        .expect("retained ping row");
    assert_eq!(retained_ping[0].latency_ms, Some(5.0));
    database.shutdown().await.expect("shutdown database");
}

#[tokio::test]
async fn stale_history_after_node_delete_is_ignored_without_reviving_node() {
    let path = TestDatabasePath::new("stale-history");
    let database = Database::open(path.as_path()).expect("open database");
    let public_id = "c".repeat(32);
    let node = database
        .create_node(history_node(&public_id), [3; 32], 1)
        .await
        .expect("create node");
    database
        .delete_node(public_id, 2)
        .await
        .expect("delete node")
        .expect("deleted row");
    database
        .persist_history_batch(
            vec![resource_history_row(node.id, 0, 1, 100)],
            vec![PingHistoryWriteRow {
                node_id: node.id,
                bucket_ts: 0,
                target_id: 99,
                sample_count: 1,
                success_count: 0,
                latency_avg_ms: None,
                latency_min_ms: None,
                latency_max_ms: None,
            }],
        )
        .await
        .expect("stale persistence is harmless");
    assert!(
        database
            .query_resource_history(node.id, 0, 60, 60)
            .await
            .expect("query stale history")
            .is_empty()
    );
    database.shutdown().await.expect("shutdown database");
}

#[test]
fn expected_table_list_has_no_duplicates() {
    let unique: BTreeSet<_> = EXPECTED_TABLES.into_iter().collect();
    assert_eq!(unique.len(), 12);
}

// ---------------------------------------------------------------------------
// Schema 2 foundation
// ---------------------------------------------------------------------------

const SCHEMA_ONE_SQL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../migrations/0001_initial.sql"
));
const SCHEMA_TWO_SQL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../migrations/0002_traffic_reset_mode_and_probe_targets.sql"
));

/// Renders rows so a comparison fails on any changed value, not merely on a
/// changed count. Types are tagged, so an integer that became text is visible.
fn rendered_rows(connection: &Connection, sql: &str) -> Vec<String> {
    use rusqlite::types::ValueRef;

    let mut statement = connection.prepare(sql).expect("prepare row snapshot");
    let columns = statement.column_count();
    let rows = statement
        .query_map([], |row| {
            let mut rendered = Vec::with_capacity(columns);
            for index in 0..columns {
                rendered.push(match row.get_ref(index)? {
                    ValueRef::Null => "NULL".to_owned(),
                    ValueRef::Integer(value) => format!("int:{value}"),
                    ValueRef::Real(value) => format!("real:{value}"),
                    ValueRef::Text(value) => {
                        format!("text:{}", String::from_utf8_lossy(value))
                    }
                    ValueRef::Blob(value) => format!(
                        "blob:{}",
                        value
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()
                    ),
                });
            }
            Ok(rendered.join("|"))
        })
        .expect("query row snapshot");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("collect row snapshot")
}

fn column_names(connection: &Connection, table: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .expect("prepare column list");
    let rows = statement
        .query_map([], |row| row.get(0))
        .expect("query column list");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("collect column list")
}

/// `notnull` and `dflt_value` straight from SQLite, so the assertions describe
/// the stored schema rather than the migration text.
fn column_shape(
    connection: &Connection,
    table: &str,
    column: &str,
) -> (String, i64, Option<String>) {
    connection
        .query_row(
            &format!(
                "SELECT type, \"notnull\", dflt_value FROM pragma_table_info('{table}') \
                 WHERE name = ?1"
            ),
            params![column],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read column shape")
}

fn foreign_key_violations(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .expect("run foreign key check")
}

fn schema_one_connection(path: &Path) -> Connection {
    let mut connection = Connection::open(path).expect("open schema 1 database");
    configure_connection(&connection).expect("configure schema 1 database");
    migrations::apply_single_migration(&mut connection, 1, SCHEMA_ONE_SQL)
        .expect("apply the v0.1.2 schema");
    assert_eq!(migrations::schema_version(&connection).unwrap(), 1);
    connection
}

/// Deliberately non-default values everywhere, so accidental defaulting,
/// truncation or column reordering during migration becomes visible.
fn populate_schema_one_fixture(connection: &Connection) {
    connection
        .execute_batch(
            "UPDATE settings SET site_name = 'fixture-site', site_timezone = 'Europe/Berlin',
                 theme_default = 'dark', history_retention_days = 7,
                 agent_report_interval_seconds = 5, ping_interval_seconds = 30,
                 offline_after_seconds = 20, default_traffic_reset_day = 29,
                 updated_at = 1700000001 WHERE id = 1;

             INSERT INTO admin (id, password_hash, updated_at)
             VALUES (1, '$argon2id$v=19$m=19456,t=2,p=1$Zml4dHVyZXNhbHQ$Zml4dHVyZWhhc2g', 1700000002);

             INSERT INTO sessions (token_hash, created_at, expires_at) VALUES
               (x'1111111111111111111111111111111111111111111111111111111111111111', 1700000010, 1700086410),
               (x'2222222222222222222222222222222222222222222222222222222222222222', 1700000011, 1700086411);

             INSERT INTO nodes (id, public_id, name, region_code, sort_order,
                 traffic_limit_bytes, traffic_reset_day, price_micros, currency,
                 renewal_cycle, expires_at, first_seen_at, created_at, updated_at) VALUES
               (11, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'tokyo-1', 'JP', 2,
                1099511627776, 29, 5990000, 'USD', 'annual', 1800000000, 1600000000, 1600000000, 1600000005),
               (22, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'frankfurt-2', 'DE', 0,
                2199023255552, 30, 1290000, 'EUR', 'monthly', 1790000000, 1610000000, 1610000000, 1610000006),
               (33, 'cccccccccccccccccccccccccccccccc', 'sao-paulo-3', 'BR', 1,
                NULL, 31, NULL, NULL, NULL, NULL, NULL, 1620000000, 1620000007);

             INSERT INTO node_tokens (node_id, token_hash, created_at) VALUES
               (11, x'3333333333333333333333333333333333333333333333333333333333333333', 1600000001),
               (22, x'4444444444444444444444444444444444444444444444444444444444444444', 1610000001),
               (33, x'5555555555555555555555555555555555555555555555555555555555555555', 1620000001);

             INSERT INTO node_last_state (node_id, hostname, os_name, os_version, kernel,
                 architecture, cpu_model, cpu_cores, virtualization, agent_version, boot_id,
                 cpu_usage_bp, load_1_milli, load_5_milli, load_15_milli,
                 memory_total_bytes, memory_used_bytes, swap_total_bytes, swap_used_bytes,
                 disk_total_bytes, disk_used_bytes, rx_rate_bytes_per_sec, tx_rate_bytes_per_sec,
                 uptime_seconds, process_count, last_ip, last_seen_at, persisted_at) VALUES
               (11, 'tokyo-host', 'Debian', '13', '6.12.0-amd64', 'x86_64', 'Xeon E5-2680', 8,
                'kvm', '0.1.2', 'boot-tokyo-1', 4237, 1250, 980, 770,
                8589934592, 3221225472, 1073741824, 268435456,
                53687091200, 21474836480, 1048576, 524288, 987654, 231, '203.0.113.7', 1700000100, 1700000101),
               (22, 'frankfurt-host', 'Ubuntu', '24.04', '6.8.0-arm64', 'aarch64', 'Ampere Altra', 4,
                'lxc', '0.1.2', 'boot-frankfurt-2', 812, 310, 290, 250,
                4294967296, 1073741824, 0, 0,
                26843545600, 5368709120, 65536, 32768, 123456, 98, '2001:db8::7', 1700000200, 1700000201);

             INSERT INTO traffic_totals (node_id, rx_total_bytes, tx_total_bytes,
                 last_rx_counter_bytes, last_tx_counter_bytes, last_boot_id, updated_at) VALUES
               (11, 987654321098, 123456789012, 55555555, 66666666, 'boot-tokyo-1', 1700000300),
               (22, 111111111111, 222222222222, 77777777, 88888888, 'boot-frankfurt-2', 1700000301),
               (33, 0, 0, NULL, NULL, NULL, 1700000302);

             INSERT INTO traffic_daily (node_id, day_start_utc, rx_bytes, tx_bytes, updated_at) VALUES
               (11, 1699920000, 5242880, 1048576, 1700000400),
               (11, 1700006400, 10485760, 2097152, 1700092700),
               (22, 1700006400, 3145728, 1572864, 1700092701);

             INSERT INTO traffic_cycles (node_id, cycle_start_utc, cycle_end_utc, rx_bytes, tx_bytes, updated_at) VALUES
               (11, 1698796800, 1701388800, 943718400, 209715200, 1700000500),
               (11, 1696118400, 1698796800, 838860800, 167772160, 1700000501),
               (22, 1698796800, 1701388800, 524288000, 104857600, 1700000502);

             INSERT INTO node_history (node_id, bucket_ts, sample_count, cpu_usage_bp,
                 load_1_milli, load_5_milli, load_15_milli, memory_used_bytes, swap_used_bytes,
                 disk_used_bytes, rx_rate_bytes_per_sec, tx_rate_bytes_per_sec) VALUES
               (11, 1700000040, 30, 4237, 1250, 980, 770, 3221225472, 268435456, 21474836480, 1048576, 524288),
               (11, 1700000100, 29, 3912, 1100, 950, 760, 3187671040, 268435456, 21474836480, 999424, 511000),
               (22, 1700000040, 28, 812, 310, 290, 250, 1073741824, 0, 5368709120, 65536, 32768);

             INSERT INTO ping_targets (id, name, host, ip_family, enabled, sort_order, created_at, updated_at) VALUES
               (101, 'cloudflare v4', '1.1.1.1', 4, 1, 0, 1600000100, 1600000101),
               (102, 'google v6', '2001:4860:4860::8888', 6, 1, 1, 1600000102, 1600000103),
               (103, 'disabled host', 'probe.example.com', 4, 0, 2, 1600000104, 1600000105);

             INSERT INTO ping_history (node_id, bucket_ts, target_id, sample_count, success_count,
                 latency_avg_ms, latency_min_ms, latency_max_ms) VALUES
               (11, 1700000040, 101, 4, 4, 12.5, 11.25, 14.75),
               (11, 1700000040, 102, 4, 3, 148.5, 140.0, 160.25),
               (11, 1700000100, 101, 4, 0, NULL, NULL, NULL),
               (22, 1700000040, 101, 3, 2, 7.125, 6.5, 8.0);",
        )
        .expect("populate the v0.1.2 fixture");
}

/// Every table, with nodes and ping_targets pinned to their schema-1 column list
/// so the comparison is like for like after two columns are added.
const FIXTURE_SNAPSHOTS: [(&str, &str); 11] = [
    ("settings", "SELECT * FROM settings ORDER BY id"),
    ("admin", "SELECT * FROM admin ORDER BY id"),
    ("sessions", "SELECT * FROM sessions ORDER BY token_hash"),
    (
        "nodes",
        "SELECT id, public_id, name, region_code, sort_order, traffic_limit_bytes,
                traffic_reset_day, price_micros, currency, renewal_cycle, expires_at,
                first_seen_at, created_at, updated_at FROM nodes ORDER BY id",
    ),
    ("node_tokens", "SELECT * FROM node_tokens ORDER BY node_id"),
    (
        "node_last_state",
        "SELECT * FROM node_last_state ORDER BY node_id",
    ),
    (
        "traffic_totals",
        "SELECT * FROM traffic_totals ORDER BY node_id",
    ),
    (
        "traffic_daily",
        "SELECT * FROM traffic_daily ORDER BY node_id, day_start_utc",
    ),
    (
        "traffic_cycles",
        "SELECT * FROM traffic_cycles ORDER BY node_id, cycle_start_utc",
    ),
    (
        "node_history",
        "SELECT * FROM node_history ORDER BY node_id, bucket_ts",
    ),
    (
        "ping_history",
        "SELECT * FROM ping_history ORDER BY node_id, bucket_ts, target_id",
    ),
];

const PING_TARGET_SNAPSHOT: &str =
    "SELECT id, name, host, ip_family, enabled, sort_order, created_at, updated_at
     FROM ping_targets ORDER BY id";

#[tokio::test]
async fn fresh_database_reaches_schema_two_with_the_declared_column_shapes() {
    let path = TestDatabasePath::new("schema-two-shape");
    let database = Database::open(path.as_path()).expect("open fresh database");
    assert_eq!(database.schema_version().await.expect("schema version"), 2);
    database.shutdown().await.expect("shutdown database");

    let connection = Connection::open(path.as_path()).expect("inspect database");
    // Additive only: no table was added or removed by schema 2.
    assert_eq!(table_names(&connection), EXPECTED_TABLES);

    assert!(column_names(&connection, "nodes").contains(&"traffic_reset_mode".to_owned()));
    assert_eq!(
        column_shape(&connection, "nodes", "traffic_reset_mode"),
        ("TEXT".to_owned(), 1, Some("'monthly'".to_owned()))
    );

    // probe_kind precedes port, which its CHECK references.
    let target_columns = column_names(&connection, "ping_targets");
    let probe_index = target_columns
        .iter()
        .position(|name| name == "probe_kind")
        .expect("probe_kind exists");
    let port_index = target_columns
        .iter()
        .position(|name| name == "port")
        .expect("port exists");
    assert!(probe_index < port_index);
    assert_eq!(
        column_shape(&connection, "ping_targets", "probe_kind"),
        ("TEXT".to_owned(), 1, Some("'icmp'".to_owned()))
    );
    assert_eq!(
        column_shape(&connection, "ping_targets", "port"),
        ("INTEGER".to_owned(), 0, None)
    );
}

#[test]
fn sqlite_itself_enforces_the_schema_two_constraints() {
    let path = TestDatabasePath::new("schema-two-checks");
    let mut connection = Connection::open(path.as_path()).expect("open database");
    configure_connection(&connection).expect("configure database");
    migrations::apply_single_migration(&mut connection, 1, SCHEMA_ONE_SQL).expect("schema 1");
    migrations::apply_single_migration(&mut connection, 2, SCHEMA_TWO_SQL).expect("schema 2");

    let node = |id: i64, mode: &str| {
        format!(
            "INSERT INTO nodes (id, public_id, name, region_code, traffic_reset_day,
                 created_at, updated_at, traffic_reset_mode)
             VALUES ({id}, '{:032x}', 'node-{id}', 'JP', 1, 1600000000, 1600000000, '{mode}')",
            id
        )
    };
    let target = |id: i64, kind: &str, port: &str| {
        format!(
            "INSERT INTO ping_targets (id, name, host, ip_family, created_at, updated_at,
                 probe_kind, port)
             VALUES ({id}, 'target-{id}', '1.1.1.1', 4, 1600000000, 1600000000, '{kind}', {port})"
        )
    };

    // Accepted boundaries.
    for (label, sql) in [
        ("monthly", node(1, "monthly")),
        ("never", node(2, "never")),
        ("icmp + NULL", target(1, "icmp", "NULL")),
        ("tcp + 1", target(2, "tcp", "1")),
        ("tcp + 65535", target(3, "tcp", "65535")),
    ] {
        connection
            .execute(&sql, [])
            .unwrap_or_else(|error| panic!("{label} must be accepted: {error}"));
    }

    // Rejections, proven against SQLite rather than a Rust validator.
    for (label, sql) in [
        ("traffic_reset_mode = 'daily'", node(3, "daily")),
        ("traffic_reset_mode = ''", node(4, "")),
        ("traffic_reset_mode = 'MONTHLY'", node(5, "MONTHLY")),
        ("probe_kind = 'udp'", target(4, "udp", "NULL")),
        ("tcp + port NULL", target(5, "tcp", "NULL")),
        ("tcp + port 0", target(6, "tcp", "0")),
        ("tcp + port 65536", target(7, "tcp", "65536")),
        ("tcp + port -1", target(8, "tcp", "-1")),
        ("icmp + non-NULL port", target(9, "icmp", "443")),
    ] {
        let error = connection
            .execute(&sql, [])
            .expect_err(&format!("{label} must be rejected"));
        assert!(
            error.to_string().contains("CHECK constraint failed"),
            "{label} was rejected for the wrong reason: {error}"
        );
    }

    // Enforcement is not insert-only: flipping a stored row must fail too.
    connection
        .execute(
            "UPDATE nodes SET traffic_reset_mode = 'never' WHERE id = 1",
            [],
        )
        .expect("monthly to never is allowed");
    connection
        .execute(
            "UPDATE nodes SET traffic_reset_mode = 'weekly' WHERE id = 1",
            [],
        )
        .expect_err("an invalid reset mode must be rejected on update");
    connection
        .execute(
            "UPDATE ping_targets SET probe_kind = 'tcp' WHERE id = 1",
            [],
        )
        .expect_err("turning an ICMP target into TCP without a port must be rejected");
    connection
        .execute("UPDATE ping_targets SET port = 443 WHERE id = 1", [])
        .expect_err("giving an ICMP target a port must be rejected");
    connection
        .execute(
            "UPDATE ping_targets SET probe_kind = 'tcp', port = 8443 WHERE id = 1",
            [],
        )
        .expect("switching kind and port together is allowed");
    connection
        .execute("UPDATE ping_targets SET port = NULL WHERE id = 2", [])
        .expect_err("dropping the port of a TCP target must be rejected");
}

#[tokio::test]
async fn v0_1_2_database_migrates_to_schema_two_without_losing_a_row() {
    let path = TestDatabasePath::new("schema-two-fixture");
    let connection = schema_one_connection(path.as_path());
    populate_schema_one_fixture(&connection);

    let before: Vec<(&str, Vec<String>)> = FIXTURE_SNAPSHOTS
        .iter()
        .map(|(table, sql)| (*table, rendered_rows(&connection, sql)))
        .collect();
    let targets_before = rendered_rows(&connection, PING_TARGET_SNAPSHOT);
    assert_eq!(foreign_key_violations(&connection), 0);
    for (table, rows) in &before {
        assert!(!rows.is_empty(), "{table} fixture is empty");
    }
    drop(connection);

    // The real startup path, not a hand-rolled migration call.
    let database = Database::open(path.as_path()).expect("migrate the v0.1.2 fixture");
    assert_eq!(database.schema_version().await.expect("schema version"), 2);
    database.shutdown().await.expect("shutdown database");

    let connection = Connection::open(path.as_path()).expect("inspect migrated database");
    configure_connection(&connection).expect("configure migrated database");
    assert_eq!(foreign_key_violations(&connection), 0);
    assert_eq!(table_names(&connection), EXPECTED_TABLES);
    assert_primary_keys(&connection);
    assert_required_indexes(&connection);

    for (table, rows) in &before {
        let sql = FIXTURE_SNAPSHOTS
            .iter()
            .find(|(name, _)| name == table)
            .map(|(_, sql)| *sql)
            .expect("snapshot query");
        assert_eq!(&rendered_rows(&connection, sql), rows, "{table} changed");
    }
    assert_eq!(
        rendered_rows(&connection, PING_TARGET_SNAPSHOT),
        targets_before,
        "ping_targets lost or altered a pre-existing value"
    );

    // Existing rows take the declared defaults and nothing else.
    assert_eq!(
        rendered_rows(
            &connection,
            "SELECT id, traffic_reset_mode FROM nodes ORDER BY id"
        ),
        vec![
            "int:11|text:monthly".to_owned(),
            "int:22|text:monthly".to_owned(),
            "int:33|text:monthly".to_owned(),
        ]
    );
    assert_eq!(
        rendered_rows(
            &connection,
            "SELECT id, probe_kind, port FROM ping_targets ORDER BY id"
        ),
        vec![
            "int:101|text:icmp|NULL".to_owned(),
            "int:102|text:icmp|NULL".to_owned(),
            "int:103|text:icmp|NULL".to_owned(),
        ]
    );
    // Stated separately because the loss columns matter to a later commit.
    assert_eq!(
        rendered_rows(
            &connection,
            "SELECT sample_count, success_count, latency_avg_ms, latency_min_ms, latency_max_ms
             FROM ping_history ORDER BY node_id, bucket_ts, target_id"
        ),
        vec![
            "int:4|int:4|real:12.5|real:11.25|real:14.75".to_owned(),
            "int:4|int:3|real:148.5|real:140|real:160.25".to_owned(),
            "int:4|int:0|NULL|NULL|NULL".to_owned(),
            "int:3|int:2|real:7.125|real:6.5|real:8".to_owned(),
        ]
    );
}

#[tokio::test]
async fn migrated_and_new_nodes_load_and_rehydrate_as_monthly() {
    let path = TestDatabasePath::new("reset-mode-hydration");
    let connection = schema_one_connection(path.as_path());
    populate_schema_one_fixture(&connection);
    drop(connection);

    // A node that existed under schema 1 hydrates as monthly, keeping its day.
    let database = Database::open(path.as_path()).expect("migrate and open");
    let hydration = hydrate_startup(&database).await.expect("hydrate");
    let migrated: Vec<(i64, String, i64)> = hydration
        .nodes
        .iter()
        .map(|node| {
            (
                node.id,
                node.traffic_reset_mode.clone(),
                node.traffic_reset_day,
            )
        })
        .collect();
    assert_eq!(
        migrated,
        vec![
            (22, "monthly".to_owned(), 30),
            (33, "monthly".to_owned(), 31),
            (11, "monthly".to_owned(), 29),
        ],
        "migrated nodes must load as monthly with their stored reset day"
    );

    // A node created after the migration defaults to monthly as well, and a
    // patched mode survives a restart.
    let created = database
        .create_node(
            NewNodeRow {
                public_id: "dddddddddddddddddddddddddddddddd".to_owned(),
                name: "after-migration".to_owned(),
                region_code: "SG".to_owned(),
                traffic_limit_bytes: None,
                traffic_reset_day: 7,
                traffic_reset_mode: "monthly".to_owned(),
                price_micros: None,
                currency: None,
                renewal_cycle: None,
                expires_at: None,
            },
            [9; 32],
            1_700_001_000,
        )
        .await
        .expect("create node");
    assert_eq!(created.traffic_reset_mode, "monthly");
    assert!(matches!(
        database
            .update_node(
                created.public_id.clone(),
                NodePatchRow {
                    traffic_reset_mode: Some("never".to_owned()),
                    ..NodePatchRow::default()
                },
                1_700_002_000,
            )
            .await
            .expect("patch mode"),
        UpdateNodeResult::Updated(_)
    ));
    database.shutdown().await.expect("shutdown");

    let database = Database::open(path.as_path()).expect("reopen");
    let hydration = hydrate_startup(&database).await.expect("rehydrate");
    let restarted = hydration
        .nodes
        .iter()
        .find(|node| node.public_id == created.public_id)
        .expect("created node after restart");
    assert_eq!(restarted.traffic_reset_mode, "never");
    assert_eq!(
        restarted.traffic_reset_day, 7,
        "switching to never must keep the stored reset day for a later switch back"
    );
    database.shutdown().await.expect("shutdown");
}

#[test]
fn a_failing_schema_two_migration_leaves_schema_one_untouched() {
    let path = TestDatabasePath::new("schema-two-atomic");
    let mut connection = schema_one_connection(path.as_path());
    populate_schema_one_fixture(&connection);
    let before: Vec<(&str, Vec<String>)> = FIXTURE_SNAPSHOTS
        .iter()
        .map(|(table, sql)| (*table, rendered_rows(&connection, sql)))
        .collect();
    let targets_before = rendered_rows(&connection, PING_TARGET_SNAPSHOT);

    // The real migration followed by a statement that cannot succeed, so the
    // failure lands after all three columns have been added. No production
    // failure hook is needed: the framework already applies a migration inside
    // one transaction and exposes it for exactly this.
    let failing = format!("{SCHEMA_TWO_SQL}\nINSERT INTO table_that_does_not_exist VALUES (1);");
    let result = migrations::apply_single_migration(&mut connection, 2, &failing);
    assert!(result.is_err(), "the broken migration must fail");

    assert_eq!(migrations::schema_version(&connection).unwrap(), 1);
    assert!(!column_names(&connection, "nodes").contains(&"traffic_reset_mode".to_owned()));
    let target_columns = column_names(&connection, "ping_targets");
    assert!(!target_columns.contains(&"probe_kind".to_owned()));
    assert!(!target_columns.contains(&"port".to_owned()));
    assert_eq!(foreign_key_violations(&connection), 0);
    for (table, rows) in &before {
        let sql = FIXTURE_SNAPSHOTS
            .iter()
            .find(|(name, _)| name == table)
            .map(|(_, sql)| *sql)
            .expect("snapshot query");
        assert_eq!(&rendered_rows(&connection, sql), rows, "{table} changed");
    }
    assert_eq!(
        rendered_rows(&connection, PING_TARGET_SNAPSHOT),
        targets_before
    );

    // And the real migration still succeeds on that untouched database.
    migrations::apply_single_migration(&mut connection, 2, SCHEMA_TWO_SQL)
        .expect("the same migration applies cleanly afterwards");
    assert_eq!(migrations::schema_version(&connection).unwrap(), 2);
    assert_eq!(foreign_key_violations(&connection), 0);
}
