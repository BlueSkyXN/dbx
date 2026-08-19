use async_trait::async_trait;

use super::types::{
    ExternalCapabilities, ExternalColumnDef, ExternalRowUpdate, ExternalTableRef, ExternalTableSnapshot,
    ExternalWriteResult,
};

/// Trait for external tabular data sources.
#[async_trait]
pub trait ExternalTabularSource: Send + Sync + std::fmt::Debug {
    fn capabilities(&self) -> ExternalCapabilities;

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, String>;

    async fn get_columns(&self, table: &ExternalTableRef) -> Result<Vec<ExternalColumnDef>, String>;

    async fn load_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSnapshot, String>;

    async fn source_version(&self, table: &ExternalTableRef) -> Result<String, String>;

    async fn test_connection(&self) -> Result<String, String>;

    fn display_name(&self) -> String;

    fn refresh_before_query(&self) -> bool {
        false
    }

    async fn append_rows(
        &self,
        _table: &ExternalTableRef,
        _rows: Vec<Vec<serde_json::Value>>,
    ) -> Result<ExternalWriteResult, String> {
        Err("External source does not support appending rows".to_string())
    }

    async fn update_rows(
        &self,
        _table: &ExternalTableRef,
        _updates: Vec<ExternalRowUpdate>,
    ) -> Result<ExternalWriteResult, String> {
        Err("External source does not support updating rows".to_string())
    }

    async fn delete_rows(
        &self,
        _table: &ExternalTableRef,
        _row_ids: Vec<String>,
    ) -> Result<ExternalWriteResult, String> {
        Err("External source does not support deleting rows".to_string())
    }

    async fn write_range(
        &self,
        _table: &ExternalTableRef,
        _range: &str,
        _rows: Vec<Vec<serde_json::Value>>,
    ) -> Result<ExternalWriteResult, String> {
        Err("External source does not support range writes".to_string())
    }
}
