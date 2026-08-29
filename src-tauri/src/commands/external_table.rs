use std::sync::Arc;

use dbx_core::external::{
    ApplyChangesRequest, ApplyChangesResult, ExternalTableRef, ExternalTableSchema, PageSnapshot, ReadPageRequest,
};
use tauri::State;

use super::connection::{ensure_connection_writable, AppState};

#[tauri::command]
pub async fn external_table_list(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
) -> Result<Vec<ExternalTableRef>, String> {
    state.external_tables.get(&connection_id).await.map_err(String::from)?.list_tables().await.map_err(String::from)
}

#[tauri::command]
pub async fn external_table_describe(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    table: ExternalTableRef,
) -> Result<ExternalTableSchema, String> {
    state
        .external_tables
        .get(&connection_id)
        .await
        .map_err(String::from)?
        .describe_table(&table)
        .await
        .map_err(String::from)
}

#[tauri::command]
pub async fn external_table_read_page(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    request: ReadPageRequest,
) -> Result<PageSnapshot, String> {
    state
        .external_tables
        .get(&connection_id)
        .await
        .map_err(String::from)?
        .read_page(request)
        .await
        .map_err(String::from)
}

#[tauri::command]
pub async fn external_table_apply_changes(
    state: State<'_, Arc<AppState>>,
    connection_id: String,
    request: ApplyChangesRequest,
) -> Result<ApplyChangesResult, String> {
    ensure_connection_writable(state.inner(), &connection_id, "Write external table").await?;
    let adapter = state.external_tables.get(&connection_id).await.map_err(String::from)?;
    if !adapter.capabilities().can_update {
        return Err("External table adapter does not support updates".to_string());
    }
    adapter.apply_changes(request).await.map_err(String::from)
}
