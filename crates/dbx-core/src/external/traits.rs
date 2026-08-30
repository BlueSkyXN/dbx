use async_trait::async_trait;

use super::types::{
    AdapterCapabilities, ApplyChangesRequest, ApplyChangesResult, ExternalConnectionTestResult, ExternalTableError,
    ExternalTableRef, ExternalTableSchema, PageSnapshot, ReadPageRequest,
};

#[async_trait]
pub trait ExternalTableAdapter: Send + Sync + std::fmt::Debug {
    fn capabilities(&self) -> AdapterCapabilities;

    async fn test_connection(&self) -> Result<ExternalConnectionTestResult, ExternalTableError>;

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, ExternalTableError>;

    async fn describe_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSchema, ExternalTableError>;

    async fn read_page(&self, request: ReadPageRequest) -> Result<PageSnapshot, ExternalTableError>;

    async fn apply_changes(&self, request: ApplyChangesRequest) -> Result<ApplyChangesResult, ExternalTableError>;
}
