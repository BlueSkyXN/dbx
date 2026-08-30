use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::models::connection::{ConnectionConfig, DatabaseType};

use super::{
    CsvAdapter, ExternalSourceConfig, ExternalTableAdapter, ExternalTableError, FeishuBaseAdapter, FeishuSheetsAdapter,
    XlsxAdapter,
};

pub fn adapter_from_connection(config: &ConnectionConfig) -> Result<Arc<dyn ExternalTableAdapter>, ExternalTableError> {
    if !config.db_type.is_external_tabular() {
        return Err(ExternalTableError::invalid(format!(
            "{} is not an external table connection",
            config.db_type.as_str()
        )));
    }
    let source = ExternalSourceConfig::parse(config.db_type, config.external_config.as_ref())
        .map_err(ExternalTableError::invalid)?;
    let timeout = Duration::from_secs(config.effective_connect_timeout_secs());
    match (config.db_type, source) {
        (DatabaseType::Csv, ExternalSourceConfig::Csv(source)) => {
            Ok(Arc::new(CsvAdapter::new(required_file_path(config, "CSV")?, source)))
        }
        (DatabaseType::Xlsx, ExternalSourceConfig::Xlsx(source)) => {
            Ok(Arc::new(XlsxAdapter::new(required_file_path(config, "XLSX")?, source)))
        }
        (DatabaseType::FeishuSheets, ExternalSourceConfig::FeishuSheets(source)) => {
            Ok(Arc::new(FeishuSheetsAdapter::new(&config.username, &config.password, source, timeout)?))
        }
        (DatabaseType::FeishuBase, ExternalSourceConfig::FeishuBase(source)) => {
            Ok(Arc::new(FeishuBaseAdapter::new(&config.username, &config.password, source, timeout)?))
        }
        _ => Err(ExternalTableError::invalid("External table connection type/configuration mismatch")),
    }
}

fn required_file_path(config: &ConnectionConfig, label: &str) -> Result<PathBuf, ExternalTableError> {
    let path = config.host.trim();
    if path.is_empty() {
        return Err(ExternalTableError::invalid(format!("{label} file path is required")));
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(db_type: &str, host: &str, external_config: Value) -> ConnectionConfig {
        serde_json::from_value(json!({
            "id": "external",
            "name": "External",
            "db_type": db_type,
            "host": host,
            "port": 0,
            "username": "",
            "password": "",
            "database": null,
            "external_config": external_config
        }))
        .unwrap()
    }

    use serde_json::{json, Value};

    #[tokio::test]
    async fn builds_and_tests_csv_adapter_from_connection_host_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.csv");
        std::fs::write(&path, "name,amount\nAda,3\n").unwrap();
        let config = connection("csv", path.to_str().unwrap(), json!({ "hasHeader": true }));

        let adapter = adapter_from_connection(&config).unwrap();

        assert!(adapter.test_connection().await.unwrap().message.contains("CSV file valid"));
    }

    #[test]
    fn rejects_non_external_connections_and_missing_file_paths() {
        let mysql = connection("mysql", "localhost", json!({}));
        assert!(adapter_from_connection(&mysql).unwrap_err().to_string().contains("not an external table"));

        let csv = connection("csv", "", json!({}));
        assert!(adapter_from_connection(&csv).unwrap_err().to_string().contains("file path"));
    }
}
