use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::connection::ConnectionTestResult;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InsertMode {
    Unsupported,
    Append,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    Unsupported,
    RemoveRow,
    DeleteRecord,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictMode {
    FileSnapshot,
    RevisionAndReadback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdapterCapabilities {
    pub can_read: bool,
    pub can_update: bool,
    pub insert_mode: InsertMode,
    pub delete_mode: DeleteMode,
    pub supports_cell_readonly: bool,
    pub conflict_mode: ConflictMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ExternalTableRef {
    pub table_key: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalValueType {
    String,
    Number,
    Boolean,
    DateTime,
    Json,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalColumn {
    pub column_key: String,
    pub display_name: String,
    pub value_type: ExternalValueType,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalRow {
    pub row_key: String,
    pub values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readonly_column_keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadState {
    Complete,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PageSnapshot {
    pub table: ExternalTableRef,
    pub columns: Vec<ExternalColumn>,
    pub rows: Vec<ExternalRow>,
    pub next_cursor: Option<String>,
    pub snapshot_token: String,
    pub read_state: ReadState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalTableSchema {
    pub table: ExternalTableRef,
    pub columns: Vec<ExternalColumn>,
    pub capabilities: AdapterCapabilities,
    pub writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadPageRequest {
    pub table: ExternalTableRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub limit: usize,
}

impl ReadPageRequest {
    pub fn bounded_limit(&self, maximum: usize) -> Result<usize, ExternalTableError> {
        if self.limit == 0 {
            return Err(ExternalTableError::invalid("Page limit must be greater than zero"));
        }
        Ok(self.limit.min(maximum))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalCellInput {
    pub column_key: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ExternalOperation {
    Update { operation_id: String, row_key: String, column_key: String, old_value: Value, new_value: Value },
    Insert { operation_id: String, values: Vec<ExternalCellInput> },
    Delete { operation_id: String, row_key: String },
}

impl ExternalOperation {
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Update { operation_id, .. }
            | Self::Insert { operation_id, .. }
            | Self::Delete { operation_id, .. } => operation_id,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    Applied,
    Conflict,
    Rejected,
    Unknown,
    NotAttempted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationResult {
    pub operation_id: String,
    pub outcome: OperationOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_row_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl OperationResult {
    pub fn new(operation_id: impl Into<String>, outcome: OperationOutcome) -> Self {
        Self { operation_id: operation_id.into(), outcome, created_row_key: None, message: None }
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyChangesRequest {
    pub table: ExternalTableRef,
    pub snapshot_token: String,
    pub operations: Vec<ExternalOperation>,
}

impl ApplyChangesRequest {
    pub fn validate(&self) -> Result<(), ExternalTableError> {
        if self.operations.is_empty() {
            return Err(ExternalTableError::invalid("At least one external table operation is required"));
        }
        let mut operation_ids = HashSet::with_capacity(self.operations.len());
        for operation in &self.operations {
            let operation_id = operation.operation_id().trim();
            if operation_id.is_empty() {
                return Err(ExternalTableError::invalid("External table operation ID must not be empty"));
            }
            if !operation_ids.insert(operation_id) {
                return Err(ExternalTableError::invalid(format!(
                    "Duplicate external table operation ID: {operation_id}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyChangesResult {
    pub operation_results: Vec<OperationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_snapshot_token: Option<String>,
    pub reload_required: bool,
    pub save_blocked: bool,
}

impl ApplyChangesResult {
    pub fn has_unknown(&self) -> bool {
        self.operation_results.iter().any(|result| result.outcome == OperationOutcome::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalTableErrorKind {
    InvalidRequest,
    NotConnected,
    Unsupported,
    Io,
    Transport,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTableError {
    pub kind: ExternalTableErrorKind,
    message: String,
}

impl ExternalTableError {
    pub fn new(kind: ExternalTableErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ExternalTableErrorKind::InvalidRequest, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ExternalTableErrorKind::Unsupported, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(ExternalTableErrorKind::Io, message)
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::new(ExternalTableErrorKind::Transport, message)
    }
}

impl fmt::Display for ExternalTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ExternalTableError {}

impl From<ExternalTableError> for String {
    fn from(error: ExternalTableError) -> Self {
        error.to_string()
    }
}

pub type ExternalConnectionTestResult = ConnectionTestResult;

#[cfg(test)]
mod tests {
    use super::*;

    fn update(operation_id: &str) -> ExternalOperation {
        ExternalOperation::Update {
            operation_id: operation_id.to_string(),
            row_key: "row:1".to_string(),
            column_key: "col:1".to_string(),
            old_value: Value::Null,
            new_value: Value::String("value".to_string()),
        }
    }

    #[test]
    fn rejects_duplicate_operation_ids_before_dispatch() {
        let request = ApplyChangesRequest {
            table: ExternalTableRef { table_key: "table".to_string(), display_name: "Table".to_string() },
            snapshot_token: "snapshot".to_string(),
            operations: vec![update("same"), update("same")],
        };

        let error = request.validate().unwrap_err();

        assert_eq!(error.kind, ExternalTableErrorKind::InvalidRequest);
        assert!(error.to_string().contains("Duplicate"));
    }

    #[test]
    fn operation_outcome_serialization_is_stable() {
        let values = [
            (OperationOutcome::Applied, "\"applied\""),
            (OperationOutcome::Conflict, "\"conflict\""),
            (OperationOutcome::Rejected, "\"rejected\""),
            (OperationOutcome::Unknown, "\"unknown\""),
            (OperationOutcome::NotAttempted, "\"not_attempted\""),
        ];
        for (value, expected) in values {
            assert_eq!(serde_json::to_string(&value).unwrap(), expected);
        }
    }
}
