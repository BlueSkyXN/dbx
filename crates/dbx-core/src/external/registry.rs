use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::{ExternalTableAdapter, ExternalTableError, ExternalTableErrorKind};

#[derive(Default)]
pub struct ExternalTableRegistry {
    adapters: RwLock<HashMap<String, Arc<dyn ExternalTableAdapter>>>,
}

impl std::fmt::Debug for ExternalTableRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ExternalTableRegistry").finish_non_exhaustive()
    }
}

impl ExternalTableRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, connection_id: impl Into<String>, adapter: Arc<dyn ExternalTableAdapter>) {
        self.adapters.write().await.insert(connection_id.into(), adapter);
    }

    pub async fn get(&self, connection_id: &str) -> Result<Arc<dyn ExternalTableAdapter>, ExternalTableError> {
        self.adapters.read().await.get(connection_id).cloned().ok_or_else(|| {
            ExternalTableError::new(
                ExternalTableErrorKind::NotConnected,
                format!("External table connection is not connected: {connection_id}"),
            )
        })
    }

    pub async fn remove(&self, connection_id: &str) -> Option<Arc<dyn ExternalTableAdapter>> {
        self.adapters.write().await.remove(connection_id)
    }

    pub async fn contains(&self, connection_id: &str) -> bool {
        self.adapters.read().await.contains_key(connection_id)
    }

    pub async fn clear(&self) {
        self.adapters.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::external::{
        AdapterCapabilities, ApplyChangesRequest, ApplyChangesResult, ConflictMode, DeleteMode,
        ExternalConnectionTestResult, ExternalTableRef, ExternalTableSchema, InsertMode, PageSnapshot, ReadPageRequest,
    };

    #[derive(Debug)]
    struct NoopAdapter;

    #[async_trait]
    impl ExternalTableAdapter for NoopAdapter {
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities {
                can_read: true,
                can_update: false,
                insert_mode: InsertMode::Unsupported,
                delete_mode: DeleteMode::Unsupported,
                supports_cell_readonly: false,
                conflict_mode: ConflictMode::FileSnapshot,
            }
        }

        async fn test_connection(&self) -> Result<ExternalConnectionTestResult, ExternalTableError> {
            Ok(ExternalConnectionTestResult::success("ok"))
        }

        async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, ExternalTableError> {
            Ok(Vec::new())
        }

        async fn describe_table(&self, _table: &ExternalTableRef) -> Result<ExternalTableSchema, ExternalTableError> {
            Err(ExternalTableError::unsupported("not used"))
        }

        async fn read_page(&self, _request: ReadPageRequest) -> Result<PageSnapshot, ExternalTableError> {
            Err(ExternalTableError::unsupported("not used"))
        }

        async fn apply_changes(&self, _request: ApplyChangesRequest) -> Result<ApplyChangesResult, ExternalTableError> {
            Err(ExternalTableError::unsupported("not used"))
        }
    }

    #[tokio::test]
    async fn registry_requires_explicit_connect_and_clears_on_disconnect() {
        let registry = ExternalTableRegistry::new();
        assert_eq!(registry.get("connection").await.unwrap_err().kind, ExternalTableErrorKind::NotConnected);

        registry.insert("connection", Arc::new(NoopAdapter)).await;
        assert!(registry.contains("connection").await);
        assert_eq!(registry.get("connection").await.unwrap().test_connection().await.unwrap().message, "ok");

        registry.remove("connection").await;
        assert!(!registry.contains("connection").await);
    }
}
