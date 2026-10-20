//! Test harness: a real server on a random port, backed by its own PostgreSQL
//! schema so tests can run in parallel against one database.
//!
//! Tests are skipped (they pass without doing anything) when `DATABASE_URL`
//! is not set, e.g. `DATABASE_URL=postgres://postgres:postgres@localhost/postgres`.

#![allow(dead_code)]

use std::sync::Arc;

use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sivryn_protocol::api::{RegistrationMode, SessionResponse};
use sivryn_protocol::media::{CameraLimits, ScreenShareLimits};
use sivryn_protocol::model::User;
use sivryn_server::config::{Config, LiveKitConfig};
use sivryn_server::events::EventBus;
use sivryn_server::{AppState, migrate, router};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{AssertSqlSafe, PgPool};

pub const PASSWORD: &str = "correct horse battery";
pub const LIVEKIT_SECRET: &str = "test-secret-that-is-long-enough-for-hs256";

pub struct TestApp {
    pub url: String,
    pub db: PgPool,
    http: reqwest::Client,
    admin: PgPool,
    schema: String,
}

/// A registered user with a session token.
#[derive(Clone)]
pub struct TestUser {
    pub token: String,
    pub user: User,
}

impl TestApp {
    /// Starts a server, or returns `None` when no database is configured.
    pub async fn spawn(registration: RegistrationMode) -> Option<Self> {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            // In CI a missing database is a misconfiguration, not a reason to skip.
            assert!(std::env::var_os("CI").is_none(), "DATABASE_URL must be set in CI");
            eprintln!("DATABASE_URL not set; skipping integration test");
            return None;
        };

        let admin = PgPool::connect(&database_url).await.expect("connect to DATABASE_URL");
        let schema = format!("test_{}", uuid::Uuid::now_v7().simple());
        // The schema name is generated here, never user input.
        sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}"))).execute(&admin).await.expect("create schema");

        let options: PgConnectOptions = database_url.parse().expect("valid DATABASE_URL");
        let db = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(options.options([("search_path", schema.as_str())]))
            .await
            .expect("connect test pool");
        migrate(&db).await.expect("migrations apply");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = Config {
            bind: addr,
            instance_name: "Test".into(),
            database_url,
            registration,
            session_ttl_days: 1,
            livekit: LiveKitConfig {
                public_url: "wss://livekit.test".into(),
                // Nothing listens here: voice presence must degrade gracefully.
                api_url: "http://127.0.0.1:9".into(),
                api_key: "test-key".into(),
                api_secret: LIVEKIT_SECRET.into(),
            },
            screen_share: ScreenShareLimits::default(),
            camera: CameraLimits::default(),
            nats_url: None,
            oidc: None,
            files_dir: std::env::temp_dir().join(format!("sivryn-test-files-{schema}")),
            max_upload_bytes: 1024 * 1024,
        };
        let state = AppState {
            db: db.clone(),
            config: Arc::new(config),
            events: EventBus::in_process(),
            presence: Default::default(),
        };
        tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });

        Some(Self { url: format!("http://{addr}"), db, http: reqwest::Client::new(), admin, schema })
    }

    /// Drops the test schema. Tests that panic leave theirs behind, which is
    /// harmless on a throwaway database.
    pub async fn cleanup(self) {
        self.db.close().await;
        sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {} CASCADE", self.schema))).execute(&self.admin).await.unwrap();
    }

    pub async fn request<B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<&B>,
    ) -> (StatusCode, Value) {
        let mut req = self.http.request(method, format!("{}/api/v1{path}", self.url));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await.expect("request sent");
        let status = resp.status();
        let text = resp.text().await.unwrap();
        let json =
            if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text)) };
        (status, json)
    }

    pub async fn get(&self, path: &str, token: &str) -> (StatusCode, Value) {
        self.request::<()>(reqwest::Method::GET, path, Some(token), None).await
    }

    pub async fn post(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(reqwest::Method::POST, path, Some(token), Some(&body)).await
    }

    /// POSTs and deserializes a successful response, failing the test otherwise.
    pub async fn post_ok<T: DeserializeOwned>(&self, path: &str, token: &str, body: Value) -> T {
        let (status, json) = self.post(path, token, body).await;
        assert!(status.is_success(), "POST {path} failed with {status}: {json}");
        serde_json::from_value(json).unwrap()
    }

    pub async fn get_ok<T: DeserializeOwned>(&self, path: &str, token: &str) -> T {
        let (status, json) = self.get(path, token).await;
        assert!(status.is_success(), "GET {path} failed with {status}: {json}");
        serde_json::from_value(json).unwrap()
    }

    pub async fn put(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(reqwest::Method::PUT, path, Some(token), Some(&body)).await
    }

    pub async fn patch(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(reqwest::Method::PATCH, path, Some(token), Some(&body)).await
    }

    pub async fn delete(&self, path: &str, token: &str) -> (StatusCode, Value) {
        self.request::<()>(reqwest::Method::DELETE, path, Some(token), None).await
    }

    /// Sends raw bytes (uploads) and returns status and JSON.
    pub async fn send_bytes(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> (StatusCode, Value) {
        let resp = self
            .http
            .request(method, format!("{}/api/v1{path}", self.url))
            .bearer_auth(token)
            .header("content-type", content_type)
            .body(bytes)
            .send()
            .await
            .expect("request sent");
        let status = resp.status();
        let text = resp.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// GETs raw bytes.
    pub async fn get_bytes(&self, path: &str, token: &str) -> (StatusCode, Vec<u8>) {
        let resp =
            self.http.get(format!("{}/api/v1{path}", self.url)).bearer_auth(token).send().await.expect("request sent");
        (resp.status(), resp.bytes().await.unwrap().to_vec())
    }

    pub async fn try_register(&self, username: &str, invite_code: Option<&str>) -> (StatusCode, Value) {
        let body = serde_json::json!({ "username": username, "password": PASSWORD, "invite_code": invite_code });
        self.request(reqwest::Method::POST, "/auth/register", None, Some(&body)).await
    }

    pub async fn register(&self, username: &str) -> TestUser {
        let (status, json) = self.try_register(username, None).await;
        assert_eq!(status, StatusCode::CREATED, "register {username}: {json}");
        let session: SessionResponse = serde_json::from_value(json).unwrap();
        TestUser { token: session.token, user: session.user }
    }
}
