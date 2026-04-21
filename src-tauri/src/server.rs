use axum::{
    extract::ConnectInfo,
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize};
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
    pub item_id_column: String,
    pub price_column: String,
    pub denomination_column: String,
    pub item_id_type: ItemIdType,
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
            item_id_column: "itemid".into(),
            price_column: "price".into(),
            denomination_column: "denomination".into(),
            item_id_type: ItemIdType::String,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemIdType {
    Integer,
    String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePayload {
    #[serde(deserialize_with = "deserialize_stringish")]
    pub itemid: String,
    #[serde(deserialize_with = "deserialize_stringish")]
    pub price: String,
    #[serde(deserialize_with = "deserialize_stringish")]
    pub denomination: String,
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
    payload: Result<Json<UpdatePayload>, JsonRejection>,
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

    let Ok(Json(payload)) = payload else {
        let response = api_error(
            StatusCode::BAD_REQUEST,
            "Invalid input. Fields required: itemid, price, denomination.",
        );
        log_response(
            &state,
            remote_addr,
            &method,
            &uri,
            &response,
            started,
            None,
            "Invalid input. Fields required: itemid, price, denomination.",
        );
        return response;
    };

    let itemid = Some(payload.itemid.clone());
    if payload.itemid.trim().is_empty()
        || payload.price.trim().is_empty()
        || payload.denomination.trim().is_empty()
    {
        let response = api_error(
            StatusCode::BAD_REQUEST,
            "Invalid input. Fields required: itemid, price, denomination.",
        );
        log_response(
            &state,
            remote_addr,
            &method,
            &uri,
            &response,
            started,
            itemid,
            "Invalid input. Fields required: itemid, price, denomination.",
        );
        return response;
    }

    if payload.price.parse::<f64>().is_err() {
        let response = api_error(
            StatusCode::BAD_REQUEST,
            "Invalid input. price must be numeric.",
        );
        log_response(
            &state,
            remote_addr,
            &method,
            &uri,
            &response,
            started,
            itemid,
            "Invalid input. price must be numeric.",
        );
        return response;
    }

    let response = match execute_update(&state.config, &state.pool, &payload).await {
        Ok(0) => api_error(StatusCode::NOT_FOUND, "No matching itemid was found."),
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
    let sql = format!(
        "UPDATE `{}` SET `{}` = ?, `{}` = ? WHERE `{}` = ?",
        config.table_name, config.price_column, config.denomination_column, config.item_id_column
    );

    let query = sqlx::query(&sql)
        .bind(payload.price.trim())
        .bind(payload.denomination.trim());

    let result = match config.item_id_type {
        ItemIdType::Integer => {
            let item_id = payload
                .itemid
                .trim()
                .parse::<i64>()
                .map_err(|_| "Invalid input. itemid must be an integer.".to_string())?;
            query.bind(item_id).execute(pool).await
        }
        ItemIdType::String => query.bind(payload.itemid.trim()).execute(pool).await,
    };

    result
        .map(|done| done.rows_affected())
        .map_err(|error| format!("Database update failed: {error}"))
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

fn deserialize_stringish<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(value) => Ok(value),
        serde_json::Value::Number(value) => Ok(value.to_string()),
        _ => Err(D::Error::custom("expected a string or number")),
    }
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

    for (label, identifier) in [
        ("table name", &config.table_name),
        ("item ID column", &config.item_id_column),
        ("price column", &config.price_column),
        ("denomination column", &config.denomination_column),
    ] {
        if !is_safe_identifier(identifier) {
            return Err(format!(
                "Invalid {label}. Use only letters, numbers, and underscores; do not start with a number."
            ));
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
                item_id_column: "itemid".into(),
                price_column: "price".into(),
                denomination_column: "denomination".into(),
                item_id_type: ItemIdType::String,
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
