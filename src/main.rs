use std::{net::SocketAddr, time::Instant};

use axum::{
    Router,
    body::Body,
    extract::{FromRequestParts, Path},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use axum_extra::headers::authorization::Bearer;
use axum_extra::headers::{Authorization, HeaderMapExt};
use log::{error, info, warn};
use logforth::{filter::env_filter::EnvFilterBuilder, starter_log};
use notion_client::endpoints::Client as NotionClient;
use notion_client::endpoints::databases::query::request::QueryDatabaseRequest;
use notion_client::objects::page::{Page, PageProperty};
use notion2md::builder::NotionToMarkdownBuilder;
use notion2md::notion_to_md::NotionToMarkdown;

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

struct MaybeBearerToken(Option<String>);

impl<S> FromRequestParts<S> for MaybeBearerToken
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let headers = parts.headers.clone();

        let token = headers
            .typed_get::<Authorization<Bearer>>()
            .map(|Authorization(bearer)| bearer.token().to_string())
            .or_else(|| {
                headers.get("Auth").and_then(|value| match value.to_str() {
                    Ok(value) => {
                        let trimmed = value.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_string())
                        }
                    }
                    Err(_) => {
                        warn!("failed to read Auth header as UTF-8");
                        None
                    }
                })
            });

        async move { Ok(MaybeBearerToken(token)) }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    starter_log::stdout()
        .filter(EnvFilterBuilder::from_default_env_or("info").build())
        .apply();

    let app = Router::new()
        .route("/page/{id}", get(get_markdown_page))
        .route("/database/{id}", get(list_database_pages))
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
    MaybeBearerToken(token): MaybeBearerToken,
) -> Result<MarkdownResponse, StatusCode> {
    if !accepts_markdown(&headers) {
        warn!("missing markdown Accept header for {id}");
        return Err(StatusCode::NOT_ACCEPTABLE);
    }

    if id.contains('/') || id.contains("..") {
        warn!("invalid page id: {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let token = notion_token_from_header(token)?;
    let converter = markdown_converter_from_token(&token)?;

    let markdown = converter.convert_page(&id).await.map_err(|err| {
        error!("failed to render notion page {id}: {err:?}");
        StatusCode::BAD_GATEWAY
    })?;

    Ok(markdown.into())
}

async fn list_database_pages(
    Path(id): Path<String>,
    headers: HeaderMap,
    MaybeBearerToken(token): MaybeBearerToken,
) -> Result<MarkdownResponse, StatusCode> {
    if !accepts_markdown(&headers) {
        warn!("missing markdown Accept header for {id}");
        return Err(StatusCode::NOT_ACCEPTABLE);
    }

    if id.contains('/') || id.contains("..") {
        warn!("invalid database id: {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let token = notion_token_from_header(token)?;
    let notion_client = notion_client_from_token(&token)?;

    let response = notion_client
        .databases
        .query_a_database(&id, QueryDatabaseRequest::default())
        .await
        .map_err(|err| {
            error!("failed to query notion database {id}: {err:?}");
            StatusCode::BAD_GATEWAY
        })?;

    let markdown = build_database_markdown(&id, &response.results);
    Ok(markdown.into())
}

fn build_database_markdown(id: &str, pages: &[Page]) -> String {
    let mut lines = vec![format!("# Database {id}"), String::new()];

    if pages.is_empty() {
        lines.push("_No pages found._".to_string());
    } else {
        lines.extend(pages.iter().map(|page| {
            let title = page_title(page);
            format!("- [{title}]({})", page.url)
        }));
    }

    lines.join("\n")
}

fn page_title(page: &Page) -> String {
    page.properties
        .values()
        .find_map(|property| {
            if let PageProperty::Title { title, .. } = property {
                let text = title
                    .iter()
                    .filter_map(|rich| rich.plain_text())
                    .collect::<String>();
                let trimmed = text.trim();

                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            } else {
                None
            }
        })
        .unwrap_or_else(|| "Untitled page".to_string())
}

fn notion_token_from_header(token: Option<String>) -> Result<String, StatusCode> {
    token.ok_or_else(|| {
        warn!("missing Notion token in request headers");
        StatusCode::UNAUTHORIZED
    })
}

fn notion_client_from_token(token: &str) -> Result<NotionClient, StatusCode> {
    NotionClient::new(token.to_string(), None).map_err(|err| {
        error!("failed to create notion client from header token: {err:?}");
        StatusCode::UNAUTHORIZED
    })
}

fn markdown_converter_from_token(token: &str) -> Result<NotionToMarkdown, StatusCode> {
    let client = notion_client_from_token(token)?;
    Ok(NotionToMarkdownBuilder::new(client).build())
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
