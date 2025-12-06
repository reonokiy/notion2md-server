use std::{collections::HashMap, net::SocketAddr, time::Instant};

use axum::{
    Json, Router,
    body::Body,
    extract::{FromRequestParts, Path, Query},
    http::{HeaderMap, Request, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use axum_extra::headers::authorization::Bearer;
use axum_extra::headers::{Authorization, HeaderMapExt};
use log::{error, info, warn};
use logforth::{filter::env_filter::EnvFilterBuilder, starter_log};
use notion_client::NotionClientError;
use notion_client::endpoints::Client as NotionClient;
use notion_client::endpoints::databases::query::request::QueryDatabaseRequest;
use notion_opendal::notion::{PropertyValue, apply_frontmatter, notion_page_to_properties};
use notion2md::builder::NotionToMarkdownBuilder;
use serde::{Deserialize, Serialize};

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
        .route("/page/{id}", get(get_page))
        .route("/database/{id}", get(list_database_pages))
        .layer(middleware::from_fn(log_requests));

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    info!("listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_page(
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<GetPageParams>,
    MaybeBearerToken(token): MaybeBearerToken,
) -> Result<Response, StatusCode> {
    if id.contains('/') || id.contains("..") {
        warn!("invalid page id: {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let token = notion_token_from_header(token)?;
    let client = notion_client_from_token(&token)?;
    let converter = NotionToMarkdownBuilder::new(client.clone()).build();
    let format = page_response_format(&headers);

    let notion_page = client
        .pages
        .retrieve_a_page(&id, None)
        .await
        .map_err(|err| {
            let status = map_notion_error(&err);
            error!("failed to retrieve notion page {id}: {err:?}");
            status
        })?;

    let properties = notion_page_to_properties(&notion_page);

    let markdown = converter.convert_page(&id).await.map_err(|err| {
        error!("failed to render notion page {id}: {err:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match format {
        PageResponseFormat::Json => {
            let response = PageJsonResponse {
                id: notion_page.id.clone(),
                properties,
                content: markdown,
            };
            Ok(Json(response).into_response())
        }
        PageResponseFormat::Markdown => {
            let content = if params.frontmatter.unwrap_or(false) {
                apply_frontmatter(&properties, &markdown)
            } else {
                markdown
            };
            Ok((
                [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
                content,
            )
                .into_response())
        }
    }
}

#[derive(Deserialize)]
struct GetPageParams {
    frontmatter: Option<bool>,
}

#[derive(Serialize)]
struct PageJsonResponse {
    id: String,
    properties: HashMap<String, PropertyValue>,
    content: String,
}

#[derive(Deserialize)]
struct ListDatabaseParams {
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct ListDatabasePagesResponse {
    total: usize,
    offset: usize,
    limit: usize,
    pages: Vec<String>,
}

async fn list_database_pages(
    Path(id): Path<String>,
    Query(params): Query<ListDatabaseParams>,
    MaybeBearerToken(token): MaybeBearerToken,
) -> Result<Json<ListDatabasePagesResponse>, StatusCode> {
    if id.contains('/') || id.contains("..") {
        warn!("invalid database id: {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let token = notion_token_from_header(token)?;
    let notion_client = notion_client_from_token(&token)?;
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(20);
    if limit == 0 {
        warn!("limit of zero requested for database {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut cursor: Option<String> = None;
    let mut skipped = 0_usize;
    let mut total = 0_usize;
    let mut pages: Vec<String> = Vec::with_capacity(limit);

    loop {
        let request = QueryDatabaseRequest {
            start_cursor: cursor.clone(),
            page_size: Some(100),
            ..Default::default()
        };

        let response = notion_client
            .databases
            .query_a_database(&id, request)
            .await
            .map_err(|err| {
                let status = map_notion_error(&err);
                error!("failed to query notion database {id}: {err:?}");
                status
            })?;

        let next_cursor = response.next_cursor.clone();
        total += response.results.len();

        for page in response.results {
            if skipped < offset {
                skipped += 1;
                continue;
            }

            if pages.len() < limit {
                pages.push(page.id);
            }
        }

        if next_cursor.is_none() {
            break;
        }

        cursor = next_cursor;
    }

    Ok(Json(ListDatabasePagesResponse {
        total,
        pages,
        offset,
        limit,
    }))
}

enum PageResponseFormat {
    Json,
    Markdown,
}

fn page_response_format(headers: &HeaderMap) -> PageResponseFormat {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());

    if let Some(content_type) = content_type {
        if content_type.starts_with("text/markdown") {
            return PageResponseFormat::Markdown;
        }
    }

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok());

    if let Some(value) = accept {
        for item in value.split(',').map(str::trim) {
            if item.starts_with("text/markdown") || item.starts_with("text/*") {
                return PageResponseFormat::Markdown;
            }

            if item.starts_with("application/json") || item.starts_with("application/*") {
                return PageResponseFormat::Json;
            }

            if item == "*/*" {
                return PageResponseFormat::Json;
            }
        }
    }

    PageResponseFormat::Json
}

fn map_notion_error(err: &NotionClientError) -> StatusCode {
    match err {
        NotionClientError::InvalidStatusCode { error } => match error.status {
            400 => StatusCode::BAD_REQUEST,
            401 | 403 => StatusCode::UNAUTHORIZED,
            404 => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        },
        NotionClientError::InvalidHeader { .. } => StatusCode::UNAUTHORIZED,
        NotionClientError::FailedToSerialize { .. }
        | NotionClientError::FailedToDeserialize { .. }
        | NotionClientError::FailedToRequest { .. }
        | NotionClientError::FailedToText { .. }
        | NotionClientError::FailedToBuildRequest { .. } => StatusCode::INTERNAL_SERVER_ERROR,
    }
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
