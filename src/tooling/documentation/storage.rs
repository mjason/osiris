use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use libsql::{Builder, Connection};

use super::{DATABASE_BYTES, DATABASE_HASH, DATABASE_SCHEMA_VERSION, SNAPSHOT_HASH};

static NEXT_MATERIALIZATION: AtomicU64 = AtomicU64::new(0);

pub(super) async fn embedded_connection() -> Result<Connection, String> {
    let actual = crate::hash::sha256(DATABASE_BYTES);
    let expected = format!("sha256:{DATABASE_HASH}");
    if actual != expected {
        return Err("embedded documentation database failed content validation".to_owned());
    }
    let path = materialize_database(&actual)?;
    if actual != crate::hash::sha256(&fs::read(&path).map_err(|error| error.to_string())?) {
        return Err("materialized documentation snapshot failed hash validation".to_owned());
    }
    let database = Builder::new_local(path)
        .build()
        .await
        .map_err(|error| format!("could not open embedded libSQL snapshot: {error}"))?;
    let connection = database.connect().map_err(|error| error.to_string())?;
    connection
        .execute("PRAGMA query_only = ON", ())
        .await
        .map_err(|error| format!("could not make documentation connection read-only: {error}"))?;
    let mut metadata = connection
        .query(
            "SELECT key, value FROM metadata WHERE key IN ('schema_version', 'snapshot_hash')",
            (),
        )
        .await
        .map_err(|error| error.to_string())?;
    let mut values = std::collections::BTreeMap::new();
    while let Some(row) = metadata.next().await.map_err(|error| error.to_string())? {
        values.insert(
            row.get::<String>(0).map_err(|error| error.to_string())?,
            row.get::<String>(1).map_err(|error| error.to_string())?,
        );
    }
    if values.get("schema_version").map(String::as_str) != Some(DATABASE_SCHEMA_VERSION) {
        return Err("documentation database schema is unsupported".to_owned());
    }
    if values.get("snapshot_hash").map(String::as_str) != Some(SNAPSHOT_HASH) {
        return Err("documentation snapshot identity does not match the executable".to_owned());
    }
    Ok(connection)
}

fn materialize_database(hash: &str) -> Result<PathBuf, String> {
    let name = format!("osiris-docs-{}.sqlite3", hash.trim_start_matches("sha256:"));
    let path = std::env::temp_dir().join(name);
    if path.is_file() {
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        if crate::hash::sha256(&bytes) == hash {
            make_read_only(&path)?;
            return Ok(path);
        }
    }
    let temporary = loop {
        let nonce = NEXT_MATERIALIZATION.fetch_add(1, Ordering::Relaxed);
        let candidate = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(DATABASE_BYTES)
                    .and_then(|()| file.sync_all())
                    .map_err(|error| error.to_string())?;
                make_read_only(&candidate)?;
                break candidate;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    };
    if let Err(error) = fs::rename(&temporary, &path) {
        let winner_is_valid = path.is_file()
            && fs::read(&path).is_ok_and(|bytes| crate::hash::sha256(&bytes) == hash);
        let _ = fs::remove_file(&temporary);
        if !winner_is_valid {
            return Err(error.to_string());
        }
    }
    Ok(path)
}

fn make_read_only(path: &std::path::Path) -> Result<(), String> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    if !permissions.readonly() {
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions).map_err(|error| error.to_string())?;
    }
    Ok(())
}
