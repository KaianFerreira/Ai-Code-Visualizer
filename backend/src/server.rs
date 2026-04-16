//! HTTP API for triggering repository scans from the frontend.

use std::net::SocketAddr;

use axum::extract::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use tower_http::cors::{Any, CorsLayer};

use crate::scan;

#[derive(Deserialize)]
pub struct ScanRequest {
    pub url: String,
}

async fn scan_handler(Json(body): Json<ScanRequest>) -> impl IntoResponse {
    let url = body.url.trim();
    if url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "url must not be empty" })),
        )
            .into_response();
    }

    let url_owned = url.to_string();
    let outcome = tokio::task::spawn_blocking(move || {
        let (result, temp) = scan::analyze_source(Some(&url_owned))
            .map_err(|e| e.to_string())?;
        let json = serde_json::to_value(&result).map_err(|e| e.to_string())?;
        drop(temp);
        Ok::<_, String>(json)
    })
    .await;

    match outcome {
        Ok(Ok(json)) => (StatusCode::OK, Json(json)).into_response(),
        Ok(Err(msg)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task join: {e}") })),
        )
            .into_response(),
    }
}

pub async fn run() -> anyhow::Result<()> {
    let port: u16 = std::env::var("SCANNER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8787);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([axum::http::Method::POST, axum::http::Method::OPTIONS])
        .allow_headers(Any);

    let app = Router::new()
        .route("/api/scan", post(scan_handler))
        .layer(cors);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Scanner API listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
