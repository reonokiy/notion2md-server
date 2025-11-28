use std::{net::SocketAddr, time::Instant};

use axum::{
    Router,
    body::Body,
    extract::Path,
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use log::{error, info, warn};
use logforth::{append, filter::EnvFilter};
use notion_client::endpoints::Client as NotionClient;
use notion2md::builder::NotionToMarkdownBuilder;

struct MarkdownResponse(String);

impl From<String> for MarkdownResponse {
    fn from(content: String) -> Self {
        MarkdownResponse(content)
    }
}

impl IntoResponse for MarkdownResponse {
    fn into_response(self) -> axum::response::Response {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/markdown; charset=utf-8"),
        );

        (headers, self.0).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    logforth::builder()
        .dispatch(|d| {
            d.filter(EnvFilter::from_default_env_or("info"))
                .append(append::Stdout::default())
        })
        .apply();

    let app = Router::new()
        .route("/page/:id", get(get_markdown_page))
        .layer(middleware::from_fn(log_requests));

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    info!("listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_markdown_page(
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<MarkdownResponse, StatusCode> {
    if !accepts_markdown(&headers) {
        warn!("missing markdown Accept header for {id}");
        return Err(StatusCode::NOT_ACCEPTABLE);
    }

    if id.contains('/') || id.contains("..") {
        warn!("invalid page id: {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let notion_token = extract_notion_token(&headers)?;
    let notion_client = NotionClient::new(notion_token, None).map_err(|err| {
        error!("failed to create notion client for {id}: {err:?}");
        StatusCode::UNAUTHORIZED
    })?;

    let markdown = NotionToMarkdownBuilder::new(notion_client)
        .build()
        .convert_page(&id)
        .await
        .map_err(|err| {
            error!("failed to render notion page {id}: {err:?}");
            StatusCode::BAD_GATEWAY
        })?;

    Ok(markdown.into())
}

fn accepts_markdown(headers: &HeaderMap) -> bool {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok());

    match accept {
        None => true,
        Some(value) => value.split(',').map(str::trim).any(|item| {
            item.starts_with("text/markdown") || item.starts_with("text/*") || item == "*/*"
        }),
    }
}

fn extract_notion_token(headers: &HeaderMap) -> Result<String, StatusCode> {
    let token_value = headers
        .get("Auth")
        .or_else(|| headers.get(header::AUTHORIZATION))
        .and_then(|value| value.to_str().ok())
        .map(str::trim);

    let Some(value) = token_value else {
        warn!("missing Auth header");
        return Err(StatusCode::UNAUTHORIZED);
    };

    let token = if let Some(stripped) = value.strip_prefix("Bearer ") {
        stripped.trim()
    } else {
        value
    };

    if token.is_empty() {
        warn!("Auth header present but empty");
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(token.to_string())
}

async fn log_requests(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let start = Instant::now();

    let response = next.run(req).await;
    let status = response.status();
    let elapsed_ms = start.elapsed().as_millis();

    info!(
        "handled {method} {path} -> {} in {}ms",
        status.as_u16(),
        elapsed_ms
    );

    response
}
