use rusqlite::{Connection, TransactionBehavior};

use super::DatabaseError;

pub const CURRENT_SCHEMA_VERSION: i64 = 1;

const MIGRATIONS: &[(i64, &str)] = &[(
    1,
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../migrations/0001_initial.sql"
    )),
)];

pub(super) fn apply_migrations(connection: &mut Connection) -> Result<(), DatabaseError> {
    let installed_version = schema_version(connection)?;
    if installed_version > CURRENT_SCHEMA_VERSION {
        return Err(DatabaseError::UnsupportedSchemaVersion {
            installed: installed_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    for &(version, sql) in MIGRATIONS {
        if version > installed_version {
            apply_single_migration(connection, version, sql)?;
        }
    }

    Ok(())
}

pub(super) fn schema_version(connection: &Connection) -> Result<i64, DatabaseError> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|source| DatabaseError::Sql {
            operation: "read schema version",
            source,
        })
}

pub(super) fn apply_single_migration(
    connection: &mut Connection,
    version: i64,
    sql: &str,
) -> Result<(), DatabaseError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| DatabaseError::Migration { version, source })?;

    transaction
        .execute_batch(sql)
        .map_err(|source| DatabaseError::Migration { version, source })?;
    transaction
        .pragma_update(None, "user_version", version)
        .map_err(|source| DatabaseError::Migration { version, source })?;
    transaction
        .commit()
        .map_err(|source| DatabaseError::Migration { version, source })
}
