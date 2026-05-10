use std::sync::Arc;

use super::types::{CacheState, ExternalColumnDef, ExternalTableRef, ExternalTableSnapshot};

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
                &source_col.duckdb_type,
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

/// Manages a DuckDB cache for one external source.
pub struct ExternalPool {
    pub source: Arc<dyn super::traits::ExternalTabularSource>,
    pub cache: Arc<std::sync::Mutex<duckdb::Connection>>,
    pub cache_state: std::sync::Mutex<CacheState>,
    pub table_map: std::sync::Mutex<std::collections::HashMap<String, ExternalTableRef>>,
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
            table_map: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub async fn refresh_cache(&self) -> Result<(), String> {
        {
            let mut state = self.cache_state.lock().map_err(|e| e.to_string())?;
            *state = CacheState::Loading;
        }

        let result = self.refresh_cache_inner().await;
        let mut state = self.cache_state.lock().map_err(|e| e.to_string())?;
        match &result {
            Ok(()) => *state = CacheState::Fresh,
            Err(err) => *state = CacheState::Error(err.clone()),
        }
        result
    }

    async fn refresh_cache_inner(&self) -> Result<(), String> {
        let tables = self.source.list_tables().await?;
        let mut new_table_map = std::collections::HashMap::new();
        let mut load_items = Vec::new();

        for table_ref in &tables {
            let snapshot = self.source.load_table(table_ref).await?;
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

            let has_columns = !snapshot.columns.is_empty();
            if has_columns {
                new_table_map.insert(sanitized_name.clone(), table_ref.clone());
            }
            load_items.push((sanitized_name, snapshot));
        }

        let cache = self.cache.clone();
        tokio::task::spawn_blocking(move || {
            let con = cache.lock().map_err(|e| e.to_string())?;
            con.execute_batch("BEGIN TRANSACTION")
                .map_err(|e| format!("Failed to begin external cache refresh transaction: {e}"))?;

            let load_result = load_items.iter().try_for_each(|(target_table_name, snapshot)| {
                load_snapshot_to_duckdb_as(&con, snapshot, target_table_name)
            });

            match load_result {
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

        Ok(())
    }
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

    fn snapshot(
        name: &str,
        columns: Vec<ExternalColumnDef>,
        rows: Vec<Vec<serde_json::Value>>,
    ) -> ExternalTableSnapshot {
        ExternalTableSnapshot { table_ref: table_ref(name), columns, rows, source_version: "test".to_string() }
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
    }
}
