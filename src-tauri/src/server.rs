use axum::{
    extract::ConnectInfo,
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{
    mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode},
    MySqlPool,
};
use std::{
    collections::VecDeque,
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;

const TOKEN_HEADER: &str = "x-api-token";
const UPDATE_ITEM_PATH: &str = "/api/items";
const REQUEST_HISTORY_LIMIT: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub mysql_host: String,
    pub mysql_port: u16,
    pub mysql_database: String,
    pub mysql_username: String,
    pub mysql_password: String,
    pub bind_host: String,
    pub server_port: u16,
    pub api_token: String,
    pub table_name: String,
    #[serde(default)]
    pub fields: Vec<ColumnField>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            mysql_host: "127.0.0.1".into(),
            mysql_port: 3306,
            mysql_database: String::new(),
            mysql_username: String::new(),
            mysql_password: String::new(),
            bind_host: "0.0.0.0".into(),
            server_port: 8045,
            api_token: String::new(),
            table_name: String::new(),
            fields: vec![
                ColumnField {
                    name: "itemid".into(),
                    field_type: FieldType::String,
                    is_key: true,
                },
                ColumnField {
                    name: "price".into(),
                    field_type: FieldType::Float,
                    is_key: false,
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnField {
    pub name: String,
    #[serde(default)]
    pub field_type: FieldType,
    #[serde(default)]
    pub is_key: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    #[default]
    String,
    Integer,
    Float,
}

#[derive(Debug, Clone)]
pub enum BoundValue {
    String(String),
    Integer(i64),
    Float(f64),
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdatePayload {
    #[serde(flatten)]
    pub values: serde_json::Map<String, serde_json::Value>,
    #[serde(skip)]
    pub key_column: String,
    #[serde(skip)]
    pub key_value_display: String,
    #[serde(skip)]
    pub bound: Vec<(String, BoundValue)>,
}

fn required_key_message(config: &ServerConfig) -> String {
    let key = config
        .fields
        .iter()
        .find(|f| f.is_key)
        .map(|f| f.name.trim())
        .unwrap_or("key");
    format!("Invalid input. Required field {key} missing.")
}

fn extract_payload(
    config: &ServerConfig,
    value: serde_json::Value,
) -> Result<UpdatePayload, String> {
    let serde_json::Value::Object(map) = value else {
        return Err("Invalid input. Expected a JSON object.".into());
    };

    let normalized: serde_json::Map<String, serde_json::Value> =
        map.into_iter().map(|(k, v)| (k.to_lowercase(), v)).collect();

    let key_field = config
        .fields
        .iter()
        .find(|f| f.is_key)
        .ok_or_else(|| "Server misconfigured: no key field defined.".to_string())?;

    let mut values = serde_json::Map::new();
    let mut bound: Vec<(String, BoundValue)> = Vec::new();
    let mut key_value_display = String::new();

    for field in &config.fields {
        let raw = normalized.get(&field.name.trim().to_lowercase());
        let present = !matches!(raw, None | Some(serde_json::Value::Null));

        if field.is_key {
            if !present {
                return Err(format!(
                    "Invalid input. Required field {} missing.",
                    field.name.trim()
                ));
            }
            let (bv, display, echo) = coerce_value(field, raw.unwrap())?;
            key_value_display = display;
            values.insert(field.name.trim().to_string(), echo);
            bound.push((field.name.trim().to_string(), bv));
        } else if present {
            let (bv, _display, echo) = coerce_value(field, raw.unwrap())?;
            values.insert(field.name.trim().to_string(), echo);
            bound.push((field.name.trim().to_string(), bv));
        }
    }

    Ok(UpdatePayload {
        values,
        key_column: key_field.name.trim().to_string(),
        key_value_display,
        bound,
    })
}

fn coerce_value(
    field: &ColumnField,
    value: &serde_json::Value,
) -> Result<(BoundValue, String, serde_json::Value), String> {
    let name = field.name.trim();
    match field.field_type {
        FieldType::String => {
            let s = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => {
                    return Err(format!(
                        "Invalid input. {name} must be a string or number."
                    ))
                }
            };
            let s = s.trim().to_string();
            if s.is_empty() {
                return Err(format!("Invalid input. {name} must not be empty."));
            }
            Ok((
                BoundValue::String(s.clone()),
                s.clone(),
                serde_json::Value::String(s),
            ))
        }
        FieldType::Integer => {
            let parsed: i64 = match value {
                serde_json::Value::Number(n) => n
                    .as_i64()
                    .ok_or_else(|| format!("Invalid input. {name} must be an integer."))?,
                serde_json::Value::String(s) => s
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| format!("Invalid input. {name} must be an integer."))?,
                _ => return Err(format!("Invalid input. {name} must be an integer.")),
            };
            Ok((
                BoundValue::Integer(parsed),
                parsed.to_string(),
                serde_json::Value::from(parsed),
            ))
        }
        FieldType::Float => {
            let parsed: f64 = match value {
                serde_json::Value::Number(n) => n
                    .as_f64()
                    .ok_or_else(|| format!("Invalid input. {name} must be numeric."))?,
                serde_json::Value::String(s) => s
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("Invalid input. {name} must be numeric."))?,
                _ => return Err(format!("Invalid input. {name} must be numeric.")),
            };
            let display = match value {
                serde_json::Value::String(s) => s.trim().to_string(),
                _ => parsed.to_string(),
            };
            Ok((
                BoundValue::Float(parsed),
                display.clone(),
                serde_json::Value::String(display),
            ))
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub status: &'static str,
    pub message: &'static str,
    pub received: UpdatePayload,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub status: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRequestLog {
    pub id: u64,
    pub timestamp_ms: u64,
    pub remote_addr: Option<String>,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub itemid: Option<String>,
    pub message: String,
}

pub type RequestHistory = Arc<Mutex<VecDeque<ApiRequestLog>>>;

#[derive(Debug)]
pub struct ServerHandle {
    shutdown: Option<oneshot::Sender<()>>,
    pub bind_address: String,
}

impl ServerHandle {
    pub fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

#[derive(Clone)]
struct ApiState {
    config: ServerConfig,
    pool: MySqlPool,
    request_history: RequestHistory,
}

pub async fn start_server(
    config: ServerConfig,
    request_history: RequestHistory,
) -> Result<ServerHandle, String> {
    validate_config(&config)?;

    let normalized_host = match config.bind_host.trim() {
        "localhost" => "127.0.0.1",
        h => h,
    };
    let bind_address = format!("{}:{}", normalized_host, config.server_port);
    let socket_addr = SocketAddr::from_str(&bind_address)
        .map_err(|error| format!("Invalid bind address {bind_address}: {error}"))?;

    let pool = create_pool(&config).await?;
    let state = ApiState {
        config,
        pool,
        request_history,
    };
    let router = Router::new()
        .route(UPDATE_ITEM_PATH, any(update_item))
        .route("/health", any(health))
        .fallback(any(not_found))
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind(socket_addr)
        .await
        .map_err(|error| format!("Unable to bind {bind_address}: {error}"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|error| format!("Unable to read local address: {error}"))?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let server = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        });

        if let Err(error) = server.await {
            eprintln!("embedded API server stopped with an error: {error}");
        }
    });

    Ok(ServerHandle {
        shutdown: Some(shutdown_tx),
        bind_address: local_addr.to_string(),
    })
}

pub async fn create_pool(config: &ServerConfig) -> Result<MySqlPool, String> {
    let options = MySqlConnectOptions::new()
        .host(&config.mysql_host)
        .port(config.mysql_port)
        .database(&config.mysql_database)
        .username(&config.mysql_username)
        .password(&config.mysql_password)
        .ssl_mode(MySqlSslMode::Disabled);

    MySqlPoolOptions::new()
        .max_connections(64)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(60))
        .max_lifetime(Duration::from_secs(30 * 60))
        .connect_with(options)
        .await
        .map_err(|error| format!("Unable to connect to MySQL: {error}"))
}

async fn health(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    method: Method,
    uri: Uri,
) -> Response {
    let started = Instant::now();
    let response = Json(serde_json::json!({
        "status": "success",
        "message": "server running"
    }))
    .into_response();
    log_request(
        &state,
        remote_addr,
        &method,
        &uri,
        response.status(),
        started,
        None,
        "server running".to_string(),
    );
    response
}

async fn update_item(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    payload: Result<Json<serde_json::Value>, JsonRejection>,
) -> Response {
    let started = Instant::now();

    if method != Method::POST {
        let response = api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
        log_response(
            &state,
            remote_addr,
            &method,
            &uri,
            &response,
            started,
            None,
            "Method not allowed.",
        );
        return response;
    }

    if !token_is_valid(&headers, &state.config.api_token) {
        let response = api_error(StatusCode::UNAUTHORIZED, "Missing or invalid API token");
        log_response(
            &state,
            remote_addr,
            &method,
            &uri,
            &response,
            started,
            None,
            "Missing or invalid API token",
        );
        return response;
    }

    let raw = match payload {
        Ok(Json(v)) => v,
        Err(_) => {
            let message = format!(
                "Invalid JSON body. Send a JSON object with Content-Type: application/json. {}",
                required_key_message(&state.config)
            );
            let response = api_error(StatusCode::BAD_REQUEST, &message);
            log_response(
                &state,
                remote_addr,
                &method,
                &uri,
                &response,
                started,
                None,
                &message,
            );
            return response;
        }
    };

    let payload = match extract_payload(&state.config, raw) {
        Ok(p) => p,
        Err(message) => {
            let response = api_error(StatusCode::BAD_REQUEST, &message);
            log_response(
                &state,
                remote_addr,
                &method,
                &uri,
                &response,
                started,
                None,
                &message,
            );
            return response;
        }
    };

    let itemid = Some(payload.key_value_display.clone());

    let response = match execute_update(&state.config, &state.pool, &payload).await {
        Ok(0) => api_error(
            StatusCode::NOT_FOUND,
            &format!("No matching {} was found.", payload.key_column),
        ),
        Ok(_) => (
            StatusCode::OK,
            Json(SuccessResponse {
                status: "success",
                message: "successfully updated",
                received: payload,
            }),
        )
            .into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let status = response.status();
    let message = if status == StatusCode::OK {
        "successfully updated".to_string()
    } else {
        status
            .canonical_reason()
            .unwrap_or("request failed")
            .to_string()
    };
    log_response(
        &state,
        remote_addr,
        &method,
        &uri,
        &response,
        started,
        itemid,
        &message,
    );
    response
}

async fn execute_update(
    config: &ServerConfig,
    pool: &MySqlPool,
    payload: &UpdatePayload,
) -> Result<u64, String> {
    let set_items: Vec<&(String, BoundValue)> = payload
        .bound
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case(&payload.key_column))
        .collect();

    if set_items.is_empty() {
        return Err("Invalid input. No updatable fields provided.".into());
    }

    let set_clause = set_items
        .iter()
        .map(|(name, _)| format!("`{name}` = ?"))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "UPDATE `{}` SET {} WHERE `{}` = ?",
        config.table_name, set_clause, payload.key_column,
    );

    let mut query = sqlx::query(&sql);
    for (_, bv) in &set_items {
        query = bind_value(query, bv);
    }

    let key_bv = payload
        .bound
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&payload.key_column))
        .map(|(_, bv)| bv)
        .ok_or_else(|| "Server misconfigured: key value missing.".to_string())?;
    query = bind_value(query, key_bv);

    query
        .execute(pool)
        .await
        .map(|done| done.rows_affected())
        .map_err(|error| format!("Database update failed: {error}"))
}

fn bind_value<'q>(
    query: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    value: &'q BoundValue,
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match value {
        BoundValue::String(s) => query.bind(s.as_str()),
        BoundValue::Integer(i) => query.bind(*i),
        BoundValue::Float(f) => query.bind(*f),
    }
}

fn token_is_valid(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|actual| actual == expected)
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            status: "error",
            message: message.to_string(),
        }),
    )
        .into_response()
}

async fn not_found(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    method: Method,
    uri: Uri,
) -> Response {
    let started = Instant::now();
    let response = api_error(StatusCode::NOT_FOUND, "Route not found.");
    log_response(
        &state,
        remote_addr,
        &method,
        &uri,
        &response,
        started,
        None,
        "Route not found.",
    );
    response
}

fn log_response(
    state: &ApiState,
    remote_addr: SocketAddr,
    method: &Method,
    uri: &Uri,
    response: &Response,
    started: Instant,
    itemid: Option<String>,
    message: &str,
) {
    log_request(
        state,
        remote_addr,
        method,
        uri,
        response.status(),
        started,
        itemid,
        message.to_string(),
    );
}

fn log_request(
    state: &ApiState,
    remote_addr: SocketAddr,
    method: &Method,
    uri: &Uri,
    status: StatusCode,
    started: Instant,
    itemid: Option<String>,
    message: String,
) {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| millis_to_u64(duration.as_millis()))
        .unwrap_or_default();

    if let Ok(mut history) = state.request_history.lock() {
        let id = history.back().map_or(1, |entry| entry.id.saturating_add(1));
        history.push_back(ApiRequestLog {
            id,
            timestamp_ms,
            remote_addr: Some(remote_addr.to_string()),
            method: method.as_str().to_string(),
            path: uri.path().to_string(),
            status: status.as_u16(),
            duration_ms: millis_to_u64(started.elapsed().as_millis()),
            itemid,
            message,
        });

        while history.len() > REQUEST_HISTORY_LIMIT {
            history.pop_front();
        }
    }
}

fn millis_to_u64(value: u128) -> u64 {
    value.try_into().unwrap_or(u64::MAX)
}


pub fn validate_config(config: &ServerConfig) -> Result<(), String> {
    if config.mysql_host.trim().is_empty()
        || config.mysql_database.trim().is_empty()
        || config.mysql_username.trim().is_empty()
        || config.bind_host.trim().is_empty()
        || config.api_token.trim().is_empty()
    {
        return Err("MySQL, bind host, and API token settings are required.".into());
    }

    if !is_safe_identifier(&config.table_name) {
        return Err(
            "Invalid table name. Use only letters, numbers, and underscores; do not start with a number.".into()
        );
    }

    if config.fields.is_empty() {
        return Err("Define at least one field (the key field).".into());
    }

    let key_count = config.fields.iter().filter(|f| f.is_key).count();
    if key_count == 0 {
        return Err("One field must be marked as the key (WHERE clause).".into());
    }
    if key_count > 1 {
        return Err("Only one field may be marked as the key.".into());
    }

    let mut seen = std::collections::HashSet::<String>::new();
    for field in &config.fields {
        let name = field.name.trim();
        if !is_safe_identifier(name) {
            return Err(format!(
                "Invalid field name '{name}'. Use only letters, numbers, and underscores; do not start with a number."
            ));
        }
        if !seen.insert(name.to_lowercase()) {
            return Err(format!("Duplicate field name '{name}'."));
        }
    }

    Ok(())
}

fn is_safe_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use reqwest::Client;
    use sqlx::{MySql, Pool};
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_TOKEN: &str = "test-token";

    #[tokio::test]
    async fn update_item_updates_mysql_row() {
        let database_url = database_url();
        let pool = test_pool(&database_url).await;
        let itemid = unique_itemid("success");
        seed_price(&pool, &itemid, "10.00", "Credits").await;

        let mut server = start_test_server().await;
        let client = Client::new();
        let response = client
            .post(format!(
                "http://{}{}",
                server.bind_address, UPDATE_ITEM_PATH
            ))
            .header("X-API-Token", TEST_TOKEN)
            .json(&serde_json::json!({
                "itemid": itemid,
                "price": "250.00",
                "denomination": "USD"
            }))
            .send()
            .await
            .expect("send update request");

        assert_eq!(response.status().as_u16(), StatusCode::OK.as_u16());
        let body: serde_json::Value = response.json().await.expect("parse success response");
        assert_eq!(body["status"], "success");
        assert_eq!(body["message"], "successfully updated");

        let row: (String, String) =
            sqlx::query_as("SELECT CAST(price AS CHAR), denomination FROM prices WHERE itemid = ?")
                .bind(&itemid)
                .fetch_one(&pool)
                .await
                .expect("fetch updated row");
        assert_eq!(row.0, "250.00");
        assert_eq!(row.1, "USD");

        delete_price(&pool, &itemid).await;
        server.stop();
    }

    #[tokio::test]
    async fn update_item_requires_valid_token() {
        let mut server = start_test_server().await;
        let client = Client::new();
        let response = client
            .post(format!(
                "http://{}{}",
                server.bind_address, UPDATE_ITEM_PATH
            ))
            .json(&serde_json::json!({
                "itemid": "missing-token",
                "price": "1.00",
                "denomination": "Credits"
            }))
            .send()
            .await
            .expect("send unauthenticated request");

        assert_eq!(
            response.status().as_u16(),
            StatusCode::UNAUTHORIZED.as_u16()
        );
        let body: serde_json::Value = response.json().await.expect("parse error response");
        assert_eq!(body["status"], "error");

        server.stop();
    }

    #[tokio::test]
    async fn update_item_rejects_invalid_payload() {
        let mut server = start_test_server().await;
        let client = Client::new();
        let response = client
            .post(format!(
                "http://{}{}",
                server.bind_address, UPDATE_ITEM_PATH
            ))
            .header("X-API-Token", TEST_TOKEN)
            .json(&serde_json::json!({
                "itemid": "bad-payload",
                "price": "not-a-number",
                "denomination": "Credits"
            }))
            .send()
            .await
            .expect("send invalid request");

        assert_eq!(response.status().as_u16(), StatusCode::BAD_REQUEST.as_u16());
        let body: serde_json::Value = response.json().await.expect("parse error response");
        assert_eq!(body["status"], "error");

        server.stop();
    }

    #[tokio::test]
    async fn update_item_returns_not_found_for_unknown_itemid() {
        let mut server = start_test_server().await;
        let client = Client::new();
        let response = client
            .post(format!(
                "http://{}{}",
                server.bind_address, UPDATE_ITEM_PATH
            ))
            .header("X-API-Token", TEST_TOKEN)
            .json(&serde_json::json!({
                "itemid": unique_itemid("missing"),
                "price": "25.00",
                "denomination": "Credits"
            }))
            .send()
            .await
            .expect("send missing row request");

        assert_eq!(response.status().as_u16(), StatusCode::NOT_FOUND.as_u16());
        let body: serde_json::Value = response.json().await.expect("parse error response");
        assert_eq!(body["status"], "error");

        server.stop();
    }

    async fn start_test_server() -> ServerHandle {
        start_server(
            ServerConfig {
                mysql_host: "127.0.0.1".into(),
                mysql_port: 3307,
                mysql_database: "bugitik_test".into(),
                mysql_username: "bugitik".into(),
                mysql_password: "bugitik".into(),
                bind_host: "127.0.0.1".into(),
                server_port: 0,
                api_token: TEST_TOKEN.into(),
                table_name: "prices".into(),
                fields: vec![
                    ColumnField {
                        name: "itemid".into(),
                        field_type: FieldType::String,
                        is_key: true,
                    },
                    ColumnField {
                        name: "price".into(),
                        field_type: FieldType::Float,
                        is_key: false,
                    },
                    ColumnField {
                        name: "denomination".into(),
                        field_type: FieldType::String,
                        is_key: false,
                    },
                ],
            },
            Arc::new(Mutex::new(VecDeque::new())),
        )
        .await
        .expect("start test server")
    }

    async fn test_pool(database_url: &str) -> Pool<MySql> {
        MySqlPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await
            .expect("connect to test database")
    }

    async fn seed_price(pool: &Pool<MySql>, itemid: &str, price: &str, denomination: &str) {
        sqlx::query(
            "INSERT INTO prices (itemid, price, denomination) VALUES (?, ?, ?) \
             ON DUPLICATE KEY UPDATE price = VALUES(price), denomination = VALUES(denomination)",
        )
        .bind(itemid)
        .bind(price)
        .bind(denomination)
        .execute(pool)
        .await
        .expect("seed price row");
    }

    async fn delete_price(pool: &Pool<MySql>, itemid: &str) {
        sqlx::query("DELETE FROM prices WHERE itemid = ?")
            .bind(itemid)
            .execute(pool)
            .await
            .expect("delete price row");
    }

    fn database_url() -> String {
        std::env::var("BUGITIK_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "mysql://bugitik:bugitik@127.0.0.1:3307/bugitik_test".to_string())
    }

    fn unique_itemid(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        format!("{prefix}-{nanos}")
    }
}
