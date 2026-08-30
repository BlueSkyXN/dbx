use std::collections::{HashMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::feishu::{FeishuClient, FeishuRequestError, FeishuRequestErrorKind};
use super::{
    AdapterCapabilities, ApplyChangesRequest, ApplyChangesResult, ConflictMode, DeleteMode, ExternalCellInput,
    ExternalColumn, ExternalConnectionTestResult, ExternalOperation, ExternalRow, ExternalTableAdapter,
    ExternalTableError, ExternalTableRef, ExternalTableSchema, ExternalValueType, FeishuBaseExternalConfig, InsertMode,
    OperationOutcome, OperationResult, PageSnapshot, ReadPageRequest, ReadState,
};

const TABLE_KEY_PREFIX: &str = "table:";
const ROW_KEY_PREFIX: &str = "record:";
const COLUMN_KEY_PREFIX: &str = "field:";
const MAX_PAGE_SIZE: usize = 200;
const METADATA_PAGE_SIZE: usize = 200;
const MAX_METADATA_PAGES: usize = 100;
const MAX_BATCH_SIZE: usize = 200;

#[derive(Debug, Clone)]
pub struct FeishuBaseAdapter {
    client: FeishuClient,
    config: FeishuBaseExternalConfig,
    write_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Clone)]
struct BaseTable {
    table_id: String,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BaseFieldKind {
    Text,
    Number,
    Select { multiple: bool },
    DateTime,
    Checkbox,
    Readonly,
}

#[derive(Debug, Clone)]
struct BaseField {
    field_id: String,
    name: String,
    kind: BaseFieldKind,
}

#[derive(Debug, Clone)]
struct BaseMetadata {
    table: BaseTable,
    fields: Vec<BaseField>,
    schema_digest: String,
}

#[derive(Debug, Clone)]
struct BaseRecord {
    record_id: String,
    fields: Map<String, Value>,
}

#[derive(Debug, Clone)]
struct RecordsPage {
    records: Vec<BaseRecord>,
    revision: Option<String>,
    has_more: bool,
    incomplete: bool,
}

#[derive(Debug, Clone)]
struct PreparedUpdate {
    operation_index: usize,
    record_id: String,
    field_id: String,
    old_value: Value,
    new_value: Value,
}

#[derive(Debug, Clone)]
struct PreparedDelete {
    operation_index: usize,
    record_id: String,
}

#[derive(Debug, Clone)]
struct PreparedInsert {
    operation_index: usize,
    fields: Map<String, Value>,
}

#[derive(Debug)]
struct UpdateBatchItem {
    record_id: String,
    fields: Map<String, Value>,
    field_ids: Vec<String>,
    operation_indexes: Vec<usize>,
}

impl BaseFieldKind {
    fn writable(&self) -> bool {
        !matches!(self, Self::Readonly)
    }

    fn value_type(&self) -> ExternalValueType {
        match self {
            Self::Text => ExternalValueType::String,
            Self::Number => ExternalValueType::Number,
            Self::Select { .. } => ExternalValueType::Json,
            Self::DateTime => ExternalValueType::DateTime,
            Self::Checkbox => ExternalValueType::Boolean,
            Self::Readonly => ExternalValueType::Json,
        }
    }

    fn validate_value(&self, value: &Value) -> Result<(), &'static str> {
        if value.is_null() {
            return Ok(());
        }
        match self {
            Self::Text if value.is_string() => Ok(()),
            Self::Number if value.is_number() => Ok(()),
            Self::Select { multiple: true }
                if value.as_array().is_some_and(|values| values.iter().all(|value| value.is_string())) =>
            {
                Ok(())
            }
            Self::Select { multiple: false }
                if value
                    .as_array()
                    .is_some_and(|values| values.len() <= 1 && values.iter().all(|value| value.is_string())) =>
            {
                Ok(())
            }
            Self::DateTime if value.is_string() || value.as_i64().is_some() || value.as_u64().is_some() => Ok(()),
            Self::Checkbox if value.is_boolean() => Ok(()),
            Self::Readonly => Err("field is read-only"),
            Self::Text => Err("field requires a string or null"),
            Self::Number => Err("field requires a number or null"),
            Self::Select { multiple: false } => {
                Err("single-select field requires an array containing at most one option name, or null")
            }
            Self::Select { multiple: true } => Err("multi-select field requires an array of strings or null"),
            Self::DateTime => Err("datetime field requires a timestamp, string, or null"),
            Self::Checkbox => Err("checkbox field requires a boolean or null"),
        }
    }
}

impl FeishuBaseAdapter {
    pub fn new(
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
        config: FeishuBaseExternalConfig,
        timeout: Duration,
    ) -> Result<Self, ExternalTableError> {
        let client = FeishuClient::new(app_id, app_secret, timeout)?;
        Self::from_client(client, config)
    }

    pub(crate) fn from_client(
        client: FeishuClient,
        config: FeishuBaseExternalConfig,
    ) -> Result<Self, ExternalTableError> {
        if config.base_token.trim().is_empty() {
            return Err(ExternalTableError::invalid("Feishu Base baseToken is required"));
        }
        Ok(Self { client, config, write_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())) })
    }

    fn base_path(&self, suffix: &str) -> String {
        format!("/open-apis/base/v3/bases/{}{}", encode_path_segment(&self.config.base_token), suffix)
    }

    fn table_path(&self, table_id: &str, suffix: &str) -> String {
        self.base_path(&format!("/tables/{}{}", encode_path_segment(table_id), suffix))
    }

    fn table_ref(table: &BaseTable) -> ExternalTableRef {
        ExternalTableRef {
            table_key: format!("{TABLE_KEY_PREFIX}{}", encode_path_segment(&table.table_id)),
            display_name: table.name.clone(),
        }
    }

    fn table_id(table: &ExternalTableRef) -> Result<String, ExternalTableError> {
        decode_key(&table.table_key, TABLE_KEY_PREFIX, "Feishu Base table")
    }

    async fn list_base_tables(&self) -> Result<Vec<BaseTable>, ExternalTableError> {
        let path = self.base_path("/tables");
        let mut offset = 0_usize;
        let mut tables = Vec::new();
        let mut completed = false;
        for _ in 0..MAX_METADATA_PAGES {
            let data = self
                .client
                .get_json(&path, &[("offset", offset.to_string()), ("limit", METADATA_PAGE_SIZE.to_string())])
                .await
                .map_err(|error| error.as_external_error())?;
            let raw =
                data.get("tables").or_else(|| data.get("items")).and_then(Value::as_array).cloned().unwrap_or_default();
            let batch = raw.iter().filter_map(parse_table).collect::<Vec<_>>();
            let batch_len = batch.len();
            if batch_len != raw.len() {
                return Err(ExternalTableError::invalid("Feishu Base table metadata contains malformed entries"));
            }
            tables.extend(
                batch.into_iter().filter(|table| {
                    self.config.table_id.as_deref().is_none_or(|configured| configured == table.table_id)
                }),
            );
            let total = usize_value(data.get("total"));
            let has_more = bool_value(data.get("has_more"))
                .unwrap_or_else(|| total.is_some_and(|total| offset.saturating_add(batch_len) < total));
            if !has_more {
                completed = true;
                break;
            }
            if batch_len == 0 {
                return Err(ExternalTableError::invalid(
                    "Feishu Base table metadata reported more pages but returned no items",
                ));
            }
            offset = offset.saturating_add(batch_len);
        }
        if !completed {
            return Err(ExternalTableError::invalid("Feishu Base table metadata exceeded the pagination limit"));
        }
        if let Some(configured) = self.config.table_id.as_deref() {
            if tables.iter().all(|table| table.table_id != configured) {
                return Err(ExternalTableError::invalid(format!("Feishu Base table not found: {configured}")));
            }
        }
        if tables.is_empty() {
            return Err(ExternalTableError::invalid("Feishu Base returned no accessible tables"));
        }
        Ok(tables)
    }

    async fn metadata(&self, table_id: &str) -> Result<BaseMetadata, ExternalTableError> {
        let table_data = self
            .client
            .get_json(&self.table_path(table_id, ""), &[])
            .await
            .map_err(|error| error.as_external_error())?;
        let table = parse_table(table_data.get("table").unwrap_or(&table_data))
            .unwrap_or_else(|| BaseTable { table_id: table_id.to_string(), name: table_id.to_string() });
        if table.table_id != table_id {
            return Err(ExternalTableError::invalid("Feishu Base table metadata returned a different table ID"));
        }

        let path = self.table_path(table_id, "/fields");
        let mut offset = 0_usize;
        let mut fields = Vec::new();
        let mut completed = false;
        for _ in 0..MAX_METADATA_PAGES {
            let data = self
                .client
                .get_json(&path, &[("offset", offset.to_string()), ("limit", METADATA_PAGE_SIZE.to_string())])
                .await
                .map_err(|error| error.as_external_error())?;
            let raw =
                data.get("fields").or_else(|| data.get("items")).and_then(Value::as_array).cloned().unwrap_or_default();
            let batch = raw.iter().filter_map(parse_field).collect::<Vec<_>>();
            let batch_len = batch.len();
            if batch_len != raw.len() {
                return Err(ExternalTableError::invalid("Feishu Base field metadata contains malformed entries"));
            }
            fields.extend(batch);
            let total = usize_value(data.get("total"));
            let has_more = bool_value(data.get("has_more"))
                .unwrap_or_else(|| total.is_some_and(|total| offset.saturating_add(batch_len) < total));
            if !has_more {
                completed = true;
                break;
            }
            if batch_len == 0 {
                return Err(ExternalTableError::invalid(
                    "Feishu Base field metadata reported more pages but returned no items",
                ));
            }
            offset = offset.saturating_add(batch_len);
        }
        if !completed {
            return Err(ExternalTableError::invalid("Feishu Base field metadata exceeded the pagination limit"));
        }
        if fields.is_empty() {
            return Err(ExternalTableError::invalid("Feishu Base table returned no field schema"));
        }
        let mut field_ids = HashSet::new();
        if fields.iter().any(|field| !field_ids.insert(field.field_id.clone())) {
            return Err(ExternalTableError::invalid("Feishu Base field schema contains duplicate field IDs"));
        }
        let schema_digest = schema_digest(&table, &fields);
        Ok(BaseMetadata { table, fields, schema_digest })
    }

    async fn fetch_records_page(
        &self,
        table_id: &str,
        fields: &[BaseField],
        offset: usize,
        limit: usize,
    ) -> Result<RecordsPage, ExternalTableError> {
        let path = self.table_path(table_id, "/records");
        let mut query = vec![("offset", offset.to_string()), ("limit", limit.to_string())];
        if let Some(view_id) = self.config.view_id.as_deref().filter(|value| !value.trim().is_empty()) {
            query.push(("view_id", view_id.to_string()));
        }
        for field in fields {
            query.push(("field_id", field.field_id.clone()));
        }
        let data = self.client.get_json(&path, &query).await.map_err(|error| error.as_external_error())?;
        parse_records_page(&data)
    }

    async fn fetch_records_by_ids(
        &self,
        table_id: &str,
        record_ids: &[String],
        field_ids: &[String],
    ) -> Result<RecordsPage, ExternalTableError> {
        if record_ids.len() > MAX_BATCH_SIZE {
            return Err(ExternalTableError::invalid("Feishu Base batch_get accepts at most 200 records"));
        }
        let mut body = json!({ "record_id_list": record_ids });
        if !field_ids.is_empty() {
            body["select_fields"] = json!(field_ids);
        }
        let data = self
            .client
            .post_json(&self.table_path(table_id, "/records/batch_get"), body, false)
            .await
            .map_err(|error| error.as_external_error())?;
        parse_records_page(&data)
    }

    async fn current_snapshot(&self, table_id: &str, metadata: &BaseMetadata) -> Result<String, ExternalTableError> {
        let page = self.fetch_records_page(table_id, &metadata.fields, 0, 1).await?;
        if page.incomplete {
            return Err(ExternalTableError::unsupported(
                "Feishu Base returned an incomplete record snapshot; writes are blocked",
            ));
        }
        let revision = page.revision.filter(|revision| !revision.is_empty()).ok_or_else(|| {
            ExternalTableError::unsupported("Feishu Base did not return a revision; writes are blocked")
        })?;
        Ok(snapshot_token(table_id, &revision, &metadata.schema_digest))
    }

    async fn page(&self, request: ReadPageRequest) -> Result<PageSnapshot, ExternalTableError> {
        let limit = request.bounded_limit(MAX_PAGE_SIZE)?;
        let offset = parse_cursor(request.cursor.as_deref())?;
        let table_id = Self::table_id(&request.table)?;
        let before = self.metadata(&table_id).await?;
        let page = self.fetch_records_page(&table_id, &before.fields, offset, limit).await?;
        let after = self.metadata(&table_id).await?;
        let schema_changed = before.schema_digest != after.schema_digest;
        let revision = page.revision.clone().unwrap_or_else(|| "unknown".to_string());
        let readonly_column_keys = before
            .fields
            .iter()
            .filter(|field| !field.kind.writable())
            .map(|field| column_key(&field.field_id))
            .collect::<Vec<_>>();
        let rows = page
            .records
            .iter()
            .map(|record| ExternalRow {
                row_key: row_key(&record.record_id),
                values: before.fields.iter().map(|field| record_value(record, field)).collect(),
                readonly_column_keys: readonly_column_keys.clone(),
            })
            .collect::<Vec<_>>();
        let next_cursor = (page.has_more && !rows.is_empty()).then(|| offset.saturating_add(rows.len()).to_string());
        Ok(PageSnapshot {
            table: Self::table_ref(&before.table),
            columns: columns(&before.fields),
            rows,
            next_cursor,
            snapshot_token: snapshot_token(&table_id, &revision, &before.schema_digest),
            read_state: if schema_changed || page.incomplete || page.revision.is_none() {
                ReadState::Incomplete
            } else {
                ReadState::Complete
            },
        })
    }
}

#[async_trait]
impl ExternalTableAdapter for FeishuBaseAdapter {
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            can_read: true,
            can_update: true,
            insert_mode: InsertMode::Append,
            delete_mode: DeleteMode::DeleteRecord,
            supports_cell_readonly: true,
            conflict_mode: ConflictMode::RevisionAndReadback,
        }
    }

    async fn test_connection(&self) -> Result<ExternalConnectionTestResult, ExternalTableError> {
        let tables = self.list_base_tables().await?;
        Ok(ExternalConnectionTestResult::success(format!("Feishu Base connection valid: {} table(s)", tables.len())))
    }

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, ExternalTableError> {
        Ok(self.list_base_tables().await?.iter().map(Self::table_ref).collect())
    }

    async fn describe_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSchema, ExternalTableError> {
        let table_id = Self::table_id(table)?;
        let metadata = self.metadata(&table_id).await?;
        let writable = metadata.fields.iter().any(|field| field.kind.writable());
        Ok(ExternalTableSchema {
            table: Self::table_ref(&metadata.table),
            columns: columns(&metadata.fields),
            capabilities: self.capabilities(),
            writable,
            readonly_reason: (!writable).then(|| "Feishu Base table has no supported writable fields".to_string()),
        })
    }

    async fn read_page(&self, request: ReadPageRequest) -> Result<PageSnapshot, ExternalTableError> {
        self.page(request).await
    }

    async fn apply_changes(&self, request: ApplyChangesRequest) -> Result<ApplyChangesResult, ExternalTableError> {
        request.validate()?;
        let _guard = self.write_lock.lock().await;
        apply_base_changes(self, request).await
    }
}

async fn apply_base_changes(
    adapter: &FeishuBaseAdapter,
    request: ApplyChangesRequest,
) -> Result<ApplyChangesResult, ExternalTableError> {
    let table_id = FeishuBaseAdapter::table_id(&request.table)?;
    let metadata = adapter.metadata(&table_id).await?;
    let field_by_id = metadata.fields.iter().map(|field| (field.field_id.as_str(), field)).collect::<HashMap<_, _>>();
    let current_snapshot = adapter.current_snapshot(&table_id, &metadata).await?;
    let (snapshot_table_id, _, requested_schema) = snapshot_parts(&request.snapshot_token)
        .ok_or_else(|| ExternalTableError::invalid("Invalid Feishu Base snapshot token"))?;
    if snapshot_table_id != table_id {
        return Err(ExternalTableError::invalid("Feishu Base snapshot token belongs to a different table"));
    }
    let schema_changed = requested_schema != metadata.schema_digest;
    let revision_changed = request.snapshot_token != current_snapshot;
    let mut results = vec![None; request.operations.len()];
    let mut updates = Vec::new();
    let mut deletes = Vec::new();
    let mut inserts = Vec::new();
    let mut seen_updates = HashSet::new();
    let mut seen_deletes = HashSet::new();

    for (index, operation) in request.operations.iter().enumerate() {
        if schema_changed {
            results[index] = Some(
                OperationResult::new(operation.operation_id(), OperationOutcome::Conflict)
                    .message("Feishu Base field schema changed; reload before saving"),
            );
            continue;
        }
        match operation {
            ExternalOperation::Update { operation_id, row_key, column_key, old_value, new_value } => {
                let record_id = match decode_key(row_key, ROW_KEY_PREFIX, "Feishu Base record") {
                    Ok(record_id) => record_id,
                    Err(error) => {
                        results[index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                let field_id = match decode_key(column_key, COLUMN_KEY_PREFIX, "Feishu Base field") {
                    Ok(field_id) => field_id,
                    Err(error) => {
                        results[index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                let Some(field) = field_by_id.get(field_id.as_str()) else {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("Feishu Base field no longer exists"),
                    );
                    continue;
                };
                if !field.kind.writable() {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message(format!("Feishu Base field '{}' is read-only", field.name)),
                    );
                    continue;
                }
                if let Err(message) = field.kind.validate_value(new_value) {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message(format!("Feishu Base field '{}': {message}", field.name)),
                    );
                    continue;
                }
                if !seen_updates.insert((record_id.clone(), field_id.clone())) {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("The same Feishu Base cell is updated more than once"),
                    );
                    continue;
                }
                updates.push(PreparedUpdate {
                    operation_index: index,
                    record_id,
                    field_id,
                    old_value: old_value.clone(),
                    new_value: new_value.clone(),
                });
            }
            ExternalOperation::Delete { operation_id, row_key } => {
                let record_id = match decode_key(row_key, ROW_KEY_PREFIX, "Feishu Base record") {
                    Ok(record_id) => record_id,
                    Err(error) => {
                        results[index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                        continue;
                    }
                };
                if !seen_deletes.insert(record_id.clone()) {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Rejected)
                            .message("Feishu Base record is already scheduled for deletion"),
                    );
                } else if revision_changed {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Conflict)
                            .message("Feishu Base revision changed; delete requires reload"),
                    );
                } else {
                    deletes.push(PreparedDelete { operation_index: index, record_id });
                }
            }
            ExternalOperation::Insert { operation_id, values } => {
                if revision_changed {
                    results[index] = Some(
                        OperationResult::new(operation_id, OperationOutcome::Conflict)
                            .message("Feishu Base revision changed; create requires reload"),
                    );
                    continue;
                }
                match prepare_insert_fields(values, &field_by_id) {
                    Ok(fields) => inserts.push(PreparedInsert { operation_index: index, fields }),
                    Err(error) => {
                        results[index] = Some(
                            OperationResult::new(operation_id, OperationOutcome::Rejected).message(error.to_string()),
                        );
                    }
                }
            }
        }
    }

    preflight_updates(adapter, &table_id, &metadata.fields, &updates, &request, &mut results).await?;
    updates.retain(|update| results[update.operation_index].is_none());

    let update_items = group_update_items(&updates);
    let mut stop_dispatch = dispatch_update_batches(adapter, &table_id, &update_items, &request, &mut results).await;
    if !stop_dispatch {
        stop_dispatch = dispatch_delete_batches(adapter, &table_id, &deletes, &request, &mut results).await;
    }
    if !stop_dispatch {
        stop_dispatch = dispatch_create_batches(adapter, &table_id, &inserts, &request, &mut results).await;
    }
    if stop_dispatch {
        fill_not_attempted(&request, &mut results);
    }

    let mut operation_results = results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.unwrap_or_else(|| {
                OperationResult::new(request.operations[index].operation_id(), OperationOutcome::Rejected)
                    .message("Feishu Base operation could not be prepared")
            })
        })
        .collect::<Vec<_>>();
    let has_applied = operation_results.iter().any(|result| result.outcome == OperationOutcome::Applied);
    let has_unknown = operation_results.iter().any(|result| result.outcome == OperationOutcome::Unknown);
    let mut reload_required = operation_results.iter().any(|result| {
        matches!(result.outcome, OperationOutcome::Applied | OperationOutcome::Conflict | OperationOutcome::Unknown)
    });
    let mut new_snapshot_token = (!has_applied).then_some(current_snapshot);

    if has_applied {
        match read_back_applied(adapter, &table_id, &metadata.fields, &updates, &inserts, &mut operation_results).await
        {
            Ok(revision) => match adapter.metadata(&table_id).await {
                Ok(after_metadata) => {
                    if after_metadata.schema_digest != metadata.schema_digest {
                        reload_required = true;
                    }
                    new_snapshot_token = Some(snapshot_token(&table_id, &revision, &after_metadata.schema_digest));
                }
                Err(error) => {
                    annotate_applied_readback_failure(&mut operation_results, &error.to_string());
                    reload_required = true;
                }
            },
            Err(error) => {
                annotate_applied_readback_failure(&mut operation_results, &error.to_string());
                reload_required = true;
            }
        }
    }

    Ok(ApplyChangesResult { operation_results, new_snapshot_token, reload_required, save_blocked: has_unknown })
}

async fn preflight_updates(
    adapter: &FeishuBaseAdapter,
    table_id: &str,
    fields: &[BaseField],
    updates: &[PreparedUpdate],
    request: &ApplyChangesRequest,
    results: &mut [Option<OperationResult>],
) -> Result<(), ExternalTableError> {
    let mut record_ids = Vec::new();
    let mut field_ids = Vec::new();
    for update in updates {
        if !record_ids.contains(&update.record_id) {
            record_ids.push(update.record_id.clone());
        }
        if !field_ids.contains(&update.field_id) {
            field_ids.push(update.field_id.clone());
        }
    }
    for record_chunk in record_ids.chunks(MAX_BATCH_SIZE) {
        let page = adapter.fetch_records_by_ids(table_id, record_chunk, &field_ids).await?;
        if page.incomplete || page.revision.as_deref().is_none_or(str::is_empty) {
            for update in updates.iter().filter(|update| record_chunk.contains(&update.record_id)) {
                results[update.operation_index] = Some(
                    OperationResult::new(
                        request.operations[update.operation_index].operation_id(),
                        OperationOutcome::Conflict,
                    )
                    .message("Feishu Base preflight response is incomplete; reload before saving"),
                );
            }
            continue;
        }
        let records = page.records.iter().map(|record| (record.record_id.as_str(), record)).collect::<HashMap<_, _>>();
        for update in updates.iter().filter(|update| record_chunk.contains(&update.record_id)) {
            let Some(record) = records.get(update.record_id.as_str()) else {
                results[update.operation_index] = Some(
                    OperationResult::new(
                        request.operations[update.operation_index].operation_id(),
                        OperationOutcome::Conflict,
                    )
                    .message("Feishu Base record no longer exists"),
                );
                continue;
            };
            let Some(field) = fields.iter().find(|field| field.field_id == update.field_id) else {
                continue;
            };
            if record_value(record, field) != update.old_value {
                results[update.operation_index] = Some(
                    OperationResult::new(
                        request.operations[update.operation_index].operation_id(),
                        OperationOutcome::Conflict,
                    )
                    .message("Feishu Base cell changed after it was read"),
                );
            }
        }
    }
    Ok(())
}

fn group_update_items(updates: &[PreparedUpdate]) -> Vec<UpdateBatchItem> {
    let mut positions = HashMap::new();
    let mut items: Vec<UpdateBatchItem> = Vec::new();
    for update in updates {
        let position = if let Some(position) = positions.get(&update.record_id).copied() {
            position
        } else {
            let position = items.len();
            positions.insert(update.record_id.clone(), position);
            items.push(UpdateBatchItem {
                record_id: update.record_id.clone(),
                fields: Map::new(),
                field_ids: Vec::new(),
                operation_indexes: Vec::new(),
            });
            position
        };
        items[position].fields.insert(update.field_id.clone(), update.new_value.clone());
        items[position].field_ids.push(update.field_id.clone());
        items[position].operation_indexes.push(update.operation_index);
    }
    items
}

async fn dispatch_update_batches(
    adapter: &FeishuBaseAdapter,
    table_id: &str,
    items: &[UpdateBatchItem],
    request: &ApplyChangesRequest,
    results: &mut [Option<OperationResult>],
) -> bool {
    for chunk in items.chunks(MAX_BATCH_SIZE) {
        let update_records = chunk
            .iter()
            .map(|item| (item.record_id.clone(), Value::Object(item.fields.clone())))
            .collect::<Map<_, _>>();
        let response = adapter
            .client
            .post_json(
                &adapter.table_path(table_id, "/records/batch_update"),
                json!({ "update_records": update_records }),
                true,
            )
            .await;
        if apply_update_batch_response(response, chunk, request, results) {
            return true;
        }
    }
    false
}

fn apply_update_batch_response(
    response: Result<Value, FeishuRequestError>,
    items: &[UpdateBatchItem],
    request: &ApplyChangesRequest,
    results: &mut [Option<OperationResult>],
) -> bool {
    match response {
        Ok(output) => {
            let failures = failure_indexes(&output);
            let ignored_fields = ignored_field_keys(&output);
            for (batch_index, item) in items.iter().enumerate() {
                for (field_id, operation_index) in item.field_ids.iter().zip(&item.operation_indexes) {
                    let outcome = if failures.contains(&batch_index) || ignored_fields.contains(field_id) {
                        OperationOutcome::Rejected
                    } else {
                        OperationOutcome::Applied
                    };
                    results[*operation_index] =
                        Some(OperationResult::new(request.operations[*operation_index].operation_id(), outcome));
                }
            }
            !failures.is_empty() || !ignored_fields.is_empty()
        }
        Err(error) if error.kind == FeishuRequestErrorKind::Unknown => {
            for item in items {
                for operation_index in &item.operation_indexes {
                    results[*operation_index] = Some(
                        OperationResult::new(
                            request.operations[*operation_index].operation_id(),
                            OperationOutcome::Unknown,
                        )
                        .message(error.message.clone()),
                    );
                }
            }
            true
        }
        Err(error) => {
            let failed_index = failure_indexes_from_message(&error.message).into_iter().min();
            for (batch_index, item) in items.iter().enumerate() {
                let outcome = match failed_index {
                    Some(failed) if batch_index < failed => OperationOutcome::Applied,
                    Some(failed) if batch_index == failed => OperationOutcome::Rejected,
                    Some(_) => OperationOutcome::NotAttempted,
                    None => OperationOutcome::Rejected,
                };
                for operation_index in &item.operation_indexes {
                    let mut result = OperationResult::new(request.operations[*operation_index].operation_id(), outcome);
                    if outcome == OperationOutcome::Rejected {
                        result.message = Some(error.message.clone());
                    }
                    results[*operation_index] = Some(result);
                }
            }
            true
        }
    }
}

async fn dispatch_delete_batches(
    adapter: &FeishuBaseAdapter,
    table_id: &str,
    deletes: &[PreparedDelete],
    request: &ApplyChangesRequest,
    results: &mut [Option<OperationResult>],
) -> bool {
    for chunk in deletes.chunks(MAX_BATCH_SIZE) {
        let record_ids = chunk.iter().map(|delete| delete.record_id.clone()).collect::<Vec<_>>();
        let response = adapter
            .client
            .post_json(
                &adapter.table_path(table_id, "/records/batch_delete"),
                json!({ "record_id_list": record_ids }),
                true,
            )
            .await;
        if apply_simple_batch_response(
            response,
            &chunk.iter().map(|item| item.operation_index).collect::<Vec<_>>(),
            request,
            results,
            false,
        ) {
            return true;
        }
    }
    false
}

async fn dispatch_create_batches(
    adapter: &FeishuBaseAdapter,
    table_id: &str,
    inserts: &[PreparedInsert],
    request: &ApplyChangesRequest,
    results: &mut [Option<OperationResult>],
) -> bool {
    for chunk in inserts.chunks(MAX_BATCH_SIZE) {
        let create_records = chunk.iter().map(|insert| Value::Object(insert.fields.clone())).collect::<Vec<_>>();
        let response = adapter
            .client
            .post_json(
                &adapter.table_path(table_id, "/records/batch_create"),
                json!({ "create_records": create_records }),
                true,
            )
            .await;
        match response {
            Ok(output) => {
                let failures = failure_indexes(&output);
                let ignored_fields = ignored_field_keys(&output);
                let created = output
                    .get("records")
                    .or_else(|| output.get("items"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let created_ids = output.get("record_id_list").and_then(Value::as_array).cloned().unwrap_or_default();
                for (batch_index, insert) in chunk.iter().enumerate() {
                    let operation_id = request.operations[insert.operation_index].operation_id();
                    if failures.contains(&batch_index) {
                        results[insert.operation_index] =
                            Some(OperationResult::new(operation_id, OperationOutcome::Rejected));
                        continue;
                    }
                    let record_id = created
                        .get(batch_index)
                        .and_then(parse_record_id)
                        .or_else(|| created_ids.get(batch_index).and_then(Value::as_str).map(str::to_string))
                        .or_else(|| {
                            created.iter().find_map(|value| {
                                (usize_value(value.get("index")) == Some(batch_index))
                                    .then(|| parse_record_id(value))
                                    .flatten()
                            })
                        });
                    results[insert.operation_index] = Some(match record_id {
                        Some(record_id) if ignored_fields.is_empty() => {
                            let mut result = OperationResult::new(operation_id, OperationOutcome::Applied);
                            result.created_row_key = Some(row_key(&record_id));
                            result
                        }
                        Some(record_id) => {
                            let mut result = OperationResult::new(operation_id, OperationOutcome::Unknown).message(
                                "Feishu Base created the record but ignored one or more fields; reload before retrying",
                            );
                            result.created_row_key = Some(row_key(&record_id));
                            result
                        }
                        None => OperationResult::new(operation_id, OperationOutcome::Unknown)
                            .message("Feishu Base may have created the record but did not return its record ID"),
                    });
                }
                if !failures.is_empty()
                    || !ignored_fields.is_empty()
                    || chunk.iter().any(|insert| {
                        results[insert.operation_index]
                            .as_ref()
                            .is_some_and(|result| result.outcome == OperationOutcome::Unknown)
                    })
                {
                    return true;
                }
            }
            Err(error) if error.kind == FeishuRequestErrorKind::Unknown => {
                for insert in chunk {
                    results[insert.operation_index] = Some(
                        OperationResult::new(
                            request.operations[insert.operation_index].operation_id(),
                            OperationOutcome::Unknown,
                        )
                        .message(error.message.clone()),
                    );
                }
                return true;
            }
            Err(error) => {
                let failed_index = failure_indexes_from_message(&error.message).into_iter().min();
                for (batch_index, insert) in chunk.iter().enumerate() {
                    let outcome = match failed_index {
                        Some(failed) if batch_index < failed => OperationOutcome::Unknown,
                        Some(failed) if batch_index == failed => OperationOutcome::Rejected,
                        Some(_) => OperationOutcome::NotAttempted,
                        None => OperationOutcome::Rejected,
                    };
                    let mut result =
                        OperationResult::new(request.operations[insert.operation_index].operation_id(), outcome);
                    if matches!(outcome, OperationOutcome::Rejected | OperationOutcome::Unknown) {
                        result.message = Some(error.message.clone());
                    }
                    results[insert.operation_index] = Some(result);
                }
                return true;
            }
        }
    }
    false
}

fn apply_simple_batch_response(
    response: Result<Value, FeishuRequestError>,
    operation_indexes: &[usize],
    request: &ApplyChangesRequest,
    results: &mut [Option<OperationResult>],
    create: bool,
) -> bool {
    match response {
        Ok(output) => {
            let failures = failure_indexes(&output);
            for (batch_index, operation_index) in operation_indexes.iter().enumerate() {
                let outcome = if failures.contains(&batch_index) {
                    OperationOutcome::Rejected
                } else {
                    OperationOutcome::Applied
                };
                results[*operation_index] =
                    Some(OperationResult::new(request.operations[*operation_index].operation_id(), outcome));
            }
            !failures.is_empty()
        }
        Err(error) if error.kind == FeishuRequestErrorKind::Unknown => {
            for operation_index in operation_indexes {
                results[*operation_index] = Some(
                    OperationResult::new(
                        request.operations[*operation_index].operation_id(),
                        OperationOutcome::Unknown,
                    )
                    .message(error.message.clone()),
                );
            }
            true
        }
        Err(error) => {
            let failed_index = failure_indexes_from_message(&error.message).into_iter().min();
            for (batch_index, operation_index) in operation_indexes.iter().enumerate() {
                let outcome = match failed_index {
                    Some(failed) if batch_index < failed && create => OperationOutcome::Unknown,
                    Some(failed) if batch_index < failed => OperationOutcome::Applied,
                    Some(failed) if batch_index == failed => OperationOutcome::Rejected,
                    Some(_) => OperationOutcome::NotAttempted,
                    None => OperationOutcome::Rejected,
                };
                let mut result = OperationResult::new(request.operations[*operation_index].operation_id(), outcome);
                if matches!(outcome, OperationOutcome::Rejected | OperationOutcome::Unknown) {
                    result.message = Some(error.message.clone());
                }
                results[*operation_index] = Some(result);
            }
            true
        }
    }
}

fn fill_not_attempted(request: &ApplyChangesRequest, results: &mut [Option<OperationResult>]) {
    for (index, result) in results.iter_mut().enumerate() {
        if result.is_none() {
            *result =
                Some(OperationResult::new(request.operations[index].operation_id(), OperationOutcome::NotAttempted));
        }
    }
}

async fn read_back_applied(
    adapter: &FeishuBaseAdapter,
    table_id: &str,
    fields: &[BaseField],
    updates: &[PreparedUpdate],
    inserts: &[PreparedInsert],
    results: &mut [OperationResult],
) -> Result<String, ExternalTableError> {
    let mut record_ids = updates
        .iter()
        .filter(|update| results[update.operation_index].outcome == OperationOutcome::Applied)
        .map(|update| update.record_id.clone())
        .collect::<Vec<_>>();
    for insert in inserts {
        if results[insert.operation_index].outcome == OperationOutcome::Applied {
            if let Some(record_id) = results[insert.operation_index]
                .created_row_key
                .as_deref()
                .and_then(|key| decode_key(key, ROW_KEY_PREFIX, "Feishu Base record").ok())
            {
                record_ids.push(record_id);
            }
        }
    }
    record_ids.sort();
    record_ids.dedup();
    let field_ids = fields.iter().map(|field| field.field_id.clone()).collect::<Vec<_>>();
    let mut readback_records = HashMap::new();
    let mut revision = None;
    for chunk in record_ids.chunks(MAX_BATCH_SIZE) {
        let page = adapter.fetch_records_by_ids(table_id, chunk, &field_ids).await?;
        if page.incomplete {
            return Err(ExternalTableError::invalid("Feishu Base record readback was incomplete"));
        }
        if let Some(page_revision) = page.revision.filter(|value| !value.is_empty()) {
            if revision.as_ref().is_some_and(|revision| revision != &page_revision) {
                return Err(ExternalTableError::invalid(
                    "Feishu Base revision changed while reading back applied operations",
                ));
            }
            revision = Some(page_revision);
        }
        for record in page.records {
            readback_records.insert(record.record_id.clone(), record);
        }
    }
    for update in updates {
        if results[update.operation_index].outcome != OperationOutcome::Applied {
            continue;
        }
        let matches = fields
            .iter()
            .find(|field| field.field_id == update.field_id)
            .and_then(|field| readback_records.get(&update.record_id).map(|record| record_value(record, field)))
            .is_some_and(|value| value == update.new_value);
        if !matches {
            results[update.operation_index].message =
                Some("Feishu Base acknowledged the update, but readback differs; reload required".to_string());
        }
    }
    for insert in inserts {
        if results[insert.operation_index].outcome != OperationOutcome::Applied {
            continue;
        }
        let exists = results[insert.operation_index]
            .created_row_key
            .as_deref()
            .and_then(|key| decode_key(key, ROW_KEY_PREFIX, "Feishu Base record").ok())
            .is_some_and(|record_id| readback_records.contains_key(&record_id));
        if !exists {
            results[insert.operation_index].message =
                Some("Feishu Base returned a record ID, but created-record readback was incomplete".to_string());
        }
    }
    if let Some(revision) = revision.filter(|revision| !revision.is_empty()) {
        return Ok(revision);
    }
    let metadata = adapter.metadata(table_id).await?;
    let snapshot = adapter.current_snapshot(table_id, &metadata).await?;
    snapshot_parts(&snapshot)
        .map(|(_, revision, _)| revision)
        .ok_or_else(|| ExternalTableError::invalid("Feishu Base readback did not return a revision"))
}

fn annotate_applied_readback_failure(results: &mut [OperationResult], message: &str) {
    for result in results.iter_mut().filter(|result| result.outcome == OperationOutcome::Applied) {
        result.message = Some(format!("Feishu Base acknowledged the operation, but readback failed: {message}"));
    }
}

fn prepare_insert_fields(
    values: &[ExternalCellInput],
    field_by_id: &HashMap<&str, &BaseField>,
) -> Result<Map<String, Value>, ExternalTableError> {
    let mut fields = Map::new();
    for cell in values {
        let field_id = decode_key(&cell.column_key, COLUMN_KEY_PREFIX, "Feishu Base field")?;
        let field = field_by_id
            .get(field_id.as_str())
            .ok_or_else(|| ExternalTableError::invalid(format!("Feishu Base field no longer exists: {field_id}")))?;
        if !field.kind.writable() {
            return Err(ExternalTableError::invalid(format!("Feishu Base field '{}' is read-only", field.name)));
        }
        if let Err(message) = field.kind.validate_value(&cell.value) {
            return Err(ExternalTableError::invalid(format!("Feishu Base field '{}': {message}", field.name)));
        }
        if fields.insert(field_id, cell.value.clone()).is_some() {
            return Err(ExternalTableError::invalid("Feishu Base insert contains a duplicate field"));
        }
    }
    Ok(fields)
}

fn parse_table(value: &Value) -> Option<BaseTable> {
    let table_id =
        value.get("id").or_else(|| value.get("table_id")).and_then(Value::as_str).filter(|value| !value.is_empty())?;
    let name = value
        .get("name")
        .or_else(|| value.get("table_name"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(table_id);
    Some(BaseTable { table_id: table_id.to_string(), name: name.to_string() })
}

fn parse_field(value: &Value) -> Option<BaseField> {
    let field_id =
        value.get("id").or_else(|| value.get("field_id")).and_then(Value::as_str).filter(|value| !value.is_empty())?;
    let name = value
        .get("name")
        .or_else(|| value.get("field_name"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(field_id);
    Some(BaseField { field_id: field_id.to_string(), name: name.to_string(), kind: field_kind(value) })
}

fn field_kind(value: &Value) -> BaseFieldKind {
    let field_type = string_value(value.get("type").or_else(|| value.get("field_type"))).to_ascii_lowercase();
    let ui_type = string_value(value.get("ui_type")).to_ascii_lowercase();
    let style_type = value
        .get("style")
        .and_then(|style| style.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let effective = if !ui_type.is_empty() { ui_type.as_str() } else { field_type.as_str() };
    match effective {
        "url" | "phone" | "email" => BaseFieldKind::Text,
        "currency" | "percent" | "percentage" | "progress" | "rating" => BaseFieldKind::Number,
        "single_select" | "single-select" | "singleselect" => BaseFieldKind::Select { multiple: false },
        "multi_select" | "multi-select" | "multiselect" => BaseFieldKind::Select { multiple: true },
        "text" if matches!(style_type.as_str(), "" | "plain" | "url" | "phone" | "email") => BaseFieldKind::Text,
        "number"
            if matches!(
                style_type.as_str(),
                "" | "number" | "currency" | "percent" | "percentage" | "progress" | "rating"
            ) =>
        {
            BaseFieldKind::Number
        }
        "select" => BaseFieldKind::Select { multiple: value.get("multiple").and_then(Value::as_bool).unwrap_or(false) },
        "datetime" | "date" | "date_time" => BaseFieldKind::DateTime,
        "checkbox" => BaseFieldKind::Checkbox,
        _ => BaseFieldKind::Readonly,
    }
}

fn parse_records_page(data: &Value) -> Result<RecordsPage, ExternalTableError> {
    if data.get("field_id_list").is_some() || data.get("record_id_list").is_some() || data.get("data").is_some() {
        return parse_record_matrix(data);
    }
    if let Some(records) = data.get("records").and_then(Value::as_object) {
        return parse_legacy_record_matrix(records, data);
    }
    let raw_records =
        data.get("records").or_else(|| data.get("items")).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut records = Vec::new();
    let mut malformed = false;
    for value in &raw_records {
        let Some(record_id) = parse_record_id(value) else {
            malformed = true;
            continue;
        };
        let fields = value.get("fields").and_then(Value::as_object).cloned().unwrap_or_default();
        records.push(BaseRecord { record_id, fields });
    }
    let revision =
        data.get("rev").or_else(|| data.get("revision")).map(stable_string).filter(|value| !value.is_empty());
    let has_more = bool_value(data.get("has_more")).unwrap_or(false);
    Ok(RecordsPage { incomplete: malformed || (has_more && records.is_empty()), records, revision, has_more })
}

fn parse_record_matrix(data: &Value) -> Result<RecordsPage, ExternalTableError> {
    let names = string_array(data.get("fields"), "fields")?;
    let field_ids = string_array(data.get("field_id_list"), "field_id_list")?;
    let record_ids = string_array(data.get("record_id_list"), "record_id_list")?;
    let rows = data
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ExternalTableError::invalid("Feishu Base record matrix is missing data rows"))?;
    if names.len() != field_ids.len() {
        return Err(ExternalTableError::invalid(
            "Feishu Base record matrix field names and field IDs have different lengths",
        ));
    }
    if record_ids.len() != rows.len() {
        return Err(ExternalTableError::invalid(
            "Feishu Base record matrix record IDs and rows have different lengths",
        ));
    }
    let mut records = Vec::with_capacity(rows.len());
    for (record_id, row) in record_ids.into_iter().zip(rows) {
        let row = row
            .as_array()
            .ok_or_else(|| ExternalTableError::invalid("Feishu Base record matrix row is not an array"))?;
        if row.len() != field_ids.len() {
            return Err(ExternalTableError::invalid(
                "Feishu Base record matrix row width does not match the field schema",
            ));
        }
        let fields = field_ids.iter().cloned().zip(row.iter().cloned()).collect::<Map<_, _>>();
        records.push(BaseRecord { record_id, fields });
    }
    let revision =
        data.get("rev").or_else(|| data.get("revision")).map(stable_string).filter(|value| !value.is_empty());
    let has_more = bool_value(data.get("has_more")).unwrap_or(false);
    let ignored = data.get("ignored_fields").and_then(Value::as_array).is_some_and(|values| !values.is_empty());
    Ok(RecordsPage { incomplete: ignored || (has_more && records.is_empty()), records, revision, has_more })
}

fn parse_legacy_record_matrix(
    records: &Map<String, Value>,
    envelope: &Value,
) -> Result<RecordsPage, ExternalTableError> {
    let names = string_array(records.get("schema"), "records.schema")?;
    let record_ids = string_array(records.get("record_ids"), "records.record_ids")?;
    let rows = records
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| ExternalTableError::invalid("Feishu Base legacy record matrix is missing rows"))?;
    if record_ids.len() != rows.len() {
        return Err(ExternalTableError::invalid(
            "Feishu Base legacy record matrix record IDs and rows have different lengths",
        ));
    }
    let mut parsed = Vec::with_capacity(rows.len());
    for (record_id, row) in record_ids.into_iter().zip(rows) {
        let row = row
            .as_array()
            .ok_or_else(|| ExternalTableError::invalid("Feishu Base legacy record matrix row is not an array"))?;
        if row.len() != names.len() {
            return Err(ExternalTableError::invalid(
                "Feishu Base legacy record matrix row width does not match its schema",
            ));
        }
        parsed.push(BaseRecord { record_id, fields: names.iter().cloned().zip(row.iter().cloned()).collect() });
    }
    let revision =
        envelope.get("rev").or_else(|| records.get("rev")).map(stable_string).filter(|value| !value.is_empty());
    let has_more = bool_value(envelope.get("has_more").or_else(|| records.get("has_more"))).unwrap_or(false);
    Ok(RecordsPage { incomplete: has_more && parsed.is_empty(), records: parsed, revision, has_more })
}

fn parse_record_id(value: &Value) -> Option<String> {
    value
        .get("record_id")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn string_array(value: Option<&Value>, label: &str) -> Result<Vec<String>, ExternalTableError> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| ExternalTableError::invalid(format!("Feishu Base record matrix is missing {label}")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| ExternalTableError::invalid(format!("Feishu Base record matrix {label} is invalid")))
        })
        .collect()
}

fn columns(fields: &[BaseField]) -> Vec<ExternalColumn> {
    fields
        .iter()
        .map(|field| ExternalColumn {
            column_key: column_key(&field.field_id),
            display_name: field.name.clone(),
            value_type: field.kind.value_type(),
            writable: field.kind.writable(),
        })
        .collect()
}

fn record_value(record: &BaseRecord, field: &BaseField) -> Value {
    record.fields.get(&field.field_id).or_else(|| record.fields.get(&field.name)).cloned().unwrap_or(Value::Null)
}

fn schema_digest(table: &BaseTable, fields: &[BaseField]) -> String {
    let normalized = json!({
        "tableId": table.table_id,
        "name": table.name,
        "fields": fields.iter().map(|field| {
            json!({
                "fieldId": field.field_id,
                "name": field.name,
                "kind": format!("{:?}", field.kind),
            })
        }).collect::<Vec<_>>()
    });
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&normalized).expect("normalized Base metadata is serializable"));
    format!("{:x}", hasher.finalize())
}

fn snapshot_token(table_id: &str, revision: &str, schema_digest: &str) -> String {
    format!("base:{}:rev:{}:schema:{schema_digest}", encode_path_segment(table_id), encode_path_segment(revision))
}

fn snapshot_parts(snapshot: &str) -> Option<(String, String, String)> {
    let tail = snapshot.strip_prefix("base:")?;
    let (encoded_table, tail) = tail.split_once(":rev:")?;
    let (encoded_revision, schema_digest) = tail.rsplit_once(":schema:")?;
    if encoded_table.is_empty() || encoded_revision.is_empty() || schema_digest.is_empty() {
        return None;
    }
    let table_id = percent_encoding::percent_decode_str(encoded_table).decode_utf8().ok()?.into_owned();
    let revision = percent_encoding::percent_decode_str(encoded_revision).decode_utf8().ok()?.into_owned();
    Some((table_id, revision, schema_digest.to_string()))
}

fn row_key(record_id: &str) -> String {
    format!("{ROW_KEY_PREFIX}{}", encode_path_segment(record_id))
}

fn column_key(field_id: &str) -> String {
    format!("{COLUMN_KEY_PREFIX}{}", encode_path_segment(field_id))
}

fn encode_path_segment(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn decode_key(key: &str, prefix: &str, label: &str) -> Result<String, ExternalTableError> {
    let encoded =
        key.strip_prefix(prefix).ok_or_else(|| ExternalTableError::invalid(format!("Invalid {label} key: {key}")))?;
    let decoded = percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .map_err(|_| ExternalTableError::invalid(format!("Invalid {label} key: {key}")))?
        .into_owned();
    if decoded.is_empty() {
        return Err(ExternalTableError::invalid(format!("Invalid {label} key: {key}")));
    }
    Ok(decoded)
}

fn parse_cursor(cursor: Option<&str>) -> Result<usize, ExternalTableError> {
    match cursor {
        None | Some("") => Ok(0),
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| ExternalTableError::invalid(format!("Invalid Feishu Base cursor: {cursor}"))),
    }
}

fn string_value(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().trim().to_string()
}

fn stable_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn usize_value(value: Option<&Value>) -> Option<usize> {
    value.and_then(|value| {
        value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn bool_value(value: Option<&Value>) -> Option<bool> {
    value.and_then(|value| value.as_bool().or_else(|| value.as_str().and_then(|value| value.parse().ok())))
}

fn failure_indexes(value: &Value) -> HashSet<usize> {
    fn visit(value: &Value, indexes: &mut HashSet<usize>) {
        if let Some(failures) = value.get("failures").or_else(|| value.get("errors")).and_then(Value::as_array) {
            for failure in failures {
                if let Some(index) = usize_value(failure.get("index")) {
                    indexes.insert(index);
                }
            }
        }
        match value {
            Value::String(value) => {
                if let Ok(nested) = serde_json::from_str::<Value>(value) {
                    visit(&nested, indexes);
                }
            }
            Value::Array(values) => values.iter().for_each(|value| visit(value, indexes)),
            Value::Object(values) => values.values().for_each(|value| visit(value, indexes)),
            _ => {}
        }
    }
    let mut indexes = HashSet::new();
    visit(value, &mut indexes);
    indexes
}

fn failure_indexes_from_message(message: &str) -> HashSet<usize> {
    message
        .find('{')
        .and_then(|start| serde_json::from_str::<Value>(&message[start..]).ok())
        .map(|value| failure_indexes(&value))
        .unwrap_or_default()
}

fn ignored_field_keys(value: &Value) -> HashSet<String> {
    value
        .get("ignored_fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|field| match field {
            Value::String(value) => Some(value.clone()),
            Value::Object(field) => field
                .get("id")
                .or_else(|| field.get("field_id"))
                .or_else(|| field.get("name"))
                .or_else(|| field.get("field_name"))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::feishu::test_support::{serve, MockReply};
    use super::*;

    fn token_reply() -> MockReply {
        MockReply::Json(
            json!({ "code": 0, "msg": "ok", "tenant_access_token": "tenant-token", "expire": 7200 }).to_string(),
        )
    }

    fn api_reply(data: Value) -> MockReply {
        MockReply::Json(json!({ "code": 0, "msg": "ok", "data": data }).to_string())
    }

    fn table_reply() -> MockReply {
        api_reply(json!({ "id": "tbl1", "name": "Tasks" }))
    }

    fn fields_value(include_formula: bool) -> Value {
        let mut fields = vec![
            json!({ "id": "fld_name", "name": "Name", "type": "text", "style": { "type": "plain" } }),
            json!({ "id": "fld_amount", "name": "Amount", "type": "number", "style": { "type": "number" } }),
        ];
        if include_formula {
            fields.push(json!({ "id": "fld_formula", "name": "Computed", "type": "formula" }));
        }
        json!({ "fields": fields, "total": fields.len(), "has_more": false })
    }

    fn fields_reply(include_formula: bool) -> MockReply {
        api_reply(fields_value(include_formula))
    }

    fn matrix_reply(
        revision: u64,
        field_ids: &[&str],
        field_names: &[&str],
        record_ids: &[&str],
        rows: Value,
        has_more: bool,
    ) -> MockReply {
        api_reply(json!({
            "timezone": "Asia/Shanghai",
            "fields": field_names,
            "field_id_list": field_ids,
            "field_type_list": field_ids.iter().map(|_| "text").collect::<Vec<_>>(),
            "record_id_list": record_ids,
            "data": rows,
            "rev": revision,
            "has_more": has_more,
            "ignored_fields": []
        }))
    }

    fn adapter(client: FeishuClient) -> FeishuBaseAdapter {
        FeishuBaseAdapter::from_client(
            client,
            FeishuBaseExternalConfig {
                base_token: "base-token".to_string(),
                table_id: Some("tbl1".to_string()),
                view_id: Some("view1".to_string()),
            },
        )
        .unwrap()
    }

    fn table_ref() -> ExternalTableRef {
        ExternalTableRef { table_key: "table:tbl1".to_string(), display_name: "Tasks".to_string() }
    }

    fn snapshot(revision: &str, include_formula: bool) -> String {
        let table = BaseTable { table_id: "tbl1".to_string(), name: "Tasks".to_string() };
        let fields = fields_value(include_formula)
            .get("fields")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(parse_field)
            .collect::<Vec<_>>();
        snapshot_token("tbl1", revision, &schema_digest(&table, &fields))
    }

    async fn finish_server(server: tokio::task::JoinHandle<Vec<String>>) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("mock server received the expected request count")
            .unwrap()
    }

    #[test]
    fn select_cell_values_use_option_name_arrays_for_single_and_multi_select() {
        let single = field_kind(&json!({ "type": "select", "multiple": false }));
        let multiple = field_kind(&json!({ "type": "select", "multiple": true }));

        assert!(single.validate_value(&json!(["Todo"])).is_ok());
        assert!(single.validate_value(&json!(["Todo", "Done"])).is_err());
        assert!(single.validate_value(&json!("Todo")).is_err());
        assert!(multiple.validate_value(&json!(["Todo", "Done"])).is_ok());
    }

    #[tokio::test]
    async fn base_read_uses_record_and_field_ids_and_marks_formula_readonly() {
        let (base_url, server) = serve(vec![
            token_reply(),
            table_reply(),
            fields_reply(true),
            matrix_reply(
                7,
                &["fld_name", "fld_amount", "fld_formula"],
                &["Name", "Amount", "Computed"],
                &["rec1"],
                json!([["Ada", 3, "=Amount*2"]]),
                true,
            ),
            table_reply(),
            fields_reply(true),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();

        let page =
            adapter(client).read_page(ReadPageRequest { table: table_ref(), cursor: None, limit: 1 }).await.unwrap();

        let requests = finish_server(server).await;
        assert_eq!(page.rows[0].row_key, "record:rec1");
        assert_eq!(page.columns[0].column_key, "field:fld%5Fname");
        assert!(page.rows[0].readonly_column_keys.contains(&"field:fld%5Fformula".to_string()));
        assert_eq!(page.next_cursor.as_deref(), Some("1"));
        assert_eq!(page.read_state, ReadState::Complete);
        assert!(requests[3].contains("view_id=view1"));
        assert!(requests[3].contains("field_id=fld_name"));
    }

    #[tokio::test]
    async fn base_read_marks_schema_changes_incomplete() {
        let changed_fields = json!({
            "fields": [
                { "id": "fld_name", "name": "Renamed", "type": "text", "style": { "type": "plain" } },
                { "id": "fld_amount", "name": "Amount", "type": "number", "style": { "type": "number" } }
            ],
            "total": 2,
            "has_more": false
        });
        let (base_url, server) = serve(vec![
            token_reply(),
            table_reply(),
            fields_reply(false),
            matrix_reply(7, &["fld_name", "fld_amount"], &["Name", "Amount"], &["rec1"], json!([["Ada", 3]]), false),
            table_reply(),
            api_reply(changed_fields),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();

        let page =
            adapter(client).read_page(ReadPageRequest { table: table_ref(), cursor: None, limit: 20 }).await.unwrap();

        finish_server(server).await;
        assert_eq!(page.read_state, ReadState::Incomplete);
    }

    #[tokio::test]
    async fn base_crud_batches_use_field_ids_and_read_back_created_record() {
        let (base_url, server) = serve(vec![
            token_reply(),
            table_reply(),
            fields_reply(false),
            matrix_reply(7, &["fld_name", "fld_amount"], &["Name", "Amount"], &["rec1"], json!([["Ada", 3]]), false),
            matrix_reply(7, &["fld_name"], &["Name"], &["rec1"], json!([["Ada"]]), false),
            api_reply(json!({})),
            api_reply(json!({ "records": [{ "record_id": "rec2", "deleted": true }] })),
            api_reply(json!({ "record_id_list": ["rec3"] })),
            matrix_reply(
                8,
                &["fld_name", "fld_amount"],
                &["Name", "Amount"],
                &["rec1", "rec3"],
                json!([["Ada Lovelace", 3], ["Grace", 5]]),
                false,
            ),
            table_reply(),
            fields_reply(false),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();

        let result = adapter(client)
            .apply_changes(ApplyChangesRequest {
                table: table_ref(),
                snapshot_token: snapshot("7", false),
                operations: vec![
                    ExternalOperation::Update {
                        operation_id: "update".to_string(),
                        row_key: "record:rec1".to_string(),
                        column_key: "field:fld_name".to_string(),
                        old_value: Value::String("Ada".to_string()),
                        new_value: Value::String("Ada Lovelace".to_string()),
                    },
                    ExternalOperation::Delete {
                        operation_id: "delete".to_string(),
                        row_key: "record:rec2".to_string(),
                    },
                    ExternalOperation::Insert {
                        operation_id: "insert".to_string(),
                        values: vec![
                            ExternalCellInput {
                                column_key: "field:fld_name".to_string(),
                                value: Value::String("Grace".to_string()),
                            },
                            ExternalCellInput {
                                column_key: "field:fld_amount".to_string(),
                                value: Value::Number(5.into()),
                            },
                        ],
                    },
                ],
            })
            .await
            .unwrap();

        let requests = finish_server(server).await;
        assert!(result.operation_results.iter().all(|result| result.outcome == OperationOutcome::Applied));
        assert_eq!(result.operation_results[2].created_row_key.as_deref(), Some("record:rec3"));
        assert!(result.new_snapshot_token.as_deref().is_some_and(|snapshot| snapshot.contains(":rev:8:")));
        assert!(requests[5].contains("/records/batch_update"));
        assert!(requests[5].contains("update_records"));
        assert!(requests[5].contains("fld_name"));
        assert!(requests[7].contains("/records/batch_create"));
        assert!(requests[7].contains("create_records"));
        assert!(requests[7].contains("fld_amount"));
    }

    #[tokio::test]
    async fn base_partial_update_stops_later_create_and_preserves_applied_result() {
        let (base_url, server) = serve(vec![
            token_reply(),
            table_reply(),
            fields_reply(false),
            matrix_reply(7, &["fld_name", "fld_amount"], &["Name", "Amount"], &["rec1"], json!([["Ada", 3]]), false),
            matrix_reply(7, &["fld_name", "fld_amount"], &["Name", "Amount"], &["rec1"], json!([["Ada", 3]]), false),
            api_reply(json!({ "ignored_fields": [{ "id": "fld_amount", "reason": "read only" }] })),
            matrix_reply(8, &["fld_name", "fld_amount"], &["Name", "Amount"], &["rec1"], json!([["A", 3]]), false),
            table_reply(),
            fields_reply(false),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();
        let update = |id: &str, field: &str, old: Value, new: Value| ExternalOperation::Update {
            operation_id: id.to_string(),
            row_key: "record:rec1".to_string(),
            column_key: format!("field:{field}"),
            old_value: old,
            new_value: new,
        };

        let result = adapter(client)
            .apply_changes(ApplyChangesRequest {
                table: table_ref(),
                snapshot_token: snapshot("7", false),
                operations: vec![
                    update("first", "fld_name", json!("Ada"), json!("A")),
                    update("second", "fld_amount", json!(3), json!(4)),
                    ExternalOperation::Insert { operation_id: "later".to_string(), values: vec![] },
                ],
            })
            .await
            .unwrap();

        finish_server(server).await;
        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Applied);
        assert_eq!(result.operation_results[1].outcome, OperationOutcome::Rejected);
        assert_eq!(result.operation_results[2].outcome, OperationOutcome::NotAttempted);
    }

    #[tokio::test]
    async fn base_unknown_create_is_not_retried_and_blocks_save() {
        let (base_url, server) = serve(vec![
            token_reply(),
            table_reply(),
            fields_reply(false),
            matrix_reply(7, &["fld_name", "fld_amount"], &["Name", "Amount"], &[], json!([]), false),
            MockReply::DropConnection,
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app", "secret", Duration::from_secs(5)).unwrap();

        let result = adapter(client)
            .apply_changes(ApplyChangesRequest {
                table: table_ref(),
                snapshot_token: snapshot("7", false),
                operations: vec![ExternalOperation::Insert {
                    operation_id: "insert".to_string(),
                    values: vec![ExternalCellInput {
                        column_key: "field:fld_name".to_string(),
                        value: Value::String("Ada".to_string()),
                    }],
                }],
            })
            .await
            .unwrap();

        let requests = finish_server(server).await;
        assert_eq!(requests.len(), 5, "unknown create must not be retried");
        assert_eq!(result.operation_results[0].outcome, OperationOutcome::Unknown);
        assert!(result.save_blocked);
    }
}
