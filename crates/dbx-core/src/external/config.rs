use serde::{Deserialize, Serialize};

use crate::models::connection::DatabaseType;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CsvExternalConfig {
    #[serde(default = "default_csv_delimiter")]
    pub delimiter: String,
    #[serde(default = "default_true")]
    pub has_header: bool,
    #[serde(default = "default_csv_encoding")]
    pub encoding: String,
}

impl Default for CsvExternalConfig {
    fn default() -> Self {
        Self { delimiter: default_csv_delimiter(), has_header: true, encoding: default_csv_encoding() }
    }
}

impl CsvExternalConfig {
    pub fn delimiter_byte(&self) -> Result<u8, String> {
        let mut chars = self.delimiter.chars();
        let delimiter = chars.next().ok_or("CSV delimiter must not be empty")?;
        if chars.next().is_some() || !delimiter.is_ascii() {
            return Err("CSV delimiter must be one ASCII character".to_string());
        }
        Ok(delimiter as u8)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct XlsxExternalConfig {
    #[serde(default = "default_true")]
    pub has_header: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_range: Option<String>,
}

impl Default for XlsxExternalConfig {
    fn default() -> Self {
        Self { has_header: true, data_range: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeishuSheetsExternalConfig {
    pub spreadsheet_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_range: Option<String>,
    #[serde(default = "default_true")]
    pub has_header: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeishuBaseExternalConfig {
    pub base_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalSourceConfig {
    Csv(CsvExternalConfig),
    Xlsx(XlsxExternalConfig),
    FeishuSheets(FeishuSheetsExternalConfig),
    FeishuBase(FeishuBaseExternalConfig),
}

impl ExternalSourceConfig {
    pub fn parse(db_type: DatabaseType, value: Option<&serde_json::Value>) -> Result<Self, String> {
        let value = value.cloned().unwrap_or_else(|| serde_json::json!({}));
        match db_type {
            DatabaseType::Csv => serde_json::from_value(value)
                .map(Self::Csv)
                .map_err(|error| format!("Invalid CSV external configuration: {error}")),
            DatabaseType::Xlsx => serde_json::from_value(value)
                .map(Self::Xlsx)
                .map_err(|error| format!("Invalid XLSX external configuration: {error}")),
            DatabaseType::FeishuSheets => serde_json::from_value::<FeishuSheetsExternalConfig>(value)
                .map(Self::FeishuSheets)
                .map_err(|error| format!("Invalid Feishu Sheets external configuration: {error}")),
            DatabaseType::FeishuBase => serde_json::from_value::<FeishuBaseExternalConfig>(value)
                .map(Self::FeishuBase)
                .map_err(|error| format!("Invalid Feishu Base external configuration: {error}")),
            _ => Err(format!("{} is not an external table connection", db_type.as_str())),
        }
    }
}

fn default_csv_delimiter() -> String {
    ",".to_string()
}

fn default_csv_encoding() -> String {
    "utf-8".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ui_config_using_camel_case_keys() {
        let config = ExternalSourceConfig::parse(
            DatabaseType::Csv,
            Some(&serde_json::json!({ "delimiter": "\t", "hasHeader": false, "encoding": "gb18030" })),
        )
        .unwrap();

        assert_eq!(
            config,
            ExternalSourceConfig::Csv(CsvExternalConfig {
                delimiter: "\t".to_string(),
                has_header: false,
                encoding: "gb18030".to_string(),
            })
        );
    }

    #[test]
    fn feishu_config_contains_resource_ids_but_no_secret_field() {
        let value = serde_json::json!({ "spreadsheetToken": "sht_1", "sheetId": "sh_1" });
        let parsed = ExternalSourceConfig::parse(DatabaseType::FeishuSheets, Some(&value)).unwrap();
        let serialized = serde_json::to_string(&value).unwrap();

        assert!(matches!(parsed, ExternalSourceConfig::FeishuSheets(_)));
        assert!(!serialized.contains("appSecret"));
        assert!(!serialized.contains("accessToken"));
    }
}
