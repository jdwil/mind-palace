//! Basic usage of the Mind Palace SDK.
//!
//! Requires AWS credentials and deployed infrastructure (see infra/).

use mind_palace::core::domain::page::ReadLevel;
use mind_palace::core::domain::service::CreatePageInput;
use mind_palace::core::domain::tenant::TenantContext;
use mind_palace::core::domain::value_objects::{PageType, Section, Slug, Visibility};
use mind_palace::{BedrockConfig, DynamoConfig, MindPalace, S3Config, S3VectorsConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Mind Palace - Basic Usage Example");
    println!("==================================");
    println!("This example requires AWS credentials and deployed infrastructure.");
    println!("Run `cd infra && sam build && sam deploy --guided` first.");
    println!();

    let palace = MindPalace::builder()
        .s3(S3Config {
            bucket_name: "my-pages".into(),
            region: "us-east-1".into(),
            prefix: "v1".into(),
        })
        .dynamo(DynamoConfig {
            table_name: "my-graph".into(),
            region: "us-east-1".into(),
        })
        .s3vectors(S3VectorsConfig {
            bucket_name: "my-vectors".into(),
            index_name: "wiki".into(),
            region: "us-east-1".into(),
        })
        .bedrock(BedrockConfig {
            model_id: "amazon.titan-embed-text-v2:0".into(),
            region: "us-east-1".into(),
        })
        .build()
        .await?;

    let ctx = TenantContext::global();

    // Create a page
    let (page, issues) = palace
        .wiki_service()
        .create_page(
            CreatePageInput {
                title: "Rust Ownership".into(),
                slug: Slug::new("rust-ownership")?,
                summary: "How Rust manages memory without a garbage collector.".into(),
                sections: vec![Section {
                    heading: "Rules".into(),
                    content: "Each value has one owner.".into(),
                }],
                page_type: PageType::Concept,
                visibility: Visibility::General,
                links: vec![],
            },
            &ctx,
        )
        .await?;
    println!("Created: {} (lint issues: {})", page.title, issues.len());

    // Search for it
    let results = palace
        .wiki_service()
        .search("ownership memory", &ctx, 5)
        .await?;
    println!("Search results: {}", results.len());

    // Read at summary level
    let response = palace
        .wiki_service()
        .read_page(&page.slug, ReadLevel::Summary, &ctx)
        .await?;
    println!("Read: {:?}", response);

    Ok(())
}
