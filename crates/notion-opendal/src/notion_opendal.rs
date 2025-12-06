use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use log::error;
use notion2md::builder::NotionToMarkdownBuilder;
use notion_client::endpoints::databases::query::request::QueryDatabaseRequest;
use notion_client::endpoints::Client as NotionClient;
use notion_client::NotionClientError;
use opendal::raw::oio;
use opendal::raw::{Access, AccessorInfo, OpList, OpRead, OpStat, RpList, RpRead, RpStat};
use opendal::{
    Buffer, Builder, Capability, Configurator, EntryMode, Error, ErrorKind, Metadata, Result,
};

use crate::notion::{apply_frontmatter, notion_page_to_properties};

/// Config for the Notion read-only service.
#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NotionConfig {
    /// Notion integration token.
    pub token: Option<String>,
    /// Default database id to list pages from.
    pub database_id: Option<String>,
    /// Whether to prepend properties as frontmatter when reading.
    pub frontmatter: bool,
}

impl Configurator for NotionConfig {
    type Builder = NotionServiceBuilder;

    fn into_builder(self) -> Self::Builder {
        NotionServiceBuilder { config: self }
    }
}

/// Builder for the Notion service.
#[derive(Default, Clone)]
pub struct NotionServiceBuilder {
    config: NotionConfig,
}

impl Debug for NotionServiceBuilder {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotionServiceBuilder")
            .field("has_token", &self.config.token.as_ref().map(|_| "***"))
            .field("database_id", &self.config.database_id)
            .field("frontmatter", &self.config.frontmatter)
            .finish()
    }
}

impl NotionServiceBuilder {
    /// Set the token used to talk to Notion.
    pub fn token(mut self, token: &str) -> Self {
        if !token.is_empty() {
            self.config.token = Some(token.to_string());
        }
        self
    }

    /// Set the default database id used by list.
    pub fn database_id(mut self, database_id: &str) -> Self {
        if !database_id.is_empty() {
            self.config.database_id = Some(database_id.to_string());
        }
        self
    }

    /// Enable or disable frontmatter on page reads.
    pub fn frontmatter(mut self, enabled: bool) -> Self {
        self.config.frontmatter = enabled;
        self
    }
}

impl Builder for NotionServiceBuilder {
    type Config = NotionConfig;

    fn build(self) -> Result<impl Access> {
        let token = self
            .config
            .token
            .ok_or_else(|| Error::new(ErrorKind::ConfigInvalid, "notion token is required"))?;

        let client = NotionClient::new(token, None).map_err(|err| {
            Error::new(ErrorKind::ConfigInvalid, "failed to build notion client")
                .with_context("source", err.to_string())
        })?;

        let info = AccessorInfo::default();
        info.set_scheme("notion");
        info.set_root("/");
        info.set_native_capability(Capability {
            stat: true,
            read: true,
            list: self.config.database_id.is_some(),
            ..Default::default()
        });

        Ok(NotionAccessor {
            client,
            database_id: self.config.database_id,
            frontmatter: self.config.frontmatter,
            info: Arc::new(info),
        })
    }
}

#[derive(Clone)]
pub struct NotionAccessor {
    client: NotionClient,
    database_id: Option<String>,
    frontmatter: bool,
    info: Arc<AccessorInfo>,
}

impl Debug for NotionAccessor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotionAccessor")
            .field("database_id", &self.database_id)
            .field("frontmatter", &self.frontmatter)
            .finish()
    }
}

impl Access for NotionAccessor {
    type Reader = Buffer;
    type Writer = ();
    type Lister = NotionLister;
    type Deleter = ();

    fn info(&self) -> Arc<AccessorInfo> {
        self.info.clone()
    }

    async fn stat(&self, path: &str, _: OpStat) -> Result<RpStat> {
        if is_root(path) {
            return Ok(RpStat::new(Metadata::new(EntryMode::DIR)));
        }

        let page_id = parse_page_path(path)?;
        let page = self
            .client
            .pages
            .retrieve_a_page(&page_id, None)
            .await
            .map_err(map_notion_error)?;
        let properties = notion_page_to_properties(&page);

        let markdown = NotionToMarkdownBuilder::new(self.client.clone())
            .build()
            .convert_page(&page_id)
            .await
            .map_err(|err| {
                Error::new(ErrorKind::Unexpected, "failed to render notion page")
                    .with_context("source", err.to_string())
            })?;

        let content = if self.frontmatter {
            apply_frontmatter(&properties, &markdown)
        } else {
            markdown
        };

        let mut meta = Metadata::new(EntryMode::FILE);
        meta.set_content_length(content.as_bytes().len() as u64);
        meta.set_content_type("text/markdown");
        meta.set_last_modified(page.last_edited_time);

        Ok(RpStat::new(meta))
    }

    async fn read(&self, path: &str, args: OpRead) -> Result<(RpRead, Self::Reader)> {
        if !args.range().is_full() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "range reads are not supported for notion",
            ));
        }

        let page_id = parse_page_path(path)?;
        let page = self
            .client
            .pages
            .retrieve_a_page(&page_id, None)
            .await
            .map_err(map_notion_error)?;
        let properties = notion_page_to_properties(&page);

        let markdown = NotionToMarkdownBuilder::new(self.client.clone())
            .build()
            .convert_page(&page_id)
            .await
            .map_err(|err| {
                Error::new(ErrorKind::Unexpected, "failed to render notion page")
                    .with_context("source", err.to_string())
            })?;

        let content = if self.frontmatter {
            apply_frontmatter(&properties, &markdown)
        } else {
            markdown
        };

        let size = content.as_bytes().len() as u64;
        Ok((
            RpRead::new().with_size(Some(size)),
            Buffer::from(content.into_bytes()),
        ))
    }

    async fn list(&self, path: &str, _: OpList) -> Result<(RpList, Self::Lister)> {
        let Some(database_id) = &self.database_id else {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "list requires a database_id",
            ));
        };

        if !is_root_dir(path) {
            return Err(Error::new(
                ErrorKind::NotADirectory,
                "only root directory is listable",
            ));
        }

        let pages = list_database_pages(self.client.clone(), database_id).await?;
        Ok((RpList::default(), NotionLister::new(pages)))
    }
}

#[derive(Debug)]
pub struct NotionLister {
    pages: Vec<String>,
    idx: usize,
}

impl NotionLister {
    fn new(pages: Vec<String>) -> Self {
        Self { pages, idx: 0 }
    }
}

impl oio::List for NotionLister {
    async fn next(&mut self) -> Result<Option<oio::Entry>> {
        if self.idx >= self.pages.len() {
            return Ok(None);
        }

        let page_id = &self.pages[self.idx];
        self.idx += 1;

        let meta = Metadata::new(EntryMode::FILE).with_content_type("text/markdown".to_string());
        let path = format!("{page_id}.md");
        Ok(Some(oio::Entry::new(&path, meta)))
    }
}

fn parse_page_path(path: &str) -> Result<String> {
    if path.contains("..") || path.contains('/') {
        return Err(Error::new(
            ErrorKind::NotFound,
            "nested paths are not supported",
        ));
    }

    let trimmed = path.trim_end_matches(".md");
    if trimmed.is_empty() {
        Err(Error::new(
            ErrorKind::NotFound,
            "page id is required in path",
        ))
    } else {
        Ok(trimmed.to_string())
    }
}

fn is_root(path: &str) -> bool {
    path.is_empty() || path == "/"
}

fn is_root_dir(path: &str) -> bool {
    is_root(path) || path == "./" || path == "/."
}

async fn list_database_pages(client: NotionClient, database_id: &str) -> Result<Vec<String>> {
    let mut cursor: Option<String> = None;
    let mut pages: Vec<String> = Vec::new();

    loop {
        let request = QueryDatabaseRequest {
            start_cursor: cursor.clone(),
            page_size: Some(100),
            ..Default::default()
        };

        let response = client
            .databases
            .query_a_database(database_id, request)
            .await
            .map_err(map_notion_error)?;

        for page in response.results {
            pages.push(page.id);
        }

        cursor = response.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    Ok(pages)
}

fn map_notion_error(err: NotionClientError) -> Error {
    match err {
        NotionClientError::InvalidStatusCode { error } => match error.status {
            400 => Error::new(ErrorKind::Unexpected, error.message),
            401 | 403 => Error::new(ErrorKind::PermissionDenied, error.message),
            404 => Error::new(ErrorKind::NotFound, error.message),
            _ => Error::new(ErrorKind::Unexpected, error.message),
        },
        NotionClientError::InvalidHeader { source } => Error::new(
            ErrorKind::ConfigInvalid,
            format!("invalid notion header: {source}"),
        ),
        other => {
            error!("notion error: {other:?}");
            Error::new(ErrorKind::Unexpected, other.to_string())
        }
    }
}
