use rusqlite::{Connection, OptionalExtension, params};

use super::{
    DatabaseError,
    models::{NodeMetaRow, SettingsRow, SqlitePragmas, TrafficRecoveryRow},
};

pub(super) fn ensure_default_settings(connection: &Connection) -> Result<(), DatabaseError> {
    connection
        .execute(
            "INSERT OR IGNORE INTO settings (
                id, site_name, site_timezone, theme_default,
                history_retention_days, agent_report_interval_seconds,
                ping_interval_seconds, offline_after_seconds,
                default_traffic_reset_day, updated_at
             ) VALUES (1, 'Monitor', 'Asia/Shanghai', 'system', 30, 2, 15, 10, 1, unixepoch())",
            [],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "ensure default settings",
            source,
        })?;
    Ok(())
}

pub(super) fn load_settings(connection: &Connection) -> Result<SettingsRow, DatabaseError> {
    connection
        .query_row(
            "SELECT site_name, site_timezone, theme_default,
                    history_retention_days, agent_report_interval_seconds,
                    ping_interval_seconds, offline_after_seconds,
                    default_traffic_reset_day, updated_at
             FROM settings WHERE id = 1",
            [],
            |row| {
                Ok(SettingsRow {
                    site_name: row.get(0)?,
                    site_timezone: row.get(1)?,
                    theme_default: row.get(2)?,
                    history_retention_days: row.get(3)?,
                    agent_report_interval_seconds: row.get(4)?,
                    ping_interval_seconds: row.get(5)?,
                    offline_after_seconds: row.get(6)?,
                    default_traffic_reset_day: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(|source| DatabaseError::Sql {
            operation: "load settings",
            source,
        })?
        .ok_or(DatabaseError::MissingSettings)
}

pub(super) fn upsert_settings(
    connection: &mut Connection,
    settings: &SettingsRow,
) -> Result<(), DatabaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| DatabaseError::Sql {
            operation: "begin settings transaction",
            source,
        })?;

    transaction
        .execute(
            "INSERT INTO settings (
                id, site_name, site_timezone, theme_default,
                history_retention_days, agent_report_interval_seconds,
                ping_interval_seconds, offline_after_seconds,
                default_traffic_reset_day, updated_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                site_name = excluded.site_name,
                site_timezone = excluded.site_timezone,
                theme_default = excluded.theme_default,
                history_retention_days = excluded.history_retention_days,
                agent_report_interval_seconds = excluded.agent_report_interval_seconds,
                ping_interval_seconds = excluded.ping_interval_seconds,
                offline_after_seconds = excluded.offline_after_seconds,
                default_traffic_reset_day = excluded.default_traffic_reset_day,
                updated_at = excluded.updated_at",
            params![
                settings.site_name,
                settings.site_timezone,
                settings.theme_default,
                settings.history_retention_days,
                settings.agent_report_interval_seconds,
                settings.ping_interval_seconds,
                settings.offline_after_seconds,
                settings.default_traffic_reset_day,
                settings.updated_at,
            ],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "upsert settings",
            source,
        })?;

    transaction.commit().map_err(|source| DatabaseError::Sql {
        operation: "commit settings transaction",
        source,
    })
}

pub(super) fn load_node_metadata(
    connection: &Connection,
) -> Result<Vec<NodeMetaRow>, DatabaseError> {
    let mut statement = connection
        .prepare(
            "SELECT id, public_id, name, region_code, sort_order,
                    traffic_limit_bytes, traffic_reset_day, price_micros,
                    currency, renewal_cycle, expires_at, first_seen_at
             FROM nodes ORDER BY sort_order, id",
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "prepare startup node metadata query",
            source,
        })?;

    let rows = statement
        .query_map([], |row| {
            Ok(NodeMetaRow {
                id: row.get(0)?,
                public_id: row.get(1)?,
                name: row.get(2)?,
                region_code: row.get(3)?,
                sort_order: row.get(4)?,
                traffic_limit_bytes: row.get(5)?,
                traffic_reset_day: row.get(6)?,
                price_micros: row.get(7)?,
                currency: row.get(8)?,
                renewal_cycle: row.get(9)?,
                expires_at: row.get(10)?,
                first_seen_at: row.get(11)?,
            })
        })
        .map_err(|source| DatabaseError::Sql {
            operation: "query startup node metadata",
            source,
        })?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| DatabaseError::Sql {
            operation: "read startup node metadata",
            source,
        })
}

pub(super) fn load_traffic_recovery(
    connection: &Connection,
) -> Result<Vec<TrafficRecoveryRow>, DatabaseError> {
    let mut statement = connection
        .prepare(
            "SELECT node_id, rx_total_bytes, tx_total_bytes,
                    last_rx_counter_bytes, last_tx_counter_bytes, last_boot_id
             FROM traffic_totals ORDER BY node_id",
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "prepare startup traffic recovery query",
            source,
        })?;

    let rows = statement
        .query_map([], |row| {
            Ok(TrafficRecoveryRow {
                node_id: row.get(0)?,
                rx_total_bytes: row.get(1)?,
                tx_total_bytes: row.get(2)?,
                last_rx_counter_bytes: row.get(3)?,
                last_tx_counter_bytes: row.get(4)?,
                last_boot_id: row.get(5)?,
            })
        })
        .map_err(|source| DatabaseError::Sql {
            operation: "query startup traffic recovery state",
            source,
        })?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| DatabaseError::Sql {
            operation: "read startup traffic recovery state",
            source,
        })
}

pub(super) fn read_pragmas(connection: &Connection) -> Result<SqlitePragmas, DatabaseError> {
    Ok(SqlitePragmas {
        foreign_keys: pragma_i64(connection, "foreign_keys")?,
        journal_mode: connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|source| DatabaseError::Sql {
                operation: "read PRAGMA journal_mode",
                source,
            })?,
        busy_timeout_ms: pragma_i64(connection, "busy_timeout")?,
        synchronous: pragma_i64(connection, "synchronous")?,
        temp_store: pragma_i64(connection, "temp_store")?,
        cache_size: pragma_i64(connection, "cache_size")?,
        wal_autocheckpoint: pragma_i64(connection, "wal_autocheckpoint")?,
        page_size: pragma_i64(connection, "page_size")?,
    })
}

fn pragma_i64(connection: &Connection, name: &'static str) -> Result<i64, DatabaseError> {
    connection
        .pragma_query_value(None, name, |row| row.get(0))
        .map_err(|source| DatabaseError::Sql {
            operation: "read SQLite PRAGMA",
            source,
        })
}
