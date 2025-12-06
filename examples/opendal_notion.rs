use futures::TryStreamExt;
use std::env;

use notion_opendal::notion_opendal::NotionServiceBuilder;
use opendal::Operator;

#[tokio::main]
async fn main() -> opendal::Result<()> {
    let token =
        env::var("NOTION_API_TOKEN").expect("set NOTION_TOKEN to your Notion integration token");
    let database_id = env::var("NOTION_DATABASE_ID").ok();
    let page_id = env::var("NOTION_PAGE_ID").ok();

    let mut builder = NotionServiceBuilder::default()
        .token(&token)
        .frontmatter(true);
    if let Some(db_id) = &database_id {
        builder = builder.database_id(db_id);
    }

    let op = Operator::new(builder)?.finish();

    if let Some(db_id) = database_id {
        println!("Listing pages in database: {db_id}");
        let mut lister = op.lister("/").await?;
        while let Some(entry) = lister.try_next().await? {
            println!(" - {}", entry.path());
        }
    } else {
        println!("NOTION_DATABASE_ID not set; skipping list");
    }

    if let Some(page_id) = page_id {
        let path = format!("{page_id}.md");
        let content = op.read(&path).await?;
        let bytes = content.to_vec();
        let text = String::from_utf8_lossy(&bytes);
        println!("\n--- Page {path} ---\n{text}");
    } else {
        println!("NOTION_PAGE_ID not set; skipping read");
    }

    Ok(())
}
