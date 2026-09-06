use std::io;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params, types::Type};

use super::{
    DatabaseError,
    models::{
        AdminNodeRow, DeletedNodeRow, NewNodeRow, NodeMetaRow, NodePatchRow, NodeTokenRow,
        NodeUpdateResult, RotatedNodeTokenRow, SessionRow, SettingsRow, SqlitePragmas,
        TrafficRecoveryRow, UpdateNodeResult,
    },
};

pub(super) fn load_admin_password_hash(
    connection: &Connection,
) -> Result<Option<String>, DatabaseError> {
    connection
        .query_row("SELECT password_hash FROM admin WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|source| DatabaseError::Sql {
            operation: "load administrator password hash",
            source,
        })
}

pub(super) fn set_admin_password(
    connection: &mut Connection,
    password_hash: &str,
    updated_at: i64,
) -> Result<(), DatabaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| DatabaseError::Sql {
            operation: "begin administrator password transaction",
            source,
        })?;
    transaction
        .execute(
            "INSERT INTO admin (id, password_hash, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                password_hash = excluded.password_hash,
                updated_at = excluded.updated_at",
            params![password_hash, updated_at],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "set administrator password",
            source,
        })?;
    transaction
        .execute("DELETE FROM sessions", [])
        .map_err(|source| DatabaseError::Sql {
            operation: "invalidate sessions after setting administrator password",
            source,
        })?;
    transaction.commit().map_err(|source| DatabaseError::Sql {
        operation: "commit administrator password transaction",
        source,
    })
}

pub(super) fn change_admin_password(
    connection: &mut Connection,
    expected_password_hash: &str,
    new_password_hash: &str,
    updated_at: i64,
) -> Result<bool, DatabaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| DatabaseError::Sql {
            operation: "begin administrator password change transaction",
            source,
        })?;
    let changed = transaction
        .execute(
            "UPDATE admin SET password_hash = ?1, updated_at = ?2
             WHERE id = 1 AND password_hash = ?3",
            params![new_password_hash, updated_at, expected_password_hash],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "change administrator password",
            source,
        })?;
    if changed == 0 {
        transaction.commit().map_err(|source| DatabaseError::Sql {
            operation: "commit unchanged administrator password transaction",
            source,
        })?;
        return Ok(false);
    }

    transaction
        .execute("DELETE FROM sessions", [])
        .map_err(|source| DatabaseError::Sql {
            operation: "invalidate sessions after administrator password change",
            source,
        })?;
    transaction.commit().map_err(|source| DatabaseError::Sql {
        operation: "commit administrator password change transaction",
        source,
    })?;
    Ok(true)
}

pub(super) fn create_session(
    connection: &Connection,
    expected_password_hash: &str,
    token_hash: &[u8; 32],
    created_at: i64,
    expires_at: i64,
) -> Result<bool, DatabaseError> {
    connection
        .execute(
            "INSERT INTO sessions (token_hash, created_at, expires_at)
             SELECT ?1, ?2, ?3
             WHERE EXISTS (
                SELECT 1 FROM admin WHERE id = 1 AND password_hash = ?4
             )",
            params![
                token_hash.as_slice(),
                created_at,
                expires_at,
                expected_password_hash,
            ],
        )
        .map(|inserted| inserted != 0)
        .map_err(|source| DatabaseError::Sql {
            operation: "create administrator session",
            source,
        })
}

pub(super) fn find_session(
    connection: &Connection,
    token_hash: &[u8; 32],
) -> Result<Option<SessionRow>, DatabaseError> {
    connection
        .query_row(
            "SELECT created_at, expires_at FROM sessions WHERE token_hash = ?1",
            [token_hash.as_slice()],
            |row| {
                Ok(SessionRow {
                    created_at: row.get(0)?,
                    expires_at: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(|source| DatabaseError::Sql {
            operation: "find administrator session",
            source,
        })
}

pub(super) fn delete_session(
    connection: &Connection,
    token_hash: &[u8; 32],
) -> Result<bool, DatabaseError> {
    connection
        .execute(
            "DELETE FROM sessions WHERE token_hash = ?1",
            [token_hash.as_slice()],
        )
        .map(|deleted| deleted != 0)
        .map_err(|source| DatabaseError::Sql {
            operation: "delete administrator session",
            source,
        })
}

pub(super) fn delete_all_sessions(connection: &Connection) -> Result<usize, DatabaseError> {
    connection
        .execute("DELETE FROM sessions", [])
        .map_err(|source| DatabaseError::Sql {
            operation: "delete all administrator sessions",
            source,
        })
}

pub(super) fn delete_expired_sessions(
    connection: &Connection,
    now: i64,
) -> Result<usize, DatabaseError> {
    connection
        .execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])
        .map_err(|source| DatabaseError::Sql {
            operation: "delete expired administrator sessions",
            source,
        })
}

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
        .query_map([], node_meta_from_row)
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

pub(super) fn load_node_tokens(
    connection: &Connection,
) -> Result<Vec<NodeTokenRow>, DatabaseError> {
    let mut statement = connection
        .prepare("SELECT node_id, token_hash FROM node_tokens ORDER BY node_id")
        .map_err(|source| DatabaseError::Sql {
            operation: "prepare startup node token query",
            source,
        })?;
    let rows = statement
        .query_map([], |row| {
            Ok(NodeTokenRow {
                node_id: row.get(0)?,
                token_hash: hash_from_row(row, 1)?,
            })
        })
        .map_err(|source| DatabaseError::Sql {
            operation: "query startup node tokens",
            source,
        })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| DatabaseError::Sql {
            operation: "read startup node tokens",
            source,
        })
}

pub(super) fn list_admin_nodes(
    connection: &Connection,
    day_start_utc: i64,
    now: i64,
) -> Result<Vec<AdminNodeRow>, DatabaseError> {
    let mut statement = connection
        .prepare(
            "SELECT
                n.id, n.public_id, n.name, n.region_code, n.sort_order,
                n.traffic_limit_bytes, n.traffic_reset_day, n.price_micros,
                n.currency, n.renewal_cycle, n.expires_at, n.first_seen_at,
                s.last_ip, s.last_seen_at,
                COALESCE(t.rx_total_bytes, 0), COALESCE(t.tx_total_bytes, 0),
                COALESCE(d.rx_bytes, 0), COALESCE(d.tx_bytes, 0),
                COALESCE(c.rx_bytes, 0), COALESCE(c.tx_bytes, 0)
             FROM nodes AS n
             LEFT JOIN node_last_state AS s ON s.node_id = n.id
             LEFT JOIN traffic_totals AS t ON t.node_id = n.id
             LEFT JOIN traffic_daily AS d
               ON d.node_id = n.id AND d.day_start_utc = ?1
             LEFT JOIN traffic_cycles AS c
               ON c.node_id = n.id AND c.cycle_start_utc <= ?2 AND c.cycle_end_utc > ?2
             ORDER BY n.sort_order, n.id",
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "prepare administrator node list query",
            source,
        })?;
    let rows = statement
        .query_map(params![day_start_utc, now], |row| {
            Ok(AdminNodeRow {
                node: node_meta_from_row(row)?,
                last_ip: row.get(12)?,
                last_seen_at: row.get(13)?,
                total_rx_bytes: row.get(14)?,
                total_tx_bytes: row.get(15)?,
                today_rx_bytes: row.get(16)?,
                today_tx_bytes: row.get(17)?,
                cycle_rx_bytes: row.get(18)?,
                cycle_tx_bytes: row.get(19)?,
            })
        })
        .map_err(|source| DatabaseError::Sql {
            operation: "query administrator node list",
            source,
        })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| DatabaseError::Sql {
            operation: "read administrator node list",
            source,
        })
}

pub(super) fn create_node(
    connection: &mut Connection,
    node: &NewNodeRow,
    token_hash: &[u8; 32],
    now: i64,
) -> Result<NodeMetaRow, DatabaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| DatabaseError::Sql {
            operation: "begin node creation transaction",
            source,
        })?;
    let sort_order: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM nodes",
            [],
            |row| row.get(0),
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "select next node sort order",
            source,
        })?;
    transaction
        .execute(
            "INSERT INTO nodes (
                public_id, name, region_code, sort_order, traffic_limit_bytes,
                traffic_reset_day, price_micros, currency, renewal_cycle,
                expires_at, first_seen_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?11)",
            params![
                node.public_id,
                node.name,
                node.region_code,
                sort_order,
                node.traffic_limit_bytes,
                node.traffic_reset_day,
                node.price_micros,
                node.currency,
                node.renewal_cycle,
                node.expires_at,
                now,
            ],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "insert node",
            source,
        })?;
    let node_id = transaction.last_insert_rowid();
    transaction
        .execute(
            "INSERT INTO node_tokens (node_id, token_hash, created_at) VALUES (?1, ?2, ?3)",
            params![node_id, token_hash.as_slice(), now],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "insert node token hash",
            source,
        })?;
    transaction
        .execute(
            "INSERT INTO traffic_totals (
                node_id, rx_total_bytes, tx_total_bytes, last_rx_counter_bytes,
                last_tx_counter_bytes, last_boot_id, updated_at
             ) VALUES (?1, 0, 0, NULL, NULL, NULL, ?2)",
            params![node_id, now],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "insert initial node traffic totals",
            source,
        })?;
    let created = NodeMetaRow {
        id: node_id,
        public_id: node.public_id.clone(),
        name: node.name.clone(),
        region_code: node.region_code.clone(),
        sort_order,
        traffic_limit_bytes: node.traffic_limit_bytes,
        traffic_reset_day: node.traffic_reset_day,
        price_micros: node.price_micros,
        currency: node.currency.clone(),
        renewal_cycle: node.renewal_cycle.clone(),
        expires_at: node.expires_at,
        first_seen_at: None,
    };
    transaction.commit().map_err(|source| DatabaseError::Sql {
        operation: "commit node creation transaction",
        source,
    })?;
    Ok(created)
}

pub(super) fn update_node(
    connection: &mut Connection,
    public_id: &str,
    patch: &NodePatchRow,
    now: i64,
) -> Result<UpdateNodeResult, DatabaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| DatabaseError::Sql {
            operation: "begin node update transaction",
            source,
        })?;
    let Some(existing) = select_node_by_public_id(&transaction, public_id)? else {
        return Ok(UpdateNodeResult::NotFound);
    };
    let new_order = patch.sort_order.unwrap_or(existing.sort_order);
    let node_count: i64 = transaction
        .query_row("SELECT count(*) FROM nodes", [], |row| row.get(0))
        .map_err(|source| DatabaseError::Sql {
            operation: "count nodes for reorder",
            source,
        })?;
    if new_order < 0 || new_order >= node_count {
        return Ok(UpdateNodeResult::InvalidSortOrder);
    }

    if existing.sort_order < new_order {
        transaction
            .execute(
                "UPDATE nodes SET sort_order = sort_order - 1, updated_at = ?1
                 WHERE sort_order > ?2 AND sort_order <= ?3",
                params![now, existing.sort_order, new_order],
            )
            .map_err(|source| DatabaseError::Sql {
                operation: "shift nodes toward lower sort order",
                source,
            })?;
    } else if existing.sort_order > new_order {
        transaction
            .execute(
                "UPDATE nodes SET sort_order = sort_order + 1, updated_at = ?1
                 WHERE sort_order >= ?2 AND sort_order < ?3",
                params![now, new_order, existing.sort_order],
            )
            .map_err(|source| DatabaseError::Sql {
                operation: "shift nodes toward higher sort order",
                source,
            })?;
    }

    let updated = NodeMetaRow {
        id: existing.id,
        public_id: existing.public_id,
        name: patch.name.clone().unwrap_or(existing.name),
        region_code: patch.region_code.clone().unwrap_or(existing.region_code),
        sort_order: new_order,
        traffic_limit_bytes: patch
            .traffic_limit_bytes
            .unwrap_or(existing.traffic_limit_bytes),
        traffic_reset_day: patch
            .traffic_reset_day
            .unwrap_or(existing.traffic_reset_day),
        price_micros: patch.price_micros.unwrap_or(existing.price_micros),
        currency: patch.currency.clone().unwrap_or(existing.currency),
        renewal_cycle: patch
            .renewal_cycle
            .clone()
            .unwrap_or(existing.renewal_cycle),
        expires_at: patch.expires_at.unwrap_or(existing.expires_at),
        first_seen_at: existing.first_seen_at,
    };
    if updated.price_micros.is_some() != updated.currency.is_some() {
        return Ok(UpdateNodeResult::InvalidConfiguration);
    }
    transaction
        .execute(
            "UPDATE nodes SET
                name = ?1, region_code = ?2, sort_order = ?3,
                traffic_limit_bytes = ?4, traffic_reset_day = ?5,
                price_micros = ?6, currency = ?7, renewal_cycle = ?8,
                expires_at = ?9, updated_at = ?10
             WHERE id = ?11",
            params![
                updated.name,
                updated.region_code,
                updated.sort_order,
                updated.traffic_limit_bytes,
                updated.traffic_reset_day,
                updated.price_micros,
                updated.currency,
                updated.renewal_cycle,
                updated.expires_at,
                now,
                updated.id,
            ],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "update node",
            source,
        })?;
    let reordered_nodes = if existing.sort_order == new_order {
        Vec::new()
    } else {
        select_node_orders(
            &transaction,
            existing.sort_order.min(new_order),
            existing.sort_order.max(new_order),
        )?
    };
    transaction.commit().map_err(|source| DatabaseError::Sql {
        operation: "commit node update transaction",
        source,
    })?;
    Ok(UpdateNodeResult::Updated(Box::new(NodeUpdateResult {
        node: updated,
        reordered_nodes,
    })))
}

pub(super) fn delete_node(
    connection: &mut Connection,
    public_id: &str,
    now: i64,
) -> Result<Option<DeletedNodeRow>, DatabaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| DatabaseError::Sql {
            operation: "begin node deletion transaction",
            source,
        })?;
    let deleted = transaction
        .query_row(
            "SELECT n.id, t.token_hash, n.sort_order
             FROM nodes AS n JOIN node_tokens AS t ON t.node_id = n.id
             WHERE n.public_id = ?1",
            [public_id],
            |row| Ok((row.get(0)?, hash_from_row(row, 1)?, row.get::<_, i64>(2)?)),
        )
        .optional()
        .map_err(|source| DatabaseError::Sql {
            operation: "find node for deletion",
            source,
        })?;
    let Some((node_id, token_hash, sort_order)) = deleted else {
        return Ok(None);
    };
    transaction
        .execute("DELETE FROM nodes WHERE id = ?1", [node_id])
        .map_err(|source| DatabaseError::Sql {
            operation: "delete node",
            source,
        })?;
    transaction
        .execute(
            "UPDATE nodes SET sort_order = sort_order - 1, updated_at = ?1
             WHERE sort_order > ?2",
            params![now, sort_order],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "close node sort order gap",
            source,
        })?;
    let reordered_nodes = select_node_orders(&transaction, sort_order, i64::MAX)?;
    transaction.commit().map_err(|source| DatabaseError::Sql {
        operation: "commit node deletion transaction",
        source,
    })?;
    Ok(Some(DeletedNodeRow {
        node_id,
        token_hash,
        reordered_nodes,
    }))
}

pub(super) fn rotate_node_token(
    connection: &mut Connection,
    public_id: &str,
    new_token_hash: &[u8; 32],
    now: i64,
) -> Result<Option<RotatedNodeTokenRow>, DatabaseError> {
    let transaction = connection
        .transaction()
        .map_err(|source| DatabaseError::Sql {
            operation: "begin node token rotation transaction",
            source,
        })?;
    let current = transaction
        .query_row(
            "SELECT n.id, t.token_hash
             FROM nodes AS n JOIN node_tokens AS t ON t.node_id = n.id
             WHERE n.public_id = ?1",
            [public_id],
            |row| Ok((row.get(0)?, hash_from_row(row, 1)?)),
        )
        .optional()
        .map_err(|source| DatabaseError::Sql {
            operation: "find node token for rotation",
            source,
        })?;
    let Some((node_id, old_token_hash)) = current else {
        return Ok(None);
    };
    transaction
        .execute(
            "UPDATE node_tokens SET token_hash = ?1, created_at = ?2 WHERE node_id = ?3",
            params![new_token_hash.as_slice(), now, node_id],
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "rotate node token hash",
            source,
        })?;
    transaction.commit().map_err(|source| DatabaseError::Sql {
        operation: "commit node token rotation transaction",
        source,
    })?;
    Ok(Some(RotatedNodeTokenRow {
        node_id,
        old_token_hash,
    }))
}

fn select_node_by_public_id(
    transaction: &Transaction<'_>,
    public_id: &str,
) -> Result<Option<NodeMetaRow>, DatabaseError> {
    transaction
        .query_row(
            "SELECT id, public_id, name, region_code, sort_order,
                    traffic_limit_bytes, traffic_reset_day, price_micros,
                    currency, renewal_cycle, expires_at, first_seen_at
             FROM nodes WHERE public_id = ?1",
            [public_id],
            node_meta_from_row,
        )
        .optional()
        .map_err(|source| DatabaseError::Sql {
            operation: "find node by public id",
            source,
        })
}

fn select_node_orders(
    transaction: &Transaction<'_>,
    first: i64,
    last: i64,
) -> Result<Vec<(i64, i64)>, DatabaseError> {
    let mut statement = transaction
        .prepare(
            "SELECT id, sort_order FROM nodes
             WHERE sort_order >= ?1 AND sort_order <= ?2
             ORDER BY sort_order, id",
        )
        .map_err(|source| DatabaseError::Sql {
            operation: "prepare changed node order query",
            source,
        })?;
    let rows = statement
        .query_map(params![first, last], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|source| DatabaseError::Sql {
            operation: "query changed node orders",
            source,
        })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| DatabaseError::Sql {
            operation: "read changed node orders",
            source,
        })
}

fn node_meta_from_row(row: &Row<'_>) -> rusqlite::Result<NodeMetaRow> {
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
}

fn hash_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<[u8; 32]> {
    let bytes: Vec<u8> = row.get(index)?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            Type::Blob,
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected 32-byte hash, got {} bytes", bytes.len()),
            )),
        )
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
