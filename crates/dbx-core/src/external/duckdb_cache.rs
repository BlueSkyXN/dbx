#![cfg(feature = "duckdb-sidecar")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::db::duckdb_worker_process::DuckDbWorkerClient;
use crate::db::duckdb_worker_protocol::DuckDbWorkerError;

use super::traits::ExternalTabularSource;
use super::types::{
    CacheState, ExternalColumnDef, ExternalRowUpdate, ExternalTableRef, ExternalTableSnapshot, ExternalWriteResult,
};

const REALTIME_REFRESH_TTL: Duration = Duration::from_secs(5);
const INSERT_BATCH_ROWS: usize = 100;

/// Manages the isolated DuckDB worker used as a query cache for one external source.
pub struct ExternalPool {
    source: Arc<dyn ExternalTabularSource>,
    worker: Arc<DuckDbWorkerClient>,
    cache_state: Mutex<CacheState>,
    table_map: Mutex<HashMap<String, ExternalTableRef>>,
    last_refresh_at: Mutex<Option<Instant>>,
    refresh_lock: AsyncMutex<()>,
    operation_lock: AsyncMutex<()>,
}

impl ExternalPool {
    pub fn new(source: Arc<dyn ExternalTabularSource>, worker: Arc<DuckDbWorkerClient>) -> Self {
        Self {
            source,
            worker,
            cache_state: Mutex::new(CacheState::Empty),
            table_map: Mutex::new(HashMap::new()),
            last_refresh_at: Mutex::new(None),
            refresh_lock: AsyncMutex::new(()),
            operation_lock: AsyncMutex::new(()),
        }
    }

    pub async fn refresh_cache(&self) -> Result<(), String> {
        self.refresh_cache_forced(true).await
    }

    pub async fn refresh_cache_if_stale(&self) -> Result<(), String> {
        self.refresh_cache_forced(false).await
    }

    async fn refresh_cache_forced(&self, force: bool) -> Result<(), String> {
        let _refresh_guard = self.refresh_lock.lock().await;
        if !force && self.realtime_refresh_is_fresh()? {
            return Ok(());
        }

        *self.cache_state.lock().map_err(|error| error.to_string())? = CacheState::Loading;
        let result = self.refresh_cache_inner().await;
        let mut state = self.cache_state.lock().map_err(|error| error.to_string())?;
        match &result {
            Ok(()) => {
                *state = CacheState::Fresh;
                *self.last_refresh_at.lock().map_err(|error| error.to_string())? = Some(Instant::now());
            }
            Err(error) => *state = CacheState::Error(error.clone()),
        }
        result
    }

    fn realtime_refresh_is_fresh(&self) -> Result<bool, String> {
        if !matches!(&*self.cache_state.lock().map_err(|error| error.to_string())?, CacheState::Fresh) {
            return Ok(false);
        }
        Ok(self
            .last_refresh_at
            .lock()
            .map_err(|error| error.to_string())?
            .is_some_and(|last_refresh_at| last_refresh_at.elapsed() < REALTIME_REFRESH_TTL))
    }

    pub async fn execute_typed(
        &self,
        database: Option<String>,
        sql: String,
        max_rows: Option<usize>,
        cancel_token: Option<CancellationToken>,
        query_timeout: Option<Duration>,
    ) -> Result<db::QueryResult, DuckDbWorkerError> {
        self.prepare_read().await.map_err(DuckDbWorkerError::from)?;
        let _operation_guard = self.operation_lock.lock().await;
        self.worker.execute_typed(database, sql, max_rows, cancel_token, query_timeout).await
    }

    pub fn worker(&self) -> Arc<DuckDbWorkerClient> {
        self.worker.clone()
    }

    async fn prepare_read(&self) -> Result<(), String> {
        if self.cache_refresh_required()? {
            self.refresh_cache_if_stale().await
        } else if self.source.refresh_before_query() {
            self.refresh_cache_if_stale().await
        } else {
            Ok(())
        }
    }

    pub async fn list_databases(&self) -> Result<Vec<db::DatabaseInfo>, String> {
        self.prepare_read().await?;
        let _operation_guard = self.operation_lock.lock().await;
        self.worker.list_databases().await
    }

    pub async fn list_schemas(&self, database: String) -> Result<Vec<String>, String> {
        self.prepare_read().await?;
        let _operation_guard = self.operation_lock.lock().await;
        self.worker.list_schemas(database).await
    }

    pub async fn list_tables(&self, database: String, schema: String) -> Result<Vec<db::TableInfo>, String> {
        self.prepare_read().await?;
        let _operation_guard = self.operation_lock.lock().await;
        self.worker.list_tables(database, schema).await
    }

    pub async fn list_columns(
        &self,
        database: String,
        schema: String,
        table: String,
    ) -> Result<Vec<db::ColumnInfo>, String> {
        self.prepare_read().await?;
        let _operation_guard = self.operation_lock.lock().await;
        self.worker.list_columns(database, schema, table).await
    }

    pub async fn get_table_ddl(&self, database: String, schema: String, table: String) -> Result<String, String> {
        self.prepare_read().await?;
        let _operation_guard = self.operation_lock.lock().await;
        self.worker.get_table_ddl(database, schema, table).await
    }

    pub async fn completion_assistant(
        &self,
        request: db::CompletionAssistantRequest,
    ) -> Result<db::CompletionAssistantResponse, String> {
        self.prepare_read().await?;
        let _operation_guard = self.operation_lock.lock().await;
        self.worker.completion_assistant(request).await
    }

    pub async fn get_object_source(
        &self,
        database: String,
        schema: String,
        name: String,
        object_type: db::ObjectSourceKind,
    ) -> Result<String, String> {
        self.prepare_read().await?;
        let _operation_guard = self.operation_lock.lock().await;
        self.worker.get_object_source(database, schema, name, object_type).await
    }

    fn cache_refresh_required(&self) -> Result<bool, String> {
        Ok(matches!(
            &*self.cache_state.lock().map_err(|error| error.to_string())?,
            CacheState::Loading | CacheState::Error(_)
        ))
    }

    async fn refresh_after_write(&self) {
        // The remote mutation has already committed. Preserve its successful
        // result even if cache reload fails so callers do not retry an append
        // and create duplicates. The Error state forces the next query to
        // retry the refresh before reading from the cache.
        let _ = self.refresh_cache().await;
    }

    pub async fn append_rows(
        &self,
        table_name: &str,
        rows: Vec<Vec<serde_json::Value>>,
    ) -> Result<ExternalWriteResult, String> {
        let table_ref = self.resolve_table_ref(table_name)?;
        let result = self.source.append_rows(&table_ref, rows).await?;
        self.refresh_after_write().await;
        Ok(result)
    }

    pub async fn update_rows(
        &self,
        table_name: &str,
        updates: Vec<ExternalRowUpdate>,
    ) -> Result<ExternalWriteResult, String> {
        let table_ref = self.resolve_table_ref(table_name)?;
        let result = self.source.update_rows(&table_ref, updates).await?;
        self.refresh_after_write().await;
        Ok(result)
    }

    pub async fn delete_rows(&self, table_name: &str, row_ids: Vec<String>) -> Result<ExternalWriteResult, String> {
        let table_ref = self.resolve_table_ref(table_name)?;
        let result = self.source.delete_rows(&table_ref, row_ids).await?;
        self.refresh_after_write().await;
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
        self.refresh_after_write().await;
        Ok(result)
    }

    fn resolve_table_ref(&self, table_name: &str) -> Result<ExternalTableRef, String> {
        let table_map = self.table_map.lock().map_err(|error| error.to_string())?;
        if let Some(table_ref) = table_map.get(table_name) {
            return Ok(table_ref.clone());
        }
        table_map
            .values()
            .find(|table_ref| table_ref.table_name == table_name || table_ref.display_name == table_name)
            .cloned()
            .ok_or_else(|| format!("Unknown external table: {table_name}"))
    }

    async fn refresh_cache_inner(&self) -> Result<(), String> {
        let tables = self.source.list_tables().await?;
        let mut new_table_map = HashMap::new();
        let mut snapshots = Vec::new();

        for table_ref in tables {
            let table_name = unique_table_name(&table_ref.table_name, &new_table_map);
            let snapshot = self.source.load_table(&table_ref).await?;
            if !snapshot.columns.is_empty() {
                new_table_map.insert(table_name.clone(), table_ref);
            }
            snapshots.push((table_name, snapshot));
        }

        let previous_table_names =
            self.table_map.lock().map_err(|error| error.to_string())?.keys().cloned().collect::<Vec<_>>();
        let _operation_guard = self.operation_lock.lock().await;
        self.execute_cache_sql("BEGIN TRANSACTION").await?;

        let refresh_result = async {
            for table_name in previous_table_names {
                self.execute_cache_sql(&format!("DROP TABLE IF EXISTS {}", quote_identifier(&table_name))).await?;
            }
            for (table_name, snapshot) in &snapshots {
                self.load_snapshot(table_name, snapshot).await?;
            }
            Ok::<(), String>(())
        }
        .await;

        match refresh_result {
            Ok(()) => self.execute_cache_sql("COMMIT").await?,
            Err(error) => {
                let rollback = self.execute_cache_sql("ROLLBACK").await;
                return match rollback {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!("{error}; rollback failed: {rollback_error}")),
                };
            }
        }

        *self.table_map.lock().map_err(|error| error.to_string())? = new_table_map;
        Ok(())
    }

    async fn load_snapshot(&self, table_name: &str, snapshot: &ExternalTableSnapshot) -> Result<(), String> {
        self.execute_cache_sql(&format!("DROP TABLE IF EXISTS {}", quote_identifier(table_name))).await?;
        if snapshot.columns.is_empty() {
            return Ok(());
        }

        let columns = normalized_columns(&snapshot.columns);
        let mut definitions = snapshot
            .columns
            .iter()
            .zip(&columns)
            .map(|(source, target)| {
                format!(
                    "{} {}{}",
                    quote_identifier(&target.name),
                    source.duckdb_type,
                    if source.is_nullable { "" } else { " NOT NULL" }
                )
            })
            .collect::<Vec<_>>();
        let primary_keys = snapshot
            .columns
            .iter()
            .zip(&columns)
            .filter(|(source, _)| source.is_primary_key)
            .map(|(_, target)| quote_identifier(&target.name))
            .collect::<Vec<_>>();
        if !primary_keys.is_empty() {
            definitions.push(format!("PRIMARY KEY ({})", primary_keys.join(", ")));
        }
        self.execute_cache_sql(&format!("CREATE TABLE {} ({})", quote_identifier(table_name), definitions.join(", ")))
            .await?;

        for rows in snapshot.rows.chunks(INSERT_BATCH_ROWS) {
            let values = rows
                .iter()
                .map(|row| {
                    let values = (0..columns.len())
                        .map(|index| sql_literal(row.get(index).unwrap_or(&serde_json::Value::Null)))
                        .collect::<Vec<_>>();
                    format!("({})", values.join(", "))
                })
                .collect::<Vec<_>>();
            self.execute_cache_sql(&format!(
                "INSERT INTO {} VALUES {}",
                quote_identifier(table_name),
                values.join(", ")
            ))
            .await?;
        }
        Ok(())
    }

    async fn execute_cache_sql(&self, sql: &str) -> Result<(), String> {
        self.worker.execute(None, sql.to_string(), Some(1), None, None).await.map(|_| ())
    }
}

impl std::fmt::Debug for ExternalPool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ExternalPool").field("cache_state", &self.cache_state).finish_non_exhaustive()
    }
}

fn unique_table_name(name: &str, existing: &HashMap<String, ExternalTableRef>) -> String {
    let base = sanitize_table_name(name);
    if !existing.contains_key(&base) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}_{suffix}");
        if !existing.contains_key(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn normalized_columns(columns: &[ExternalColumnDef]) -> Vec<ExternalColumnDef> {
    let mut seen = HashMap::<String, usize>::new();
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let base = if column.name.trim().is_empty() {
                format!("column_{}", index + 1)
            } else {
                column.name.trim().to_string()
            };
            let count = seen.entry(base.to_lowercase()).or_insert(0);
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

fn sql_literal(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(value) => if *value { "TRUE" } else { "FALSE" }.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => format!("'{}'", value.replace('\'', "''")),
        value => format!("'{}'", value.to_string().replace('\'', "''")),
    }
}

/// Sanitize a source table name into a stable DuckDB table identifier.
pub fn sanitize_table_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| if character.is_alphanumeric() || character == '_' { character } else { '_' })
        .collect::<String>();
    if sanitized.is_empty() {
        "_unnamed_".to_string()
    } else if sanitized.chars().next().is_some_and(|character| character.is_ascii_digit()) {
        format!("_{sanitized}")
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_literals_escape_strings_and_preserve_scalars() {
        assert_eq!(sql_literal(&serde_json::json!("O'Reilly")), "'O''Reilly'");
        assert_eq!(sql_literal(&serde_json::json!(42)), "42");
        assert_eq!(sql_literal(&serde_json::Value::Null), "NULL");
    }

    #[test]
    fn table_names_are_stable_and_safe() {
        assert_eq!(sanitize_table_name("Quarter 1 / Sales"), "Quarter_1___Sales");
        assert_eq!(sanitize_table_name("123"), "_123");
    }
}
