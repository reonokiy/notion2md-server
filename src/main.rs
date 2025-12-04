use std::{net::SocketAddr, time::Instant};

use axum::{
    Json, Router,
    body::Body,
    extract::{FromRequestParts, Path, Query},
    http::{HeaderMap, Request, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use axum_extra::headers::authorization::Bearer;
use axum_extra::headers::{Authorization, HeaderMapExt};
use log::{error, info, warn};
use logforth::{filter::env_filter::EnvFilterBuilder, starter_log};
use notion_client::endpoints::Client as NotionClient;
use notion_client::endpoints::databases::query::request::QueryDatabaseRequest;
use notion_client::objects::page::{
    DateOrDateTime, DatePropertyValue, Page as NotionPage, PageProperty as NotionPageProperty,
};
use notion_client::objects::rich_text::RichText;
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
    MaybeBearerToken(token): MaybeBearerToken,
) -> Result<Json<Page>, StatusCode> {
    if !accepts_json(&headers) {
        warn!("missing json Accept header for {id}");
        return Err(StatusCode::NOT_ACCEPTABLE);
    }

    if id.contains('/') || id.contains("..") {
        warn!("invalid page id: {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let token = notion_token_from_header(token)?;
    let client = notion_client_from_token(&token)?;
    let converter = NotionToMarkdownBuilder::new(client.clone()).build();

    let markdown = converter.convert_page(&id).await.map_err(|err| {
        error!("failed to render notion page {id}: {err:?}");
        StatusCode::BAD_GATEWAY
    })?;

    let notion_page = client
        .pages
        .retrieve_a_page(&id, None)
        .await
        .map_err(|err| {
            error!("failed to retrieve notion page {id}: {err:?}");
            StatusCode::BAD_GATEWAY
        })?;

    let page = notion_page_to_response(notion_page, Some(markdown));

    Ok(Json(page))
}

#[derive(Serialize)]
struct Page {
    id: String,
    properties: Vec<PagePropertyItem>,
    markdown: Option<String>,
}

#[derive(Serialize, Clone)]
struct PagePropertyItem {
    id: String,
    #[serde(flatten)]
    value: PropertyValue,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", content = "content")]
enum PropertyValue {
    Title(String),
    RichText(String),
    Select(String),
    MultiSelect(Vec<String>),
    Status(String),
    Checkbox(bool),
    Number(f64),
    Url(String),
    Email(String),
    Phone(String),
    Date(String),
}

#[derive(Deserialize)]
struct ListDatabaseParams {
    start: Option<usize>,
    page_size: Option<usize>,
}

#[derive(Serialize)]
struct ListDatabasePagesResponse {
    total: usize,
    pages: Vec<String>,
    start: usize,
    page_size: usize,
}

async fn list_database_pages(
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<ListDatabaseParams>,
    MaybeBearerToken(token): MaybeBearerToken,
) -> Result<Json<ListDatabasePagesResponse>, StatusCode> {
    if !accepts_json(&headers) {
        warn!("missing json Accept header for {id}");
        return Err(StatusCode::NOT_ACCEPTABLE);
    }

    if id.contains('/') || id.contains("..") {
        warn!("invalid database id: {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let token = notion_token_from_header(token)?;
    let notion_client = notion_client_from_token(&token)?;
    let start = params.start.unwrap_or(0);
    let page_size = params.page_size.unwrap_or(100);
    if page_size == 0 {
        warn!("page_size of zero requested for database {id}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut cursor: Option<String> = None;
    let mut skipped = 0usize;
    let mut pages: Vec<String> = Vec::with_capacity(page_size);

    while pages.len() < page_size {
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
                error!("failed to query notion database {id}: {err:?}");
                StatusCode::BAD_GATEWAY
            })?;

        let next_cursor = response.next_cursor.clone();

        for page in response.results {
            if skipped < start {
                skipped += 1;
                continue;
            }

            if pages.len() < page_size {
                pages.push(page.id);
            } else {
                break;
            }
        }

        if pages.len() >= page_size || next_cursor.is_none() {
            break;
        }

        cursor = next_cursor;
    }

    let size = pages.len();

    Ok(Json(ListDatabasePagesResponse {
        total: size,
        pages,
        start,
        page_size,
    }))
}

fn notion_page_to_response(page: NotionPage, markdown: Option<String>) -> Page {
    let mut properties = Vec::new();

    for (_key, property) in page.properties.into_iter() {
        if let Ok(value) = PagePropertyItem::try_from(property) {
            properties.push(value);
        }
    }

    Page {
        id: page.id,
        properties,
        markdown,
    }
}

impl TryFrom<NotionPageProperty> for PagePropertyItem {
    type Error = ();

    fn try_from(property: NotionPageProperty) -> Result<Self, Self::Error> {
        match property {
            NotionPageProperty::Title { id, title } => rich_text_to_string(&title)
                .map(|text| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::Title(text),
                })
                .ok_or(()),
            NotionPageProperty::RichText { id, rich_text } => rich_text_to_string(&rich_text)
                .map(|text| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::RichText(text),
                })
                .ok_or(()),
            NotionPageProperty::Select { id, select } => select
                .and_then(|value| value.name)
                .map(|selected| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::Select(selected),
                })
                .ok_or(()),
            NotionPageProperty::Status { id, status } => status
                .and_then(|value| value.name)
                .map(|name| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::Status(name),
                })
                .ok_or(()),
            NotionPageProperty::MultiSelect { id, multi_select } => {
                let values: Vec<String> = multi_select
                    .into_iter()
                    .filter_map(|item| item.name)
                    .collect();

                if values.is_empty() {
                    Err(())
                } else {
                    Ok(PagePropertyItem {
                        id: id.unwrap_or_default(),
                        value: PropertyValue::MultiSelect(values),
                    })
                }
            }
            NotionPageProperty::Checkbox { id, checkbox } => Ok(PagePropertyItem {
                id: id.unwrap_or_default(),
                value: PropertyValue::Checkbox(checkbox),
            }),
            NotionPageProperty::Number { id, number } => number
                .and_then(|value| value.as_f64())
                .map(|number| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::Number(number),
                })
                .ok_or(()),
            NotionPageProperty::Url { id, url } => url
                .map(|value| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::Url(value),
                })
                .ok_or(()),
            NotionPageProperty::Email { id, email } => email
                .map(|value| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::Email(value),
                })
                .ok_or(()),
            NotionPageProperty::PhoneNumber { id, phone_number } => phone_number
                .map(|value| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::Phone(value),
                })
                .ok_or(()),
            NotionPageProperty::Date { id, date } => date
                .and_then(date_to_string)
                .map(|value| PagePropertyItem {
                    id: id.unwrap_or_default(),
                    value: PropertyValue::Date(value),
                })
                .ok_or(()),
            _ => Err(()),
        }
    }
}

fn rich_text_to_string(text: &[RichText]) -> Option<String> {
    let combined = text
        .iter()
        .filter_map(|item| item.plain_text())
        .collect::<String>();

    let trimmed = combined.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn date_to_string(date: DatePropertyValue) -> Option<String> {
    date.start.map(date_or_datetime_to_string)
}

fn date_or_datetime_to_string(date: DateOrDateTime) -> String {
    match date {
        DateOrDateTime::Date(date) => date.to_string(),
        DateOrDateTime::DateTime(date_time) => date_time.to_rfc3339(),
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

fn accepts_json(headers: &HeaderMap) -> bool {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok());

    match accept {
        None => true,
        Some(value) => value.split(',').map(str::trim).any(|item| {
            item.starts_with("application/json")
                || item.starts_with("application/*")
                || item == "*/*"
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
