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
        snapshot: crate::snapshot::NodeSnapshot {
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
        },
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

#[test]
fn expected_table_list_has_no_duplicates() {
    let unique: BTreeSet<_> = EXPECTED_TABLES.into_iter().collect();
    assert_eq!(unique.len(), 12);
}
