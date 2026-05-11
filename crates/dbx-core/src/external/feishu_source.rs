use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use super::traits::ExternalTabularSource;
use super::types::*;

const DEFAULT_FEISHU_BASE_URL: &str = "https://open.feishu.cn";
const BITABLE_BATCH_CREATE_LIMIT: usize = 500;
const BITABLE_BATCH_UPDATE_LIMIT: usize = 1000;
const BITABLE_BATCH_DELETE_LIMIT: usize = 500;

#[derive(Clone)]
struct FeishuClient {
    http: reqwest::Client,
    base_url: String,
    app_id: String,
    app_secret: String,
    access_token: Option<String>,
    cached_tenant_token: Arc<Mutex<Option<CachedToken>>>,
}

#[derive(Clone)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

impl std::fmt::Debug for FeishuClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuClient").field("base_url", &self.base_url).finish_non_exhaustive()
    }
}

impl FeishuClient {
    fn new(base_url: &str, app_id: &str, app_secret: &str, access_token: Option<String>) -> Self {
        let base_url = if base_url.trim().is_empty() {
            DEFAULT_FEISHU_BASE_URL.to_string()
        } else {
            base_url.trim().trim_end_matches('/').to_string()
        };

        Self {
            http: reqwest::Client::new(),
            base_url,
            app_id: app_id.trim().to_string(),
            app_secret: app_secret.trim().to_string(),
            access_token: clean_access_token(access_token),
            cached_tenant_token: Arc::new(Mutex::new(None)),
        }
    }

    async fn bearer_token(&self) -> Result<String, String> {
        if let Some(token) = self.access_token.as_deref().filter(|token| !token.trim().is_empty()) {
            return Ok(normalize_access_token_value(token));
        }

        {
            let cached = self.cached_tenant_token.lock().await;
            if let Some(token) = cached.as_ref().filter(|token| token.expires_at > Instant::now()) {
                return Ok(token.token.clone());
            }
        }

        if self.app_id.is_empty() || self.app_secret.is_empty() {
            return Err("Feishu App ID/App Secret or an access token is required".to_string());
        }

        #[derive(Deserialize)]
        struct TenantTokenResponse {
            code: i64,
            msg: Option<String>,
            tenant_access_token: Option<String>,
            expire: Option<u64>,
        }

        let url = format!("{}/open-apis/auth/v3/tenant_access_token/internal", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "app_secret": self.app_secret,
            }))
            .send()
            .await
            .map_err(|e| format!("Feishu token request failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| format!("Feishu token response read failed: {e}"))?;
        if !status.is_success() {
            return Err(format!("Feishu token request failed with HTTP {status}: {text}"));
        }

        let parsed: TenantTokenResponse =
            serde_json::from_str(&text).map_err(|e| format!("Invalid Feishu token response: {e}; body={text}"))?;
        if parsed.code != 0 {
            return Err(format!(
                "Feishu token request failed: code={} msg={}",
                parsed.code,
                parsed.msg.unwrap_or_default()
            ));
        }

        let token = parsed.tenant_access_token.ok_or("Feishu token response missing tenant_access_token")?;
        let ttl = parsed.expire.unwrap_or(7200).saturating_sub(300).max(60);
        let cached = CachedToken { token: token.clone(), expires_at: Instant::now() + Duration::from_secs(ttl) };
        *self.cached_tenant_token.lock().await = Some(cached);
        Ok(token)
    }

    async fn get_data<T: DeserializeOwned>(&self, path: &str, query: &[(String, String)]) -> Result<T, String> {
        self.request_data(Method::GET, path, query, None).await
    }

    async fn post_data<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
        body: Value,
    ) -> Result<T, String> {
        self.request_data(Method::POST, path, query, Some(body)).await
    }

    async fn put_data<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
        body: Value,
    ) -> Result<T, String> {
        self.request_data(Method::PUT, path, query, Some(body)).await
    }

    async fn request_data<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<T, String> {
        #[derive(Deserialize)]
        struct Envelope<T> {
            code: i64,
            msg: Option<String>,
            data: Option<T>,
        }

        let token = self.bearer_token().await?;
        let url = format!("{}{}", self.base_url, path);
        let mut request = self
            .http
            .request(method, &url)
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json; charset=utf-8");
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().await.map_err(|e| format!("Feishu request failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| format!("Feishu response read failed: {e}"))?;
        if !status.is_success() {
            return Err(format!("Feishu request failed with HTTP {status}: {text}"));
        }

        let envelope: Envelope<T> =
            serde_json::from_str(&text).map_err(|e| format!("Invalid Feishu response: {e}; body={text}"))?;
        if envelope.code != 0 {
            return Err(format!("Feishu API error: code={} msg={}", envelope.code, envelope.msg.unwrap_or_default()));
        }
        envelope.data.ok_or_else(|| "Feishu response missing data".to_string())
    }
}

#[derive(Debug)]
pub struct FeishuSheetsSource {
    client: FeishuClient,
    config: FeishuSheetsExternalConfig,
}

impl FeishuSheetsSource {
    pub fn new(base_url: &str, app_id: &str, app_secret: &str, config: FeishuSheetsExternalConfig) -> Self {
        let client = FeishuClient::new(base_url, app_id, app_secret, config.access_token.clone());
        Self { client, config }
    }

    async fn list_sheet_metadata(&self) -> Result<Vec<FeishuSheet>, String> {
        if self.config.spreadsheet_token.trim().is_empty() {
            return Err("Feishu spreadsheet token is required".to_string());
        }

        let path = format!(
            "/open-apis/sheets/v3/spreadsheets/{}/sheets/query",
            encode_path_segment(&self.config.spreadsheet_token)
        );
        let data: SheetQueryData = self.client.get_data(&path, &[]).await?;
        let target_sheet = clean_optional(self.config.sheet_id.clone());

        let sheets = data
            .sheets
            .into_iter()
            .filter(|sheet| sheet.resource_type.as_deref().unwrap_or("sheet") == "sheet")
            .filter(|sheet| target_sheet.as_deref().map_or(true, |target| sheet.sheet_id == target))
            .collect::<Vec<_>>();

        if sheets.is_empty() {
            if let Some(target) = target_sheet {
                return Err(format!("Feishu sheet not found: {target}"));
            }
        }

        Ok(sheets)
    }

    async fn read_values(&self, sheet: &FeishuSheet) -> Result<(Vec<Vec<Value>>, String), String> {
        let range = self.read_range(sheet);
        let path = format!(
            "/open-apis/sheets/v2/spreadsheets/{}/values/{}",
            encode_path_segment(&self.config.spreadsheet_token),
            encode_path_segment(&range)
        );
        let mut query = Vec::new();
        if let Some(value) = clean_optional(self.config.value_render_option.clone()) {
            query.push(("valueRenderOption".to_string(), value));
        }
        if let Some(value) = clean_optional(self.config.date_time_render_option.clone()) {
            query.push(("dateTimeRenderOption".to_string(), value));
        }

        let data: SheetValuesData = self.client.get_data(&path, &query).await?;
        let value_range = data.value_range.unwrap_or_default();
        let version = value_range
            .revision
            .or(data.revision)
            .map(|revision| format!("revision:{revision}"))
            .unwrap_or_else(|| "revision:unknown".to_string());
        Ok((value_range.values.unwrap_or_default(), version))
    }

    fn read_range(&self, sheet: &FeishuSheet) -> String {
        if let Some(range) = clean_optional(self.config.range.clone()) {
            if range.contains('!') {
                return range;
            }
            return format!("{}!{}", sheet.sheet_id, range);
        }

        let rows = sheet
            .grid_properties
            .as_ref()
            .and_then(|grid| grid.row_count)
            .unwrap_or(self.config.max_rows)
            .max(1)
            .min(self.config.max_rows.max(1));
        let columns = sheet
            .grid_properties
            .as_ref()
            .and_then(|grid| grid.column_count)
            .unwrap_or(self.config.max_columns)
            .max(1)
            .min(self.config.max_columns.max(1));
        format!("{}!A1:{}{}", sheet.sheet_id, column_label(columns), rows)
    }

    async fn write_sheet_range(
        &self,
        sheet_id: &str,
        range: &str,
        rows: Vec<Vec<Value>>,
    ) -> Result<ExternalWriteResult, String> {
        let write_range = normalize_sheet_write_range(sheet_id, range, &rows);
        let path =
            format!("/open-apis/sheets/v2/spreadsheets/{}/values", encode_path_segment(&self.config.spreadsheet_token));
        let body = serde_json::json!({
            "valueRange": {
                "range": write_range,
                "values": rows,
            }
        });
        let data: Value = self.client.put_data(&path, &[], body).await?;
        let affected_rows = data.get("updatedRows").and_then(Value::as_u64).unwrap_or(0) as usize;
        Ok(ExternalWriteResult { affected_rows, raw: data })
    }
}

#[async_trait]
impl ExternalTabularSource for FeishuSheetsSource {
    fn capabilities(&self) -> ExternalCapabilities {
        ExternalCapabilities {
            can_read: true,
            can_write: true,
            can_append: true,
            can_delete_rows: false,
            supports_multiple_tables: self.config.sheet_id.as_ref().map_or(true, |value| value.trim().is_empty()),
            supports_refresh: true,
            supports_file_watch: false,
            supports_schema_detection: true,
        }
    }

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, String> {
        Ok(self
            .list_sheet_metadata()
            .await?
            .into_iter()
            .map(|sheet| {
                let title = clean_title(&sheet.title).unwrap_or_else(|| sheet.sheet_id.clone());
                ExternalTableRef { source_id: sheet.sheet_id, table_name: title.clone(), display_name: title }
            })
            .collect())
    }

    async fn get_columns(&self, table: &ExternalTableRef) -> Result<Vec<ExternalColumnDef>, String> {
        Ok(self.load_table(table).await?.columns)
    }

    async fn load_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSnapshot, String> {
        let sheets = self.list_sheet_metadata().await?;
        let sheet = sheets
            .into_iter()
            .find(|sheet| sheet.sheet_id == table.source_id)
            .ok_or_else(|| format!("Feishu sheet not found: {}", table.source_id))?;
        let (values, source_version) = self.read_values(&sheet).await?;
        Ok(sheet_values_to_snapshot(table.clone(), values, self.config.has_header, source_version))
    }

    async fn source_version(&self, table: &ExternalTableRef) -> Result<String, String> {
        let sheets = self.list_sheet_metadata().await?;
        let sheet = sheets
            .into_iter()
            .find(|sheet| sheet.sheet_id == table.source_id)
            .ok_or_else(|| format!("Feishu sheet not found: {}", table.source_id))?;
        let (_, version) = self.read_values(&sheet).await?;
        Ok(version)
    }

    async fn test_connection(&self) -> Result<String, String> {
        let sheets = self.list_sheet_metadata().await?;
        Ok(format!("Feishu Sheets connected: {} sheet(s)", sheets.len()))
    }

    fn display_name(&self) -> String {
        format!("Feishu Sheets: {}", self.config.spreadsheet_token)
    }

    fn refresh_before_query(&self) -> bool {
        self.config.sync_mode == ExternalSyncMode::Realtime
    }

    async fn append_rows(
        &self,
        table: &ExternalTableRef,
        rows: Vec<Vec<Value>>,
    ) -> Result<ExternalWriteResult, String> {
        let append_range = self
            .config
            .range
            .as_deref()
            .filter(|range| !range.trim().is_empty())
            .map(|range| if range.contains('!') { range.to_string() } else { format!("{}!{range}", table.source_id) })
            .unwrap_or_else(|| table.source_id.clone());
        let path = format!(
            "/open-apis/sheets/v2/spreadsheets/{}/values_append",
            encode_path_segment(&self.config.spreadsheet_token)
        );
        let query = vec![("insertDataOption".to_string(), "INSERT_ROWS".to_string())];
        let body = serde_json::json!({
            "valueRange": {
                "range": append_range,
                "values": rows,
            }
        });
        let data: Value = self.client.post_data(&path, &query, body).await?;
        let affected_rows =
            data.get("updates").and_then(|updates| updates.get("updatedRows")).and_then(Value::as_u64).unwrap_or(0)
                as usize;
        Ok(ExternalWriteResult { affected_rows, raw: data })
    }

    async fn write_range(
        &self,
        table: &ExternalTableRef,
        range: &str,
        rows: Vec<Vec<Value>>,
    ) -> Result<ExternalWriteResult, String> {
        self.write_sheet_range(&table.source_id, range, rows).await
    }
}

#[derive(Debug)]
pub struct FeishuBitableSource {
    client: FeishuClient,
    config: FeishuBitableExternalConfig,
}

impl FeishuBitableSource {
    pub fn new(base_url: &str, app_id: &str, app_secret: &str, config: FeishuBitableExternalConfig) -> Self {
        let client = FeishuClient::new(base_url, app_id, app_secret, config.access_token.clone());
        Self { client, config }
    }

    async fn list_bitable_tables(&self) -> Result<Vec<BitableTable>, String> {
        if self.config.app_token.trim().is_empty() {
            return Err("Feishu Bitable app token is required".to_string());
        }

        let path = format!("/open-apis/bitable/v1/apps/{}/tables", encode_path_segment(&self.config.app_token));
        let target_table = clean_optional(self.config.table_id.clone());
        let mut page_token: Option<String> = None;
        let mut tables = Vec::new();

        loop {
            let mut query = vec![("page_size".to_string(), "100".to_string())];
            if let Some(token) = page_token.as_deref() {
                query.push(("page_token".to_string(), token.to_string()));
            }
            let data: BitableTablesData = self.client.get_data(&path, &query).await?;
            tables.extend(
                data.items
                    .into_iter()
                    .filter(|table| target_table.as_deref().map_or(true, |target| table.table_id == target)),
            );
            if !data.has_more.unwrap_or(false) {
                break;
            }
            page_token = clean_optional(data.page_token);
            if page_token.is_none() {
                break;
            }
        }

        if tables.is_empty() {
            if let Some(target) = target_table {
                return Err(format!("Feishu Bitable table not found: {target}"));
            }
        }
        Ok(tables)
    }

    async fn list_fields(&self, table_id: &str) -> Result<Vec<BitableField>, String> {
        let path = format!(
            "/open-apis/bitable/v1/apps/{}/tables/{}/fields",
            encode_path_segment(&self.config.app_token),
            encode_path_segment(table_id)
        );
        let wanted = wanted_field_names(&self.config.field_names);
        let mut page_token: Option<String> = None;
        let mut fields = Vec::new();

        loop {
            let mut query = vec![("page_size".to_string(), "100".to_string())];
            if let Some(view_id) = clean_optional(self.config.view_id.clone()) {
                query.push(("view_id".to_string(), view_id));
            }
            if let Some(token) = page_token.as_deref() {
                query.push(("page_token".to_string(), token.to_string()));
            }
            let data: BitableFieldsData = self.client.get_data(&path, &query).await?;
            fields.extend(data.items.into_iter().filter(|field| {
                wanted.as_ref().map_or(true, |wanted| wanted.iter().any(|name| name == &field.field_name))
            }));
            if !data.has_more.unwrap_or(false) {
                break;
            }
            page_token = clean_optional(data.page_token);
            if page_token.is_none() {
                break;
            }
        }

        Ok(fields)
    }

    async fn search_records(&self, table_id: &str, fields: &[BitableField]) -> Result<Vec<BitableRecord>, String> {
        let path = format!(
            "/open-apis/bitable/v1/apps/{}/tables/{}/records/search",
            encode_path_segment(&self.config.app_token),
            encode_path_segment(table_id)
        );
        let page_size = self.config.page_size.clamp(1, 500);
        let max_records = self.config.max_records.max(1);
        let field_names: Vec<String> = fields.iter().map(|field| field.field_name.clone()).collect();
        let mut records = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut query = vec![("page_size".to_string(), page_size.to_string())];
            if let Some(user_id_type) = clean_optional(self.config.user_id_type.clone()) {
                query.push(("user_id_type".to_string(), user_id_type));
            }
            if let Some(token) = page_token.as_deref() {
                query.push(("page_token".to_string(), token.to_string()));
            }

            let mut body = Map::new();
            body.insert("automatic_fields".to_string(), Value::Bool(self.config.automatic_fields));
            if let Some(view_id) = clean_optional(self.config.view_id.clone()) {
                body.insert("view_id".to_string(), Value::String(view_id));
            }
            if !field_names.is_empty() {
                body.insert(
                    "field_names".to_string(),
                    Value::Array(field_names.iter().map(|name| Value::String(name.clone())).collect()),
                );
            }

            let data: BitableRecordsData = self.client.post_data(&path, &query, Value::Object(body)).await?;
            for record in data.items {
                records.push(record);
                if records.len() >= max_records {
                    return Ok(records);
                }
            }
            if !data.has_more.unwrap_or(false) {
                break;
            }
            page_token = clean_optional(data.page_token);
            if page_token.is_none() {
                break;
            }
        }

        Ok(records)
    }

    fn user_id_query(&self) -> Vec<(String, String)> {
        clean_optional(self.config.user_id_type.clone())
            .map(|user_id_type| vec![("user_id_type".to_string(), user_id_type)])
            .unwrap_or_default()
    }
}

#[async_trait]
impl ExternalTabularSource for FeishuBitableSource {
    fn capabilities(&self) -> ExternalCapabilities {
        ExternalCapabilities {
            can_read: true,
            can_write: true,
            can_append: true,
            can_delete_rows: true,
            supports_multiple_tables: self.config.table_id.as_ref().map_or(true, |value| value.trim().is_empty()),
            supports_refresh: true,
            supports_file_watch: false,
            supports_schema_detection: true,
        }
    }

    async fn list_tables(&self) -> Result<Vec<ExternalTableRef>, String> {
        Ok(self
            .list_bitable_tables()
            .await?
            .into_iter()
            .map(|table| {
                let name = clean_title(&table.name).unwrap_or_else(|| table.table_id.clone());
                ExternalTableRef { source_id: table.table_id, table_name: name.clone(), display_name: name }
            })
            .collect())
    }

    async fn get_columns(&self, table: &ExternalTableRef) -> Result<Vec<ExternalColumnDef>, String> {
        let fields = self.list_fields(&table.source_id).await?;
        Ok(bitable_columns(&fields))
    }

    async fn load_table(&self, table: &ExternalTableRef) -> Result<ExternalTableSnapshot, String> {
        let fields = self.list_fields(&table.source_id).await?;
        let records = self.search_records(&table.source_id, &fields).await?;
        let columns = bitable_columns(&fields);
        let rows = records
            .iter()
            .map(|record| {
                let mut row = vec![Value::String(record.record_id.clone())];
                row.extend(fields.iter().map(|field| {
                    record.fields.get(&field.field_name).map(bitable_value_to_json).unwrap_or(Value::Null)
                }));
                row
            })
            .collect();

        Ok(ExternalTableSnapshot {
            table_ref: table.clone(),
            columns,
            rows,
            source_version: format!("records:{}", records.len()),
        })
    }

    async fn source_version(&self, table: &ExternalTableRef) -> Result<String, String> {
        let table_id = table.source_id.as_str();
        let version = self
            .list_bitable_tables()
            .await?
            .into_iter()
            .find(|table| table.table_id == table_id)
            .and_then(|table| table.revision)
            .map(|revision| revision.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        Ok(format!("revision:{version}"))
    }

    async fn test_connection(&self) -> Result<String, String> {
        let tables = self.list_bitable_tables().await?;
        Ok(format!("Feishu Bitable connected: {} table(s)", tables.len()))
    }

    fn display_name(&self) -> String {
        format!("Feishu Bitable: {}", self.config.app_token)
    }

    fn refresh_before_query(&self) -> bool {
        self.config.sync_mode == ExternalSyncMode::Realtime
    }

    async fn append_rows(
        &self,
        table: &ExternalTableRef,
        rows: Vec<Vec<Value>>,
    ) -> Result<ExternalWriteResult, String> {
        if rows.is_empty() {
            return Ok(ExternalWriteResult { affected_rows: 0, raw: serde_json::json!({ "records": [] }) });
        }

        let fields = self.list_fields(&table.source_id).await?;
        let path = format!(
            "/open-apis/bitable/v1/apps/{}/tables/{}/records/batch_create",
            encode_path_segment(&self.config.app_token),
            encode_path_segment(&table.source_id)
        );
        let query = self.user_id_query();
        let mut raw_responses = Vec::new();
        let mut affected_rows = 0;

        for chunk in rows.chunks(BITABLE_BATCH_CREATE_LIMIT) {
            let records: Vec<Value> = chunk
                .iter()
                .map(|row| {
                    let mut values = Map::new();
                    for (index, field) in fields.iter().enumerate() {
                        let value = row.get(index).cloned().unwrap_or(Value::Null);
                        if !value.is_null() {
                            values.insert(field.field_name.clone(), value);
                        }
                    }
                    serde_json::json!({ "fields": values })
                })
                .collect();

            let data: Value = self.client.post_data(&path, &query, serde_json::json!({ "records": records })).await?;
            affected_rows += data.get("records").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            raw_responses.push(data);
        }

        Ok(ExternalWriteResult { affected_rows, raw: collapse_raw_responses(raw_responses) })
    }

    async fn update_rows(
        &self,
        table: &ExternalTableRef,
        updates: Vec<ExternalRowUpdate>,
    ) -> Result<ExternalWriteResult, String> {
        let updates = updates
            .into_iter()
            .filter_map(|mut update| {
                let row_id = update.row_id.trim().to_string();
                update.fields.remove("_record_id");
                if row_id.is_empty() || update.fields.is_empty() {
                    None
                } else {
                    Some(serde_json::json!({ "record_id": row_id, "fields": update.fields }))
                }
            })
            .collect::<Vec<_>>();

        if updates.is_empty() {
            return Ok(ExternalWriteResult { affected_rows: 0, raw: serde_json::json!({ "records": [] }) });
        }

        let path = format!(
            "/open-apis/bitable/v1/apps/{}/tables/{}/records/batch_update",
            encode_path_segment(&self.config.app_token),
            encode_path_segment(&table.source_id)
        );
        let query = self.user_id_query();
        let mut raw_responses = Vec::new();
        let mut affected_rows = 0;

        for chunk in updates.chunks(BITABLE_BATCH_UPDATE_LIMIT) {
            let data: Value = self.client.post_data(&path, &query, serde_json::json!({ "records": chunk })).await?;
            affected_rows += data.get("records").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            raw_responses.push(data);
        }

        Ok(ExternalWriteResult { affected_rows, raw: collapse_raw_responses(raw_responses) })
    }

    async fn delete_rows(&self, table: &ExternalTableRef, row_ids: Vec<String>) -> Result<ExternalWriteResult, String> {
        let row_ids = row_ids
            .into_iter()
            .map(|row_id| row_id.trim().to_string())
            .filter(|row_id| !row_id.is_empty())
            .collect::<Vec<_>>();

        if row_ids.is_empty() {
            return Ok(ExternalWriteResult { affected_rows: 0, raw: serde_json::json!({ "records": [] }) });
        }

        let path = format!(
            "/open-apis/bitable/v1/apps/{}/tables/{}/records/batch_delete",
            encode_path_segment(&self.config.app_token),
            encode_path_segment(&table.source_id)
        );
        let mut raw_responses = Vec::new();
        let mut affected_rows = 0;

        for chunk in row_ids.chunks(BITABLE_BATCH_DELETE_LIMIT) {
            let data: Value = self.client.post_data(&path, &[], serde_json::json!({ "records": chunk })).await?;
            affected_rows += data
                .get("records")
                .and_then(Value::as_array)
                .map(|records| {
                    records
                        .iter()
                        .filter(|record| record.get("deleted").and_then(Value::as_bool).unwrap_or(true))
                        .count()
                })
                .unwrap_or(chunk.len());
            raw_responses.push(data);
        }

        Ok(ExternalWriteResult { affected_rows, raw: collapse_raw_responses(raw_responses) })
    }
}

#[derive(Debug, Deserialize)]
struct SheetQueryData {
    #[serde(default)]
    sheets: Vec<FeishuSheet>,
}

#[derive(Debug, Clone, Deserialize)]
struct FeishuSheet {
    sheet_id: String,
    title: String,
    #[serde(default)]
    resource_type: Option<String>,
    #[serde(default)]
    grid_properties: Option<SheetGridProperties>,
}

#[derive(Debug, Clone, Deserialize)]
struct SheetGridProperties {
    #[serde(default)]
    row_count: Option<usize>,
    #[serde(default)]
    column_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SheetValuesData {
    #[serde(default)]
    revision: Option<i64>,
    #[serde(default)]
    value_range: Option<SheetValueRange>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SheetValueRange {
    #[serde(default)]
    revision: Option<i64>,
    #[serde(default)]
    values: Option<Vec<Vec<Value>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct BitableTable {
    table_id: String,
    name: String,
    #[serde(default)]
    revision: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BitableTablesData {
    #[serde(default)]
    items: Vec<BitableTable>,
    #[serde(default)]
    has_more: Option<bool>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BitableField {
    field_name: String,
    #[serde(default)]
    is_primary: Option<bool>,
    #[serde(default, rename = "type")]
    field_type: Option<i64>,
    #[serde(default)]
    ui_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitableFieldsData {
    #[serde(default)]
    items: Vec<BitableField>,
    #[serde(default)]
    has_more: Option<bool>,
    #[serde(default)]
    page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitableRecord {
    record_id: String,
    #[serde(default)]
    fields: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct BitableRecordsData {
    #[serde(default)]
    items: Vec<BitableRecord>,
    #[serde(default)]
    has_more: Option<bool>,
    #[serde(default)]
    page_token: Option<String>,
}

fn sheet_values_to_snapshot(
    table_ref: ExternalTableRef,
    values: Vec<Vec<Value>>,
    has_header: bool,
    source_version: String,
) -> ExternalTableSnapshot {
    let max_width = values.iter().map(Vec::len).max().unwrap_or(0);
    let (headers, data_start) = if has_header && !values.is_empty() {
        let headers = (0..max_width)
            .map(|index| {
                values[0]
                    .get(index)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("column_{}", index + 1))
            })
            .collect::<Vec<_>>();
        (headers, 1)
    } else {
        ((0..max_width).map(|index| format!("column_{}", index + 1)).collect::<Vec<_>>(), 0)
    };

    let rows: Vec<Vec<Value>> = values[data_start..]
        .iter()
        .map(|row| (0..max_width).map(|index| row.get(index).map(normalize_json_cell).unwrap_or(Value::Null)).collect())
        .collect();
    let columns = headers
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let samples = rows.iter().filter_map(|row| row.get(index)).collect::<Vec<_>>();
            ExternalColumnDef {
                name: name.clone(),
                duckdb_type: infer_json_type(&samples),
                is_nullable: true,
                is_primary_key: false,
                comment: None,
            }
        })
        .collect();

    ExternalTableSnapshot { table_ref, columns, rows, source_version }
}

fn bitable_columns(fields: &[BitableField]) -> Vec<ExternalColumnDef> {
    let mut columns = vec![ExternalColumnDef {
        name: "_record_id".to_string(),
        duckdb_type: "VARCHAR".to_string(),
        is_nullable: false,
        is_primary_key: true,
        comment: Some("Feishu Bitable record ID".to_string()),
    }];

    columns.extend(fields.iter().map(|field| ExternalColumnDef {
        name: field.field_name.clone(),
        duckdb_type: bitable_duckdb_type(field),
        is_nullable: true,
        is_primary_key: field.is_primary.unwrap_or(false),
        comment: field.ui_type.clone(),
    }));
    columns
}

fn bitable_duckdb_type(field: &BitableField) -> String {
    match (field.field_type, field.ui_type.as_deref()) {
        (Some(2), _) => "DOUBLE",
        (Some(5), _) | (Some(1001), _) | (Some(1002), _) => "BIGINT",
        (Some(7), _) => "BOOLEAN",
        (_, Some("Number" | "Progress" | "Currency" | "Rating")) => "DOUBLE",
        (_, Some("Checkbox")) => "BOOLEAN",
        _ => "VARCHAR",
    }
    .to_string()
}

fn bitable_value_to_json(value: &Value) -> Value {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
        Value::Array(items) => {
            let text =
                items.iter().filter_map(|item| item.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("");
            if !text.is_empty() {
                Value::String(text)
            } else {
                Value::String(value.to_string())
            }
        }
        Value::Object(object) => object
            .get("text")
            .or_else(|| object.get("name"))
            .and_then(Value::as_str)
            .map(|text| Value::String(text.to_string()))
            .unwrap_or_else(|| Value::String(value.to_string())),
    }
}

fn infer_json_type(values: &[&Value]) -> String {
    let non_null = values.iter().copied().filter(|value| !value.is_null()).collect::<Vec<_>>();
    if non_null.is_empty() {
        return "VARCHAR".to_string();
    }
    if non_null.iter().all(|value| value.as_bool().is_some()) {
        return "BOOLEAN".to_string();
    }
    if non_null.iter().all(|value| value.as_i64().is_some()) {
        return "BIGINT".to_string();
    }
    if non_null.iter().all(|value| value.as_f64().is_some()) {
        return "DOUBLE".to_string();
    }
    "VARCHAR".to_string()
}

fn normalize_json_cell(value: &Value) -> Value {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
        _ => Value::String(value.to_string()),
    }
}

fn wanted_field_names(values: &[String]) -> Option<Vec<String>> {
    let values = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

fn clean_access_token(value: Option<String>) -> Option<String> {
    clean_optional(value).map(|value| normalize_access_token_value(&value)).filter(|value| !value.is_empty())
}

fn normalize_access_token_value(value: &str) -> String {
    let value = value.trim();
    let rest = value.get(6..).unwrap_or_default();
    if value.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer"))
        && rest.chars().next().is_some_and(char::is_whitespace)
    {
        rest.trim().to_string()
    } else {
        value.to_string()
    }
}

fn clean_title(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn encode_path_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn column_label(mut one_based_index: usize) -> String {
    one_based_index = one_based_index.max(1);
    let mut chars = Vec::new();
    while one_based_index > 0 {
        one_based_index -= 1;
        chars.push((b'A' + (one_based_index % 26) as u8) as char);
        one_based_index /= 26;
    }
    chars.iter().rev().collect()
}

fn normalize_sheet_write_range(sheet_id: &str, range: &str, rows: &[Vec<Value>]) -> String {
    let range = range.trim();
    let (target_sheet_id, cell_range) = match range.split_once('!') {
        Some((range_sheet_id, cell_range)) => (range_sheet_id.trim(), cell_range.trim()),
        None => (sheet_id.trim(), range),
    };

    let target_sheet_id = if target_sheet_id.is_empty() { sheet_id.trim() } else { target_sheet_id };
    if target_sheet_id.is_empty() {
        return range.to_string();
    }
    let cell_range = if !range.contains('!') && cell_range == target_sheet_id { "" } else { cell_range };

    if let Some((column, row)) = parse_cell_ref(cell_range) {
        let (row_count, column_count) = matrix_dimensions(rows);
        let start_column = column_label(column_index(column));
        let end_column = column_label(column_index(column) + column_count.saturating_sub(1));
        let end_row = row + row_count.saturating_sub(1);
        return format!("{target_sheet_id}!{start_column}{row}:{end_column}{end_row}");
    }

    if cell_range.is_empty() {
        let (row_count, column_count) = matrix_dimensions(rows);
        return format!("{target_sheet_id}!A1:{}{}", column_label(column_count), row_count);
    }

    if range.contains('!') {
        format!("{target_sheet_id}!{cell_range}")
    } else {
        format!("{target_sheet_id}!{range}")
    }
}

fn matrix_dimensions(rows: &[Vec<Value>]) -> (usize, usize) {
    let row_count = rows.len().max(1);
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    (row_count, column_count)
}

fn parse_cell_ref(value: &str) -> Option<(&str, usize)> {
    let value = value.trim();
    if value.is_empty() || value.contains(':') {
        return None;
    }

    let split_at = value.find(|c: char| c.is_ascii_digit())?;
    let (column, row) = value.split_at(split_at);
    if column.is_empty()
        || row.is_empty()
        || !column.chars().all(|c| c.is_ascii_alphabetic())
        || !row.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }

    let row = row.parse::<usize>().ok()?;
    if row == 0 {
        return None;
    }
    Some((column, row))
}

fn column_index(label: &str) -> usize {
    label
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .fold(0usize, |acc, c| acc * 26 + (c.to_ascii_uppercase() as u8 - b'A' + 1) as usize)
        .max(1)
}

fn collapse_raw_responses(mut responses: Vec<Value>) -> Value {
    if responses.len() == 1 {
        responses.remove(0)
    } else {
        serde_json::json!({ "responses": responses })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_spreadsheet_column_labels() {
        assert_eq!(column_label(1), "A");
        assert_eq!(column_label(26), "Z");
        assert_eq!(column_label(27), "AA");
        assert_eq!(column_label(702), "ZZ");
    }

    #[test]
    fn normalizes_pasted_bearer_tokens() {
        assert_eq!(normalize_access_token_value("Bearer t-abc"), "t-abc");
        assert_eq!(normalize_access_token_value("bearer   u-abc  "), "u-abc");
        assert_eq!(normalize_access_token_value("bearer-token-without-space"), "bearer-token-without-space");
        assert_eq!(normalize_access_token_value("t-raw"), "t-raw");
    }

    #[test]
    fn expands_sheet_write_anchor_ranges_to_payload_dimensions() {
        let rows = vec![
            vec![Value::String("A".to_string()), Value::String("B".to_string())],
            vec![Value::String("C".to_string()), Value::String("D".to_string())],
        ];

        assert_eq!(normalize_sheet_write_range("s1", "A1", &rows), "s1!A1:B2");
        assert_eq!(normalize_sheet_write_range("s1", "C3", &rows), "s1!C3:D4");
        assert_eq!(normalize_sheet_write_range("s1", "s2!Z9", &rows), "s2!Z9:AA10");
        assert_eq!(normalize_sheet_write_range("s1", "A1:C3", &rows), "s1!A1:C3");
        assert_eq!(normalize_sheet_write_range("s1", "s1", &rows), "s1!A1:B2");
    }

    #[test]
    fn converts_sheet_values_to_typed_snapshot() {
        let table_ref = ExternalTableRef {
            source_id: "sheet1".to_string(),
            table_name: "Sheet1".to_string(),
            display_name: "Sheet1".to_string(),
        };
        let values = vec![
            vec![Value::String("id".to_string()), Value::String("active".to_string())],
            vec![Value::Number(1.into()), Value::Bool(true)],
            vec![Value::Number(2.into()), Value::Bool(false)],
        ];

        let snapshot = sheet_values_to_snapshot(table_ref, values, true, "revision:1".to_string());

        assert_eq!(snapshot.columns[0].name, "id");
        assert_eq!(snapshot.columns[0].duckdb_type, "BIGINT");
        assert_eq!(snapshot.columns[1].duckdb_type, "BOOLEAN");
        assert_eq!(snapshot.rows.len(), 2);
    }

    #[test]
    fn flattens_bitable_rich_text_values() {
        let value = serde_json::json!([
            {"type": "text", "text": "Hello"},
            {"type": "text", "text": " World"}
        ]);

        assert_eq!(bitable_value_to_json(&value), Value::String("Hello World".to_string()));
    }
}
