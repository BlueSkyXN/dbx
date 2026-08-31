use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{ExternalTableError, ExternalTableErrorKind};

const DEFAULT_FEISHU_BASE_URL: &str = "https://open.feishu.cn";
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ERROR_MESSAGE_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FeishuRequestErrorKind {
    BeforeDispatch,
    Rejected,
    Unknown,
}

#[derive(Debug, Clone)]
pub(crate) struct FeishuRequestError {
    pub kind: FeishuRequestErrorKind,
    pub message: String,
}

impl FeishuRequestError {
    fn before_dispatch(message: impl Into<String>) -> Self {
        Self { kind: FeishuRequestErrorKind::BeforeDispatch, message: message.into() }
    }

    fn rejected(message: impl Into<String>) -> Self {
        Self { kind: FeishuRequestErrorKind::Rejected, message: message.into() }
    }

    fn unknown(message: impl Into<String>) -> Self {
        Self { kind: FeishuRequestErrorKind::Unknown, message: message.into() }
    }

    pub fn as_external_error(&self) -> ExternalTableError {
        let kind = match self.kind {
            FeishuRequestErrorKind::BeforeDispatch => ExternalTableErrorKind::Transport,
            FeishuRequestErrorKind::Rejected => ExternalTableErrorKind::Remote,
            FeishuRequestErrorKind::Unknown => ExternalTableErrorKind::Transport,
        };
        ExternalTableError::new(kind, self.message.clone())
    }
}

#[derive(Debug, Clone)]
struct CachedToken {
    value: String,
    refresh_at: Instant,
}

#[derive(Clone)]
pub(crate) struct FeishuClient {
    base_url: String,
    app_id: String,
    app_secret: String,
    client: reqwest::Client,
    token: Arc<Mutex<Option<CachedToken>>>,
}

impl std::fmt::Debug for FeishuClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeishuClient")
            .field("base_url", &self.base_url)
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}

impl FeishuClient {
    pub(crate) fn new(
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, ExternalTableError> {
        Self::with_base_url(DEFAULT_FEISHU_BASE_URL, app_id, app_secret, timeout)
    }

    pub(crate) fn with_base_url(
        base_url: impl Into<String>,
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, ExternalTableError> {
        let app_id = app_id.into();
        let app_secret = app_secret.into();
        if app_id.trim().is_empty() || app_secret.is_empty() {
            return Err(ExternalTableError::invalid(
                "Feishu app_id and app_secret are required for app identity authentication",
            ));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(timeout.min(Duration::from_secs(30)))
            .timeout(timeout)
            .user_agent("DBX external-table/1")
            .build()
            .map_err(|error| ExternalTableError::transport(format!("Failed to build Feishu HTTP client: {error}")))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            app_id,
            app_secret,
            client,
            token: Arc::new(Mutex::new(None)),
        })
    }

    async fn tenant_access_token(&self) -> Result<String, FeishuRequestError> {
        let mut token = self.token.lock().await;
        if let Some(token) = token.as_ref().filter(|token| token.refresh_at > Instant::now()) {
            return Ok(token.value.clone());
        }

        let url = format!("{}/open-apis/auth/v3/tenant_access_token/internal", self.base_url);
        let response = self
            .client
            .post(url)
            .json(&json!({ "app_id": self.app_id, "app_secret": self.app_secret }))
            .send()
            .await
            .map_err(|error| self.classify_transport_error(error, false))?;
        let status = response.status();
        let body = bounded_body(response, false).await?;
        if !status.is_success() {
            return Err(FeishuRequestError::rejected(format!("Feishu token request failed with HTTP {status}")));
        }
        #[derive(Deserialize)]
        struct TokenResponse {
            code: Option<i64>,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            tenant_access_token: String,
            #[serde(default)]
            expire: u64,
        }
        let parsed: TokenResponse = serde_json::from_slice(&body)
            .map_err(|_| FeishuRequestError::rejected("Feishu token endpoint returned invalid JSON"))?;
        if parsed.code != Some(0) || parsed.tenant_access_token.is_empty() {
            return Err(FeishuRequestError::rejected(format!(
                "Feishu token request rejected (code {}): {}",
                parsed.code.map(|code| code.to_string()).unwrap_or_else(|| "missing".to_string()),
                self.redact_message(&parsed.msg)
            )));
        }
        let refresh_in = parsed.expire.saturating_sub(60).max(1);
        let value = parsed.tenant_access_token;
        *token =
            Some(CachedToken { value: value.clone(), refresh_at: Instant::now() + Duration::from_secs(refresh_in) });
        Ok(value)
    }

    pub(crate) async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value, FeishuRequestError> {
        self.request_json(Method::GET, path, query, None, false).await
    }

    pub(crate) async fn post_json(&self, path: &str, body: Value, mutation: bool) -> Result<Value, FeishuRequestError> {
        self.request_json(Method::POST, path, &[], Some(body), mutation).await
    }

    async fn request_json(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
        mutation: bool,
    ) -> Result<Value, FeishuRequestError> {
        let token = self.tenant_access_token().await?;
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.client.request(method, url).bearer_auth(&token).query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| self.classify_transport_error(error, mutation))?;
        let status = response.status();
        let bytes = bounded_body(response, mutation).await?;
        if !status.is_success() {
            if mutation && (status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT) {
                return Err(FeishuRequestError::unknown(format!(
                    "{}; the server may have applied the operation, so automatic retry is blocked",
                    http_status_message(status)
                )));
            }
            return Err(FeishuRequestError::rejected(http_status_message(status)));
        }
        let envelope: Value = serde_json::from_slice(&bytes)
            .map_err(|_| response_decode_error("Feishu API returned invalid JSON", mutation))?;
        let code = envelope
            .get("code")
            .and_then(Value::as_i64)
            .ok_or_else(|| response_decode_error("Feishu API response is missing a numeric code", mutation))?;
        if code != 0 {
            let message = envelope.get("msg").and_then(Value::as_str).unwrap_or("request rejected");
            return Err(FeishuRequestError::rejected(format!(
                "Feishu API rejected the request (code {code}): {}",
                self.redact_message(message)
            )));
        }
        Ok(envelope.get("data").cloned().unwrap_or(Value::Null))
    }

    pub(crate) async fn invoke_sheet_tool(
        &self,
        spreadsheet_token: &str,
        tool_name: &str,
        input: Value,
        write: bool,
    ) -> Result<Value, FeishuRequestError> {
        let encoded_token =
            percent_encoding::utf8_percent_encode(spreadsheet_token, percent_encoding::NON_ALPHANUMERIC);
        let suffix = if write { "invoke_write" } else { "invoke_read" };
        let input = serde_json::to_string(&input).map_err(|error| {
            FeishuRequestError::before_dispatch(format!("Failed to encode sheet tool input: {error}"))
        })?;
        let data = self
            .post_json(
                &format!("/open-apis/sheet_ai/v2/spreadsheets/{encoded_token}/tools/{suffix}"),
                json!({ "tool_name": tool_name, "input": input }),
                write,
            )
            .await?;
        let output = data.get("output").and_then(Value::as_str).unwrap_or_default();
        if output.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(output)
            .map_err(|_| response_decode_error(format!("Sheet tool '{tool_name}' returned invalid JSON output"), write))
    }

    fn classify_transport_error(&self, error: reqwest::Error, mutation: bool) -> FeishuRequestError {
        let message = if error.is_timeout() {
            "Feishu request timed out"
        } else if error.is_connect() {
            "Failed to connect to Feishu"
        } else {
            "Feishu transport failed"
        };
        if mutation && !error.is_connect() && !error.is_builder() {
            FeishuRequestError::unknown(format!(
                "{message}; the server may have applied the operation, so automatic retry is blocked"
            ))
        } else {
            FeishuRequestError::before_dispatch(message)
        }
    }

    fn redact_message(&self, message: &str) -> String {
        let mut message = message.replace(&self.app_secret, "[REDACTED]").replace(&self.app_id, "[REDACTED]");
        if message.chars().count() > MAX_ERROR_MESSAGE_CHARS {
            message = message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect::<String>();
            message.push('…');
        }
        message
    }
}

async fn bounded_body(mut response: reqwest::Response, mutation: bool) -> Result<bytes::Bytes, FeishuRequestError> {
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(response_decode_error("Feishu response exceeds the configured size limit", mutation));
    }
    let mut body = bytes::BytesMut::new();
    while let Some(chunk) =
        response.chunk().await.map_err(|_| response_decode_error("Failed to read Feishu response", mutation))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(response_decode_error("Feishu response exceeds the configured size limit", mutation));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

fn response_decode_error(message: impl Into<String>, mutation: bool) -> FeishuRequestError {
    let message = message.into();
    if mutation {
        FeishuRequestError::unknown(format!(
            "{message}; the server may have applied the operation, so automatic retry is blocked"
        ))
    } else {
        FeishuRequestError::rejected(message)
    }
}

fn http_status_message(status: StatusCode) -> String {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            format!("Feishu authorization failed with HTTP {status}")
        }
        StatusCode::TOO_MANY_REQUESTS => "Feishu rate limit exceeded (HTTP 429)".to_string(),
        status if status.is_server_error() => format!("Feishu service failed with HTTP {status}"),
        _ => format!("Feishu request failed with HTTP {status}"),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Debug, Clone)]
    pub enum MockReply {
        Json(String),
        Status(u16, String),
        DropConnection,
    }

    pub async fn serve(replies: Vec<MockReply>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                let mut expected_length = None;
                loop {
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                        let header_end = header_end + 4;
                        if expected_length.is_none() {
                            let headers = String::from_utf8_lossy(&request[..header_end]);
                            let content_length = headers.lines().find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            });
                            expected_length = Some(header_end + content_length.unwrap_or(0));
                        }
                        if request.len() >= expected_length.unwrap_or(header_end) {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8_lossy(&request).into_owned());
                match reply {
                    MockReply::DropConnection => {}
                    MockReply::Json(body) => write_reply(&mut stream, 200, "OK", &body).await,
                    MockReply::Status(status, body) => write_reply(&mut stream, status, "Error", &body).await,
                }
            }
            requests
        });
        (format!("http://{address}"), task)
    }

    async fn write_reply(stream: &mut tokio::net::TcpStream, status: u16, reason: &str, body: &str) {
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{serve, MockReply};
    use super::*;

    fn token() -> MockReply {
        MockReply::Json(
            json!({ "code": 0, "msg": "ok", "tenant_access_token": "tenant-sensitive", "expire": 7200 }).to_string(),
        )
    }

    fn token_value(value: &str) -> MockReply {
        MockReply::Json(json!({ "code": 0, "msg": "ok", "tenant_access_token": value, "expire": 7200 }).to_string())
    }

    #[tokio::test]
    async fn token_is_cached_only_in_memory_and_sheet_output_is_decoded() {
        let output = json!({ "sheets": [{ "sheet_id": "sh1", "title": "Sheet1" }] }).to_string();
        let (base_url, server) = serve(vec![
            token(),
            MockReply::Json(json!({ "code": 0, "msg": "ok", "data": { "output": output } }).to_string()),
            MockReply::Json(json!({ "code": 0, "msg": "ok", "data": { "output": "{\"revision\":2}" } }).to_string()),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

        let structure = client
            .invoke_sheet_tool("spreadsheet", "get_workbook_structure", json!({ "excel_id": "spreadsheet" }), false)
            .await
            .unwrap();
        let revision = client
            .invoke_sheet_tool("spreadsheet", "get_workbook_structure", json!({ "excel_id": "spreadsheet" }), false)
            .await
            .unwrap();

        assert_eq!(structure["sheets"][0]["sheet_id"], "sh1");
        assert_eq!(revision["revision"], 2);
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3, "two API calls must share one cached tenant token");
        assert!(requests[1].to_ascii_lowercase().contains("authorization: bearer tenant-sensitive"));
    }

    #[tokio::test]
    async fn nonzero_business_code_and_http_errors_do_not_leak_bodies_or_secrets() {
        let (base_url, server) = serve(vec![
            token(),
            MockReply::Json(
                json!({ "code": 99, "msg": "app-secret rejected with customer-sensitive-row", "data": {} }).to_string(),
            ),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

        let error = client.get_json("/probe", &[]).await.unwrap_err();

        server.await.unwrap();
        assert_eq!(error.kind, FeishuRequestErrorKind::Rejected);
        assert!(!error.message.contains("app-secret"));
        assert!(error.message.contains("customer-sensitive-row"));
    }

    #[tokio::test]
    async fn missing_business_code_is_never_treated_as_success() {
        let (read_url, read_server) = serve(vec![token(), MockReply::Json(json!({ "data": {} }).to_string())]).await;
        let read_client =
            FeishuClient::with_base_url(read_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

        let read_error = read_client.get_json("/probe", &[]).await.unwrap_err();

        read_server.await.unwrap();
        assert_eq!(read_error.kind, FeishuRequestErrorKind::Rejected);
        assert!(read_error.message.contains("numeric code"));

        let (write_url, write_server) = serve(vec![token(), MockReply::Json(json!({ "data": {} }).to_string())]).await;
        let write_client =
            FeishuClient::with_base_url(write_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

        let write_error = write_client.post_json("/write", json!({ "value": 1 }), true).await.unwrap_err();

        write_server.await.unwrap();
        assert_eq!(write_error.kind, FeishuRequestErrorKind::Unknown);
        assert!(write_error.message.contains("automatic retry is blocked"));
    }

    #[tokio::test]
    async fn dropped_write_response_is_unknown_and_blocks_automatic_retry() {
        let (base_url, server) = serve(vec![token(), MockReply::DropConnection]).await;
        let client = FeishuClient::with_base_url(base_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

        let error = client.post_json("/write", json!({ "value": 1 }), true).await.unwrap_err();

        server.await.unwrap();
        assert_eq!(error.kind, FeishuRequestErrorKind::Unknown);
        assert!(error.message.contains("automatic retry is blocked"));
    }

    #[tokio::test]
    async fn expired_cached_token_is_refreshed_before_the_next_request() {
        let (base_url, server) = serve(vec![
            token_value("tenant-one"),
            MockReply::Json(json!({ "code": 0, "data": {} }).to_string()),
            token_value("tenant-two"),
            MockReply::Json(json!({ "code": 0, "data": {} }).to_string()),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

        client.get_json("/probe", &[]).await.unwrap();
        client.token.lock().await.as_mut().unwrap().refresh_at = Instant::now() - Duration::from_secs(1);
        client.get_json("/probe", &[]).await.unwrap();

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests[1].to_ascii_lowercase().contains("authorization: bearer tenant-one"));
        assert!(requests[3].to_ascii_lowercase().contains("authorization: bearer tenant-two"));
    }

    #[tokio::test]
    async fn http_authorization_rate_limit_and_server_errors_hide_response_bodies() {
        for (status, expected) in [(401, "authorization"), (429, "rate limit"), (503, "service failed")] {
            let (base_url, server) =
                serve(vec![token(), MockReply::Status(status, "customer-sensitive-response-body".to_string())]).await;
            let client = FeishuClient::with_base_url(base_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

            let error = client.get_json("/probe", &[]).await.unwrap_err();

            server.await.unwrap();
            assert_eq!(error.kind, FeishuRequestErrorKind::Rejected);
            assert!(error.message.contains(expected));
            assert!(!error.message.contains("customer-sensitive-response-body"));
        }
    }

    #[tokio::test]
    async fn malformed_sheet_output_is_rejected_without_exposing_the_body() {
        let (base_url, server) = serve(vec![
            token(),
            MockReply::Json(json!({ "code": 0, "data": { "output": "{customer-sensitive-invalid-json" } }).to_string()),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

        let error =
            client.invoke_sheet_tool("spreadsheet", "get_workbook_structure", json!({}), false).await.unwrap_err();

        server.await.unwrap();
        assert_eq!(error.kind, FeishuRequestErrorKind::Rejected);
        assert!(!error.message.contains("customer-sensitive-invalid-json"));
    }

    #[tokio::test]
    async fn malformed_write_response_is_unknown_and_blocks_retry() {
        let (base_url, server) = serve(vec![
            token(),
            MockReply::Json(json!({ "code": 0, "data": { "output": "{customer-sensitive-invalid-json" } }).to_string()),
        ])
        .await;
        let client = FeishuClient::with_base_url(base_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

        let error = client.invoke_sheet_tool("spreadsheet", "set_cell_range", json!({}), true).await.unwrap_err();

        server.await.unwrap();
        assert_eq!(error.kind, FeishuRequestErrorKind::Unknown);
        assert!(error.message.contains("automatic retry is blocked"));
        assert!(!error.message.contains("customer-sensitive-invalid-json"));
    }

    #[tokio::test]
    async fn mutation_server_error_is_unknown_but_rate_limit_is_rejected() {
        for (status, expected_kind) in [
            (503, FeishuRequestErrorKind::Unknown),
            (408, FeishuRequestErrorKind::Unknown),
            (429, FeishuRequestErrorKind::Rejected),
        ] {
            let (base_url, server) = serve(vec![token(), MockReply::Status(status, "ignored".to_string())]).await;
            let client = FeishuClient::with_base_url(base_url, "app-id", "app-secret", Duration::from_secs(5)).unwrap();

            let error = client.post_json("/write", json!({ "value": 1 }), true).await.unwrap_err();

            server.await.unwrap();
            assert_eq!(error.kind, expected_kind);
            if expected_kind == FeishuRequestErrorKind::Unknown {
                assert!(error.message.contains("automatic retry is blocked"));
            }
        }
    }
}
