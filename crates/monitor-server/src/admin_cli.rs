use std::{
    io::{self, BufRead, Write},
    path::Path,
};

use crate::{
    auth::{AuthError, hash_password, unix_timestamp, validate_password},
    database::{Database, DatabaseError},
};

#[derive(Debug)]
pub enum AdminCliError {
    Input(io::Error),
    Output(io::Error),
    Authentication(AuthError),
    Database(DatabaseError),
}

impl std::fmt::Display for AdminCliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(source) => write!(formatter, "read administrator password: {source}"),
            Self::Output(source) => write!(formatter, "write command result: {source}"),
            Self::Authentication(source) => write!(formatter, "administrator password: {source}"),
            Self::Database(source) => write!(formatter, "administrator database: {source}"),
        }
    }
}

impl std::error::Error for AdminCliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Input(source) | Self::Output(source) => Some(source),
            Self::Authentication(source) => Some(source),
            Self::Database(source) => Some(source),
        }
    }
}

impl From<AuthError> for AdminCliError {
    fn from(value: AuthError) -> Self {
        Self::Authentication(value)
    }
}

impl From<DatabaseError> for AdminCliError {
    fn from(value: DatabaseError) -> Self {
        Self::Database(value)
    }
}

pub async fn set_password_from_stdio(database_path: &Path) -> Result<(), AdminCliError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    set_password(database_path, stdin.lock(), stdout.lock()).await
}

async fn set_password(
    database_path: &Path,
    reader: impl BufRead,
    mut writer: impl Write,
) -> Result<(), AdminCliError> {
    let database = Database::open(database_path)?;
    let password = read_password_line(reader)?;
    validate_password(&password)?;
    let password_hash = hash_password(password).await?;
    database
        .set_admin_password(password_hash, unix_timestamp()?)
        .await?;
    writeln!(writer, "administrator password updated").map_err(AdminCliError::Output)?;
    database.shutdown().await?;
    Ok(())
}

fn read_password_line(reader: impl BufRead) -> Result<String, AdminCliError> {
    let mut line = String::new();
    reader
        .take(1_027)
        .read_line(&mut line)
        .map_err(AdminCliError::Input)?;
    while line.ends_with(['\r', '\n']) {
        line.pop();
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Cursor, path::PathBuf};

    use super::*;
    use crate::auth::verify_password;

    #[tokio::test]
    async fn cli_password_is_hashed_persisted_and_not_trimmed() {
        let path = test_database_path();
        remove_database_files(&path);
        let existing = Database::open(&path).expect("open existing database");
        let existing_hash = "x".repeat(32);
        existing
            .set_admin_password(existing_hash.clone(), 1)
            .await
            .expect("set existing password fixture");
        assert!(
            existing
                .create_session(existing_hash, [5_u8; 32], 1, 2)
                .await
                .expect("create existing session fixture")
        );
        existing.shutdown().await.expect("close existing database");

        let mut output = Vec::new();
        set_password(&path, Cursor::new(b" password \r\n"), &mut output)
            .await
            .expect("set administrator password");

        assert_eq!(output, b"administrator password updated\n");
        let database = Database::open(&path).expect("reopen database");
        let password_hash = database
            .load_admin_password_hash()
            .await
            .expect("load password hash")
            .expect("administrator exists");
        assert_ne!(password_hash, " password ");
        assert!(password_hash.starts_with("$argon2id$"));
        assert!(
            verify_password(" password ".into(), password_hash)
                .await
                .expect("verify password")
        );
        assert_eq!(
            database
                .find_session([5_u8; 32])
                .await
                .expect("find invalidated session"),
            None
        );
        database.shutdown().await.expect("shutdown database");
        remove_database_files(&path);
    }

    fn test_database_path() -> PathBuf {
        std::env::temp_dir().join(format!("monitor-admin-cli-{}.db", std::process::id()))
    }

    fn remove_database_files(path: &Path) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove test database: {error}"),
            }
        }
    }
}
