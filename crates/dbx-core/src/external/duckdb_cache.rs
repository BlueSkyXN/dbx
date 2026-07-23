use std::{collections::HashMap, sync::Arc, time::Duration};

use super::types::{
    CacheState, ExternalColumnDef, ExternalRowUpdate, ExternalTableRef, ExternalTableSnapshot, ExternalWriteResult,
};

/// Loads an external table snapshot into an in-memory DuckDB connection.
pub fn load_snapshot_to_duckdb(con: &duckdb::Connection, snapshot: &ExternalTableSnapshot) -> Result<(), String> {
    let table_name = sanitize_table_name(&snapshot.table_ref.table_name);
    load_snapshot_to_duckdb_as(con, snapshot, &table_name)
}

fn load_snapshot_to_duckdb_as(
    con: &duckdb::Connection,
    snapshot: &ExternalTableSnapshot,
    table_name: &str,
) -> Result<(), String> {
    con.execute(&format!("DROP TABLE IF EXISTS {}", quote_identifier(table_name)), [])
        .map_err(|e| format!("Failed to drop table: {e}"))?;

    if snapshot.columns.is_empty() {
        return Ok(());
    }

    let columns = normalized_columns(&snapshot.columns);
    let col_defs: Vec<String> = snapshot
        .columns
        .iter()
        .zip(columns.iter())
        .map(|c| {
            let (source_col, target_col) = c;
            format!(
                "{} {}{}",
                quote_identifier(&target_col.name),
                source_col.duckdb_type,
                if source_col.is_nullable { "" } else { " NOT NULL" }
            )
        })
        .collect();

    let pk_cols: Vec<String> = snapshot
        .columns
        .iter()
        .zip(columns.iter())
        .filter(|(source_col, _)| source_col.is_primary_key)
        .map(|(_, target_col)| target_col.name.clone())
        .collect();

    let mut create_sql = format!("CREATE TABLE {} ({}", quote_identifier(table_name), col_defs.join(", "));
    if !pk_cols.is_empty() {
        create_sql.push_str(&format!(
            ", PRIMARY KEY ({})",
            pk_cols.iter().map(|c| quote_identifier(c)).collect::<Vec<_>>().join(", ")
        ));
    }
    create_sql.push(')');

    con.execute(&create_sql, []).map_err(|e| format!("Failed to create table: {e}"))?;

    if snapshot.rows.is_empty() {
        return Ok(());
    }

    let column_count = columns.len();
    let placeholders: Vec<&str> = (0..column_count).map(|_| "?").collect();
    let insert_sql = format!("INSERT INTO {} VALUES ({})", quote_identifier(table_name), placeholders.join(", "));
    let mut stmt = con.prepare(&insert_sql).map_err(|e| format!("Failed to prepare insert: {e}"))?;

    for row in &snapshot.rows {
        let params: Vec<Box<dyn duckdb::ToSql>> = (0..column_count)
            .map(|i| {
                let val = row.get(i).unwrap_or(&serde_json::Value::Null);
                let col_type = snapshot.columns.get(i).map(|c| c.duckdb_type.as_str()).unwrap_or("VARCHAR");
                json_to_duckdb_param(val, col_type)
            })
            .collect();

        let param_refs: Vec<&dyn duckdb::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        stmt.execute(param_refs.as_slice()).map_err(|e| format!("Failed to insert row: {e}"))?;
    }

    Ok(())
}

fn normalized_columns(columns: &[ExternalColumnDef]) -> Vec<ExternalColumnDef> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let base = if column.name.trim().is_empty() {
                format!("column_{}", index + 1)
            } else {
                column.name.trim().to_string()
            };
            let key = base.to_lowercase();
            let count = seen.entry(key).or_insert(0);
            *count += 1;

            let mut normalized = column.clone();
            normalized.name = if *count == 1 { base } else { format!("{base}_{count}") };
            normalized
        })
        .collect()
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn json_to_duckdb_param(val: &serde_json::Value, col_type: &str) -> Box<dyn duckdb::ToSql> {
    match val {
        serde_json::Value::Null => Box::new(None::<String>),
        serde_json::Value::Bool(b) => Box::new(*b),
        serde_json::Value::Number(n) => {
            if col_type.contains("INT") {
                Box::new(n.as_i64().unwrap_or(0))
            } else if col_type.contains("DOUBLE") || col_type.contains("FLOAT") || col_type.contains("DECIMAL") {
                Box::new(n.as_f64().unwrap_or(0.0))
            } else {
                Box::new(n.to_string())
            }
        }
        serde_json::Value::String(s) => Box::new(s.clone()),
        _ => Box::new(val.to_string()),
    }
}

/// Sanitize a source table name into a stable DuckDB table identifier.
pub fn sanitize_table_name(name: &str) -> String {
    let sanitized: String = name.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();

    if sanitized.is_empty() {
        "_unnamed_".to_string()
    } else if sanitized.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{sanitized}")
    } else {
        sanitized
    }
}

const REALTIME_REFRESH_TTL: Duration = Duration::from_secs(5);

/// Manages a DuckDB cache for one external source.
pub struct ExternalPool {
    pub source: Arc<dyn super::traits::ExternalTabularSource>,
    pub cache: Arc<std::sync::Mutex<duckdb::Connection>>,
    pub cache_state: std::sync::Mutex<CacheState>,
    pub table_map: std::sync::Mutex<HashMap<String, ExternalTableRef>>,
    table_versions: std::sync::Mutex<HashMap<String, String>>,
    last_refresh_at: std::sync::Mutex<Option<std::time::Instant>>,
    refresh_lock: tokio::sync::Mutex<()>,
}

impl ExternalPool {
    pub fn new(
        source: Arc<dyn super::traits::ExternalTabularSource>,
        cache: Arc<std::sync::Mutex<duckdb::Connection>>,
    ) -> Self {
        Self {
            source,
            cache,
            cache_state: std::sync::Mutex::new(CacheState::Empty),
            table_map: std::sync::Mutex::new(HashMap::new()),
            table_versions: std::sync::Mutex::new(HashMap::new()),
            last_refresh_at: std::sync::Mutex::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn refresh_cache(&self) -> Result<(), String> {
        self.refresh_cache_forced(true).await
    }

    pub async fn refresh_cache_if_stale(&self) -> Result<(), String> {
        self.refresh_cache_forced(false).await
    }

    async fn refresh_cache_forced(&self, force: bool) -> Result<(), String> {
        let _guard = self.refresh_lock.lock().await;
        if !force && self.realtime_refresh_is_fresh()? {
            return Ok(());
        }

        {
            let mut state = self.cache_state.lock().map_err(|e| e.to_string())?;
            *state = CacheState::Loading;
        }

        let result = self.refresh_cache_inner(force).await;
        let mut state = self.cache_state.lock().map_err(|e| e.to_string())?;
        match &result {
            Ok(()) => {
                *state = CacheState::Fresh;
                *self.last_refresh_at.lock().map_err(|e| e.to_string())? = Some(std::time::Instant::now());
            }
            Err(err) => *state = CacheState::Error(err.clone()),
        }
        result
    }

    fn realtime_refresh_is_fresh(&self) -> Result<bool, String> {
        Ok(self
            .last_refresh_at
            .lock()
            .map_err(|e| e.to_string())?
            .is_some_and(|last_refresh_at| last_refresh_at.elapsed() < REALTIME_REFRESH_TTL))
    }

    pub fn refresh_before_query(&self) -> bool {
        self.source.refresh_before_query()
    }

    pub async fn append_rows(
        &self,
        table_name: &str,
        rows: Vec<Vec<serde_json::Value>>,
    ) -> Result<ExternalWriteResult, String> {
        let table_ref = self.resolve_table_ref(table_name)?;
        let result = self.source.append_rows(&table_ref, rows).await?;
        self.refresh_cache().await?;
        Ok(result)
    }

    pub async fn update_rows(
        &self,
        table_name: &str,
        updates: Vec<ExternalRowUpdate>,
    ) -> Result<ExternalWriteResult, String> {
        let table_ref = self.resolve_table_ref(table_name)?;
        let result = self.source.update_rows(&table_ref, updates).await?;
        self.refresh_cache().await?;
        Ok(result)
    }

    pub async fn delete_rows(&self, table_name: &str, row_ids: Vec<String>) -> Result<ExternalWriteResult, String> {
        let table_ref = self.resolve_table_ref(table_name)?;
        let result = self.source.delete_rows(&table_ref, row_ids).await?;
        self.refresh_cache().await?;
        Ok(result)
    }

    pub async fn write_range(
        &self,
        table_name: &str,
        range: &str,
        rows: Vec<Vec<serde_json::Value>>,
    ) -> Result<ExternalWriteResult, String> {
        let table_ref = self.resolve_table_ref(table_name)?;
        let result = self.source.write_range(&table_ref, range, rows).await?;
        self.refresh_cache().await?;
        Ok(result)
    }

    fn resolve_table_ref(&self, table_name: &str) -> Result<ExternalTableRef, String> {
        let table_map = self.table_map.lock().map_err(|e| e.to_string())?;
        if let Some(table_ref) = table_map.get(table_name) {
            return Ok(table_ref.clone());
        }

        table_map
            .values()
            .find(|table_ref| table_ref.table_name == table_name || table_ref.display_name == table_name)
            .cloned()
            .ok_or_else(|| format!("Unknown external table: {table_name}"))
    }

    async fn refresh_cache_inner(&self, force: bool) -> Result<(), String> {
        let tables = self.source.list_tables().await?;
        let current_versions = self.table_versions.lock().map_err(|e| e.to_string())?.clone();
        let current_table_map = self.table_map.lock().map_err(|e| e.to_string())?.clone();
        let mut new_table_map = HashMap::new();
        let mut new_versions = HashMap::new();
        let mut load_items = Vec::new();
        let mut keep_table_names = Vec::new();

        for table_ref in &tables {
            let mut sanitized_name = sanitize_table_name(&table_ref.table_name);

            if new_table_map.contains_key(&sanitized_name) {
                let base = sanitized_name.clone();
                let mut suffix = 2;
                loop {
                    sanitized_name = format!("{base}_{suffix}");
                    if !new_table_map.contains_key(&sanitized_name) {
                        break;
                    }
                    suffix += 1;
                }
            }

            let source_version = if force { None } else { Some(self.source.source_version(table_ref).await?) };
            let can_keep = source_version.as_ref().is_some_and(|source_version| {
                cacheable_source_version(source_version)
                    && current_table_map.get(&sanitized_name).is_some_and(|current| current == table_ref)
                    && current_versions.get(&sanitized_name).is_some_and(|version| version == source_version)
            });
            if can_keep {
                new_table_map.insert(sanitized_name.clone(), table_ref.clone());
                new_versions.insert(sanitized_name.clone(), source_version.unwrap_or_default());
                keep_table_names.push(sanitized_name);
                continue;
            }

            let snapshot = self.source.load_table(table_ref).await?;
            let has_columns = !snapshot.columns.is_empty();
            if has_columns {
                new_table_map.insert(sanitized_name.clone(), table_ref.clone());
                let version =
                    if let Some(source_version) = source_version.filter(|version| cacheable_source_version(version)) {
                        source_version
                    } else {
                        snapshot.source_version.clone()
                    };
                new_versions.insert(sanitized_name.clone(), version);
            }
            load_items.push((sanitized_name, snapshot));
        }

        let target_table_names: Vec<String> = new_table_map.keys().cloned().collect();
        let cache = self.cache.clone();
        tokio::task::spawn_blocking(move || {
            let con = cache.lock().map_err(|e| e.to_string())?;
            con.execute_batch("BEGIN TRANSACTION")
                .map_err(|e| format!("Failed to begin external cache refresh transaction: {e}"))?;

            let refresh_result: Result<(), String> = (|| {
                let existing_tables = external_cache_table_names(&con)?;
                for table_name in existing_tables {
                    if !target_table_names.contains(&table_name) {
                        con.execute(&format!("DROP TABLE IF EXISTS {}", quote_identifier(&table_name)), [])
                            .map_err(|e| format!("Failed to drop stale external cache table '{table_name}': {e}"))?;
                    }
                }

                for kept_table_name in &keep_table_names {
                    if !external_cache_table_exists(&con, kept_table_name)? {
                        return Err(format!("Cached external table '{kept_table_name}' is missing"));
                    }
                }

                load_items.iter().try_for_each(|(target_table_name, snapshot)| {
                    load_snapshot_to_duckdb_as(&con, snapshot, target_table_name)
                })
            })();

            match refresh_result {
                Ok(()) => con
                    .execute_batch("COMMIT")
                    .map_err(|e| format!("Failed to commit external cache refresh transaction: {e}")),
                Err(err) => {
                    if let Err(rollback_err) = con.execute_batch("ROLLBACK") {
                        return Err(format!("{err}; rollback failed: {rollback_err}"));
                    }
                    Err(err)
                }
            }
        })
        .await
        .map_err(|e| e.to_string())??;

        {
            let mut map = self.table_map.lock().map_err(|e| e.to_string())?;
            *map = new_table_map;
        }
        {
            let mut versions = self.table_versions.lock().map_err(|e| e.to_string())?;
            *versions = new_versions;
        }

        Ok(())
    }
}

fn external_cache_table_names(con: &duckdb::Connection) -> Result<Vec<String>, String> {
    let mut stmt = con
        .prepare(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'main' AND table_type = 'BASE TABLE' \
             ORDER BY table_name",
        )
        .map_err(|e| format!("Failed to list external cache tables: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Failed to query external cache tables: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("Failed to read external cache table name: {e}"))
}

fn external_cache_table_exists(con: &duckdb::Connection, table_name: &str) -> Result<bool, String> {
    let count: i64 = con
        .query_row(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_schema = 'main' AND table_type = 'BASE TABLE' AND table_name = ?",
            [table_name],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to check external cache table '{table_name}': {e}"))?;
    Ok(count > 0)
}

fn cacheable_source_version(version: &str) -> bool {
    let version = version.trim();
    !version.is_empty() && version != "unknown" && !version.ends_with(":unknown")
}

impl std::fmt::Debug for ExternalPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalPool")
            .field("source", &self.source.display_name())
            .field("cache_state", &self.cache_state)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn table_ref(name: &str) -> ExternalTableRef {
        ExternalTableRef { source_id: "test".to_string(), table_name: name.to_string(), display_name: name.to_string() }
    }

    fn column(name: &str, duckdb_type: &str) -> ExternalColumnDef {
        ExternalColumnDef {
            name: name.to_string(),
            duckdb_type: duckdb_type.to_string(),
            is_nullable: true,
            is_primary_key: false,
            comment: None,
        }
    }

    fn required_column(name: &str, duckdb_type: &str) -> ExternalColumnDef {
        ExternalColumnDef { is_nullable: false, ..column(name, duckdb_type) }
    }

    #[derive(Debug)]
    struct SnapshotSource {
        snapshots: Vec<ExternalTableSnapshot>,
    }

    #[derive(Debug, Clone)]
    struct MutableSnapshotSource {
        snapshots: Arc<std::sync::Mutex<Vec<ExternalTableSnapshot>>>,
    }

    #[derive(Debug, Clone)]
    struct VersionedSnapshotSource {
        snapshots: Arc<std::sync::Mutex<Vec<ExternalTableSnapshot>>>,
        versions: Arc<std::sync::Mutex<HashMap<String, String>>>,
        load_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl super::super::traits::ExternalTabularSource for SnapshotSource {
        fn capabilities(&self) -> super::super::types::ExternalCapabilities {
            super::super::types::ExternalCapabilities { can_read: true, supports_refresh: true, ..Default::default() }
        }

        async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, String> {
            Ok(self.snapshots.iter().map(|snapshot| snapshot.table_ref.clone()).collect())
        }

        async fn get_columns(&self, table: &ExternalTableRef) -> Result<Vec<ExternalColumnDef>, String> {
            self.snapshots
                .iter()
                .find(|snapshot| snapshot.table_ref == *table)
                .map(|snapshot| snapshot.columns.clone())
                .ok_or_else(|| format!("Unknown table {}", table.table_name))
        }

        async fn load_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSnapshot, String> {
            self.snapshots
                .iter()
                .find(|snapshot| snapshot.table_ref == *table)
                .cloned()
                .ok_or_else(|| format!("Unknown table {}", table.table_name))
        }

        async fn source_version(&self, table: &ExternalTableRef) -> Result<String, String> {
            Ok(format!("test:{}", table.table_name))
        }

        async fn test_connection(&self) -> Result<String, String> {
            Ok("Connection successful".to_string())
        }

        fn display_name(&self) -> String {
            "snapshot-source".to_string()
        }
    }

    #[async_trait]
    impl super::super::traits::ExternalTabularSource for MutableSnapshotSource {
        fn capabilities(&self) -> super::super::types::ExternalCapabilities {
            super::super::types::ExternalCapabilities { can_read: true, supports_refresh: true, ..Default::default() }
        }

        async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, String> {
            let snapshots = self.snapshots.lock().map_err(|e| e.to_string())?;
            Ok(snapshots.iter().map(|snapshot| snapshot.table_ref.clone()).collect())
        }

        async fn get_columns(&self, table: &ExternalTableRef) -> Result<Vec<ExternalColumnDef>, String> {
            let snapshots = self.snapshots.lock().map_err(|e| e.to_string())?;
            snapshots
                .iter()
                .find(|snapshot| snapshot.table_ref == *table)
                .map(|snapshot| snapshot.columns.clone())
                .ok_or_else(|| format!("Unknown table {}", table.table_name))
        }

        async fn load_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSnapshot, String> {
            let snapshots = self.snapshots.lock().map_err(|e| e.to_string())?;
            snapshots
                .iter()
                .find(|snapshot| snapshot.table_ref == *table)
                .cloned()
                .ok_or_else(|| format!("Unknown table {}", table.table_name))
        }

        async fn source_version(&self, table: &ExternalTableRef) -> Result<String, String> {
            Ok(format!("test:{}", table.table_name))
        }

        async fn test_connection(&self) -> Result<String, String> {
            Ok("Connection successful".to_string())
        }

        fn display_name(&self) -> String {
            "mutable-snapshot-source".to_string()
        }
    }

    #[async_trait]
    impl super::super::traits::ExternalTabularSource for VersionedSnapshotSource {
        fn capabilities(&self) -> super::super::types::ExternalCapabilities {
            super::super::types::ExternalCapabilities { can_read: true, supports_refresh: true, ..Default::default() }
        }

        async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, String> {
            let snapshots = self.snapshots.lock().map_err(|e| e.to_string())?;
            Ok(snapshots.iter().map(|snapshot| snapshot.table_ref.clone()).collect())
        }

        async fn get_columns(&self, table: &ExternalTableRef) -> Result<Vec<ExternalColumnDef>, String> {
            let snapshots = self.snapshots.lock().map_err(|e| e.to_string())?;
            snapshots
                .iter()
                .find(|snapshot| snapshot.table_ref == *table)
                .map(|snapshot| snapshot.columns.clone())
                .ok_or_else(|| format!("Unknown table {}", table.table_name))
        }

        async fn load_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSnapshot, String> {
            self.load_count.fetch_add(1, Ordering::SeqCst);
            let snapshots = self.snapshots.lock().map_err(|e| e.to_string())?;
            snapshots
                .iter()
                .find(|snapshot| snapshot.table_ref == *table)
                .cloned()
                .ok_or_else(|| format!("Unknown table {}", table.table_name))
        }

        async fn source_version(&self, table: &ExternalTableRef) -> Result<String, String> {
            let versions = self.versions.lock().map_err(|e| e.to_string())?;
            Ok(versions.get(&table.table_name).cloned().unwrap_or_else(|| "unknown".to_string()))
        }

        async fn test_connection(&self) -> Result<String, String> {
            Ok("Connection successful".to_string())
        }

        fn display_name(&self) -> String {
            "versioned-snapshot-source".to_string()
        }
    }

    fn snapshot(
        name: &str,
        columns: Vec<ExternalColumnDef>,
        rows: Vec<Vec<serde_json::Value>>,
    ) -> ExternalTableSnapshot {
        ExternalTableSnapshot { table_ref: table_ref(name), columns, rows, source_version: "test".to_string() }
    }

    fn snapshot_with_version(
        name: &str,
        columns: Vec<ExternalColumnDef>,
        rows: Vec<Vec<serde_json::Value>>,
        source_version: &str,
    ) -> ExternalTableSnapshot {
        ExternalTableSnapshot { source_version: source_version.to_string(), ..snapshot(name, columns, rows) }
    }

    #[test]
    fn snapshot_load_normalizes_duplicate_columns_and_row_width() {
        let con = duckdb::Connection::open_in_memory().unwrap();
        let snapshot = snapshot(
            "orders",
            vec![column("id", "BIGINT"), column("id", "BIGINT"), column("", "VARCHAR")],
            vec![
                vec![serde_json::json!(1)],
                vec![serde_json::json!(2), serde_json::json!(3), serde_json::json!("ok"), serde_json::json!("ignored")],
            ],
        );

        load_snapshot_to_duckdb(&con, &snapshot).unwrap();

        let rows: Vec<(i64, Option<i64>, Option<String>)> = con
            .prepare("SELECT \"id\", \"id_2\", \"column_3\" FROM \"orders\" ORDER BY \"id\"")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows, vec![(1, None, None), (2, Some(3), Some("ok".to_string()))]);
    }

    #[test]
    fn snapshot_with_no_columns_drops_existing_cache_table() {
        let con = duckdb::Connection::open_in_memory().unwrap();
        con.execute("CREATE TABLE \"empty_sheet\" (id BIGINT)", []).unwrap();

        let snapshot = snapshot("empty_sheet", vec![], vec![]);
        load_snapshot_to_duckdb(&con, &snapshot).unwrap();

        let count: i64 = con
            .query_row(
                "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'main' AND table_name = 'empty_sheet'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn refresh_cache_rolls_back_partial_loads_on_failure() {
        let cache = Arc::new(std::sync::Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
        {
            let con = cache.lock().unwrap();
            con.execute("CREATE TABLE \"existing\" (id BIGINT)", []).unwrap();
            con.execute("INSERT INTO \"existing\" VALUES (42)", []).unwrap();
        }
        let source = Arc::new(SnapshotSource {
            snapshots: vec![
                snapshot("valid", vec![column("id", "BIGINT")], vec![vec![serde_json::json!(1)]]),
                snapshot("invalid", vec![required_column("id", "BIGINT")], vec![vec![serde_json::Value::Null]]),
            ],
        });
        let pool = ExternalPool::new(source, cache.clone());

        let err = pool.refresh_cache().await.unwrap_err();
        assert!(err.contains("Failed to insert row"));
        assert!(matches!(*pool.cache_state.lock().unwrap(), CacheState::Error(_)));
        assert!(pool.table_map.lock().unwrap().is_empty());

        let con = cache.lock().unwrap();
        let count: i64 = con
            .query_row(
                "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'main' AND table_name = 'valid'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let existing_value: i64 = con.query_row("SELECT id FROM \"existing\"", [], |row| row.get(0)).unwrap();
        assert_eq!(existing_value, 42);
    }

    #[tokio::test]
    async fn refresh_cache_drops_tables_removed_from_source() {
        let cache = Arc::new(std::sync::Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
        let snapshots = Arc::new(std::sync::Mutex::new(vec![
            snapshot("active_sheet", vec![column("id", "BIGINT")], vec![vec![serde_json::json!(1)]]),
            snapshot("deleted_sheet", vec![column("id", "BIGINT")], vec![vec![serde_json::json!(2)]]),
        ]));
        let pool = ExternalPool::new(Arc::new(MutableSnapshotSource { snapshots: snapshots.clone() }), cache.clone());

        pool.refresh_cache().await.unwrap();
        {
            let con = cache.lock().unwrap();
            let tables = external_cache_table_names(&con).unwrap();
            assert_eq!(tables.len(), 2);
            assert!(tables.contains(&"active_sheet".to_string()));
            assert!(tables.contains(&"deleted_sheet".to_string()));
        }

        *snapshots.lock().unwrap() =
            vec![snapshot("active_sheet", vec![column("id", "BIGINT")], vec![vec![serde_json::json!(3)]])];
        pool.refresh_cache().await.unwrap();

        let con = cache.lock().unwrap();
        let tables = external_cache_table_names(&con).unwrap();
        assert_eq!(tables, vec!["active_sheet".to_string()]);
        let value: i64 = con.query_row("SELECT id FROM \"active_sheet\"", [], |row| row.get(0)).unwrap();
        assert_eq!(value, 3);
    }

    #[tokio::test]
    async fn refresh_cache_keeps_previous_tables_on_failed_reload() {
        let cache = Arc::new(std::sync::Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
        let snapshots = Arc::new(std::sync::Mutex::new(vec![snapshot(
            "stable_sheet",
            vec![column("id", "BIGINT")],
            vec![vec![serde_json::json!(1)]],
        )]));
        let pool = ExternalPool::new(Arc::new(MutableSnapshotSource { snapshots: snapshots.clone() }), cache.clone());

        pool.refresh_cache().await.unwrap();
        *snapshots.lock().unwrap() =
            vec![snapshot("stable_sheet", vec![required_column("id", "BIGINT")], vec![vec![serde_json::Value::Null]])];

        let err = pool.refresh_cache().await.unwrap_err();
        assert!(err.contains("Failed to insert row"));

        let con = cache.lock().unwrap();
        let tables = external_cache_table_names(&con).unwrap();
        assert_eq!(tables, vec!["stable_sheet".to_string()]);
        let value: i64 = con.query_row("SELECT id FROM \"stable_sheet\"", [], |row| row.get(0)).unwrap();
        assert_eq!(value, 1);
    }

    #[tokio::test]
    async fn refresh_cache_if_stale_uses_ttl_and_skips_unchanged_source_versions() {
        let cache = Arc::new(std::sync::Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
        let snapshots = Arc::new(std::sync::Mutex::new(vec![snapshot_with_version(
            "versioned_sheet",
            vec![column("id", "BIGINT")],
            vec![vec![serde_json::json!(1)]],
            "v1",
        )]));
        let versions =
            Arc::new(std::sync::Mutex::new(HashMap::from([("versioned_sheet".to_string(), "v1".to_string())])));
        let load_count = Arc::new(AtomicUsize::new(0));
        let source = VersionedSnapshotSource {
            snapshots: snapshots.clone(),
            versions: versions.clone(),
            load_count: load_count.clone(),
        };
        let pool = ExternalPool::new(Arc::new(source), cache.clone());

        pool.refresh_cache().await.unwrap();
        assert_eq!(load_count.load(Ordering::SeqCst), 1);

        pool.refresh_cache_if_stale().await.unwrap();
        assert_eq!(load_count.load(Ordering::SeqCst), 1);

        *pool.last_refresh_at.lock().unwrap() =
            Some(std::time::Instant::now() - REALTIME_REFRESH_TTL - Duration::from_secs(1));
        pool.refresh_cache_if_stale().await.unwrap();
        assert_eq!(load_count.load(Ordering::SeqCst), 1);

        *snapshots.lock().unwrap() = vec![snapshot_with_version(
            "versioned_sheet",
            vec![column("id", "BIGINT")],
            vec![vec![serde_json::json!(2)]],
            "v2",
        )];
        versions.lock().unwrap().insert("versioned_sheet".to_string(), "v2".to_string());
        *pool.last_refresh_at.lock().unwrap() =
            Some(std::time::Instant::now() - REALTIME_REFRESH_TTL - Duration::from_secs(1));
        pool.refresh_cache_if_stale().await.unwrap();
        assert_eq!(load_count.load(Ordering::SeqCst), 2);

        let con = cache.lock().unwrap();
        let value: i64 = con.query_row("SELECT id FROM \"versioned_sheet\"", [], |row| row.get(0)).unwrap();
        assert_eq!(value, 2);
    }

    #[tokio::test]
    async fn refresh_cache_if_stale_does_not_keep_cache_for_different_table_ref() {
        let cache = Arc::new(std::sync::Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
        let first_ref = ExternalTableRef {
            source_id: "first".to_string(),
            table_name: "same_name".to_string(),
            display_name: "same_name".to_string(),
        };
        let second_ref = ExternalTableRef {
            source_id: "second".to_string(),
            table_name: "same_name".to_string(),
            display_name: "same_name".to_string(),
        };
        let snapshots = Arc::new(std::sync::Mutex::new(vec![ExternalTableSnapshot {
            table_ref: first_ref,
            columns: vec![column("id", "BIGINT")],
            rows: vec![vec![serde_json::json!(1)]],
            source_version: "v1".to_string(),
        }]));
        let versions = Arc::new(std::sync::Mutex::new(HashMap::from([("same_name".to_string(), "v1".to_string())])));
        let load_count = Arc::new(AtomicUsize::new(0));
        let source = VersionedSnapshotSource {
            snapshots: snapshots.clone(),
            versions: versions.clone(),
            load_count: load_count.clone(),
        };
        let pool = ExternalPool::new(Arc::new(source), cache.clone());

        pool.refresh_cache().await.unwrap();

        *snapshots.lock().unwrap() = vec![ExternalTableSnapshot {
            table_ref: second_ref,
            columns: vec![column("id", "BIGINT")],
            rows: vec![vec![serde_json::json!(2)]],
            source_version: "v1".to_string(),
        }];
        *pool.last_refresh_at.lock().unwrap() =
            Some(std::time::Instant::now() - REALTIME_REFRESH_TTL - Duration::from_secs(1));
        pool.refresh_cache_if_stale().await.unwrap();

        assert_eq!(load_count.load(Ordering::SeqCst), 2);
        let con = cache.lock().unwrap();
        let value: i64 = con.query_row("SELECT id FROM \"same_name\"", [], |row| row.get(0)).unwrap();
        assert_eq!(value, 2);
    }

    #[tokio::test]
    async fn refresh_cache_if_stale_coalesces_concurrent_refreshes() {
        let cache = Arc::new(std::sync::Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
        let snapshots = Arc::new(std::sync::Mutex::new(vec![snapshot_with_version(
            "live_sheet",
            vec![column("id", "BIGINT")],
            vec![vec![serde_json::json!(1)]],
            "v1",
        )]));
        let versions = Arc::new(std::sync::Mutex::new(HashMap::from([("live_sheet".to_string(), "v1".to_string())])));
        let load_count = Arc::new(AtomicUsize::new(0));
        let source = VersionedSnapshotSource { snapshots, versions, load_count: load_count.clone() };
        let pool = Arc::new(ExternalPool::new(Arc::new(source), cache));

        let tasks = (0..5)
            .map(|_| {
                let pool = pool.clone();
                tokio::spawn(async move { pool.refresh_cache_if_stale().await.unwrap() })
            })
            .collect::<Vec<_>>();
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(load_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_cache_force_ignores_realtime_ttl() {
        let cache = Arc::new(std::sync::Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
        let snapshots = Arc::new(std::sync::Mutex::new(vec![snapshot_with_version(
            "forced_sheet",
            vec![column("id", "BIGINT")],
            vec![vec![serde_json::json!(1)]],
            "v1",
        )]));
        let versions = Arc::new(std::sync::Mutex::new(HashMap::from([("forced_sheet".to_string(), "v1".to_string())])));
        let load_count = Arc::new(AtomicUsize::new(0));
        let source = VersionedSnapshotSource { snapshots, versions, load_count: load_count.clone() };
        let pool = ExternalPool::new(Arc::new(source), cache);

        pool.refresh_cache().await.unwrap();
        pool.refresh_cache().await.unwrap();

        assert_eq!(load_count.load(Ordering::SeqCst), 2);
    }
}
