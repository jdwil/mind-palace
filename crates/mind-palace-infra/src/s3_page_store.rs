use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use mind_palace_core::domain::page::Page;
use mind_palace_core::domain::tenant::TenantContext;
use mind_palace_core::domain::value_objects::{
    Confidence, PageId, PageType, Section, Slug, TableOfContents, Visibility,
};
use mind_palace_core::error::MindPalaceError;
use mind_palace_core::ports::page_store::{PageFilter, PageStore};

#[derive(Debug, Clone)]
pub struct S3PageStoreConfig {
    pub bucket_name: String,
    pub prefix: String,
}

pub struct S3PageStore {
    client: Client,
    config: S3PageStoreConfig,
}

impl S3PageStore {
    pub fn new(client: Client, config: S3PageStoreConfig) -> Self {
        Self { client, config }
    }

    fn object_key(&self, slug: &Slug, visibility: &Visibility) -> String {
        let tenant_segment = match visibility {
            Visibility::General => "general".to_string(),
            Visibility::Tenant(tid) => tid.0.clone(),
            Visibility::User(uid) => format!("user-{uid}"),
            Visibility::Archived => "archived".to_string(),
        };
        format!(
            "{}/{}/pages/{}.md",
            self.config.prefix,
            tenant_segment,
            slug.as_str()
        )
    }

    fn list_prefix(&self, ctx: &TenantContext) -> Vec<String> {
        let mut prefixes = vec![format!("{}/general/pages/", self.config.prefix)];
        for tid in &ctx.visible_tenants {
            prefixes.push(format!("{}/{}/pages/", self.config.prefix, tid.0));
        }
        if let Some(uid) = &ctx.user_id {
            prefixes.push(format!("{}/user-{}/pages/", self.config.prefix, uid));
        }
        prefixes
    }
}

#[derive(Serialize, Deserialize)]
struct PageFrontmatter {
    id: uuid::Uuid,
    slug: String,
    title: String,
    summary: String,
    page_type: PageType,
    visibility: Visibility,
    confidence: f32,
    version: u32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    links: Vec<String>,
    toc: TableOfContents,
}

fn serialize_page(page: &Page) -> Result<String, MindPalaceError> {
    let fm = PageFrontmatter {
        id: page.id.0,
        slug: page.slug.as_str().to_string(),
        title: page.title.clone(),
        summary: page.summary.clone(),
        page_type: page.page_type.clone(),
        visibility: page.visibility.clone(),
        confidence: page.confidence.value(),
        version: page.version,
        created_at: page.created_at,
        updated_at: page.updated_at,
        links: page.links.iter().map(|s| s.as_str().to_string()).collect(),
        toc: page.toc.clone(),
    };
    let json = serde_json::to_string_pretty(&fm)
        .map_err(|e| MindPalaceError::Serialization(e.to_string()))?;

    let mut body = String::new();
    for section in &page.sections {
        body.push_str(&format!(
            "## {}\n\n{}\n\n",
            section.heading, section.content
        ));
    }

    Ok(format!("---\n{}\n---\n{}", json, body))
}

fn deserialize_page(raw: &str) -> Result<Page, MindPalaceError> {
    let content = raw
        .strip_prefix("---\n")
        .ok_or_else(|| MindPalaceError::Serialization("missing frontmatter start".into()))?;
    let (json_str, markdown) = content
        .split_once("\n---\n")
        .ok_or_else(|| MindPalaceError::Serialization("missing frontmatter end".into()))?;

    let fm: PageFrontmatter = serde_json::from_str(json_str)
        .map_err(|e| MindPalaceError::Serialization(e.to_string()))?;

    let sections = parse_sections(markdown);
    let slug = Slug::new(&fm.slug).map_err(|e| MindPalaceError::Serialization(e.to_string()))?;
    let links: Result<Vec<Slug>, _> = fm.links.iter().map(|s| Slug::new(s)).collect();
    let links = links.map_err(|e| MindPalaceError::Serialization(e.to_string()))?;

    Ok(Page {
        id: PageId(fm.id),
        slug,
        title: fm.title,
        summary: fm.summary,
        toc: fm.toc,
        sections,
        page_type: fm.page_type,
        visibility: fm.visibility,
        confidence: Confidence::new(fm.confidence).unwrap_or_default(),
        version: fm.version,
        created_at: fm.created_at,
        updated_at: fm.updated_at,
        links,
    })
}

fn parse_sections(markdown: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_content = String::new();
    // Track whether we're inside a fenced code block (``` or ~~~). Lines starting
    // with "## " inside a fence are code content (e.g. shell/python comments,
    // markdown examples), NOT section headings.
    let mut in_fence = false;
    let mut fence_marker: Option<String> = None;

    for line in markdown.lines() {
        let trimmed = line.trim_start();
        // Detect fence open/close. A fence is a run of >=3 backticks or tildes.
        if let Some(marker) = fence_delim(trimmed) {
            if in_fence {
                // Only the same delimiter type closes the fence.
                if fence_marker.as_deref() == Some(marker) {
                    in_fence = false;
                    fence_marker = None;
                }
            } else {
                in_fence = true;
                fence_marker = Some(marker.to_string());
            }
            if current_heading.is_some() {
                current_content.push_str(line);
                current_content.push('\n');
            }
            continue;
        }

        if !in_fence && let Some(heading) = line.strip_prefix("## ") {
            if let Some(h) = current_heading.take() {
                sections.push(Section {
                    heading: h,
                    content: current_content.trim().to_string(),
                });
                current_content.clear();
            }
            current_heading = Some(heading.to_string());
        } else if current_heading.is_some() {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }
    if let Some(h) = current_heading {
        sections.push(Section {
            heading: h,
            content: current_content.trim().to_string(),
        });
    }
    sections
}

/// Returns the fence delimiter kind ("```" or "~~~") if the line opens/closes a
/// fenced code block, else None. Matches a leading run of >=3 identical markers.
fn fence_delim(trimmed_line: &str) -> Option<&'static str> {
    if trimmed_line.starts_with("```") {
        Some("```")
    } else if trimmed_line.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

#[async_trait]
impl PageStore for S3PageStore {
    async fn get_page(&self, id: &PageId, ctx: &TenantContext) -> Result<Page, MindPalaceError> {
        // List all visible prefixes and find the page by id in metadata
        let pages = self.list_pages(&PageFilter::default(), ctx).await?;
        pages
            .into_iter()
            .find(|p| p.id == *id)
            .ok_or_else(|| MindPalaceError::PageNotFound(id.0.to_string()))
    }

    async fn get_page_by_slug(
        &self,
        slug: &Slug,
        ctx: &TenantContext,
    ) -> Result<Page, MindPalaceError> {
        // Try each visible prefix
        let prefixes = self.list_prefix(ctx);
        for prefix in prefixes {
            let key = format!("{}{}.md", prefix, slug.as_str());
            let result = self
                .client
                .get_object()
                .bucket(&self.config.bucket_name)
                .key(&key)
                .send()
                .await;
            match result {
                Ok(output) => {
                    let bytes = output
                        .body
                        .collect()
                        .await
                        .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                    let raw = String::from_utf8(bytes.to_vec())
                        .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                    return deserialize_page(&raw);
                }
                Err(_) => continue,
            }
        }
        Err(MindPalaceError::PageNotFound(slug.as_str().to_string()))
    }

    async fn get_page_by_slug_unfiltered(&self, slug: &Slug) -> Result<Page, MindPalaceError> {
        // Try all known prefixes including archived (bypasses visibility filtering)
        let all_prefixes = vec![
            format!("{}/general/pages/", self.config.prefix),
            format!("{}/archived/pages/", self.config.prefix),
        ];
        for prefix in all_prefixes {
            let key = format!("{}{}.md", prefix, slug.as_str());
            let result = self
                .client
                .get_object()
                .bucket(&self.config.bucket_name)
                .key(&key)
                .send()
                .await;
            match result {
                Ok(output) => {
                    let bytes = output
                        .body
                        .collect()
                        .await
                        .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                    let raw = String::from_utf8(bytes.to_vec())
                        .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                    return deserialize_page(&raw);
                }
                Err(_) => continue,
            }
        }
        // Also try scanning by prefix in case it's under a tenant segment
        let prefix = &self.config.prefix;
        let mut continuation_token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.config.bucket_name)
                .prefix(prefix);
            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| MindPalaceError::Store(e.to_string()))?;
            for obj in resp.contents() {
                let key = obj.key().unwrap_or_default();
                if key.ends_with(&format!("/{}.md", slug.as_str())) {
                    let get_result = self
                        .client
                        .get_object()
                        .bucket(&self.config.bucket_name)
                        .key(key)
                        .send()
                        .await;
                    if let Ok(output) = get_result {
                        let bytes = output
                            .body
                            .collect()
                            .await
                            .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                        let raw = String::from_utf8(bytes.to_vec())
                            .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                        return deserialize_page(&raw);
                    }
                }
            }
            if resp.is_truncated() == Some(true) {
                continuation_token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }
        Err(MindPalaceError::PageNotFound(slug.as_str().to_string()))
    }

    async fn save_page(&self, page: &Page) -> Result<(), MindPalaceError> {
        let key = self.object_key(&page.slug, &page.visibility);
        let body = serialize_page(page)?;
        self.client
            .put_object()
            .bucket(&self.config.bucket_name)
            .key(&key)
            .content_type("text/markdown")
            .metadata("page_id", page.id.0.to_string())
            .body(ByteStream::from(body.into_bytes()))
            .send()
            .await
            .map_err(|e| MindPalaceError::Store(e.to_string()))?;
        Ok(())
    }

    async fn delete_page(&self, id: &PageId) -> Result<(), MindPalaceError> {
        // We need to find the object key first. Scan all objects for the page_id metadata.
        // Simple approach: list with prefix, head each to find matching page_id.
        let prefix = &self.config.prefix;
        let mut continuation_token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.config.bucket_name)
                .prefix(prefix);
            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| MindPalaceError::Store(e.to_string()))?;
            for obj in resp.contents() {
                let key = obj.key().unwrap_or_default();
                let head = self
                    .client
                    .head_object()
                    .bucket(&self.config.bucket_name)
                    .key(key)
                    .send()
                    .await;
                if let Ok(head_resp) = head
                    && head_resp.metadata().and_then(|m| m.get("page_id"))
                        == Some(&id.0.to_string())
                {
                    self.client
                        .delete_object()
                        .bucket(&self.config.bucket_name)
                        .key(key)
                        .send()
                        .await
                        .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                    return Ok(());
                }
            }
            if resp.is_truncated() == Some(true) {
                continuation_token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }
        Err(MindPalaceError::PageNotFound(id.0.to_string()))
    }

    async fn list_pages(
        &self,
        _filter: &PageFilter,
        ctx: &TenantContext,
    ) -> Result<Vec<Page>, MindPalaceError> {
        let prefixes = self.list_prefix(ctx);
        let mut pages = Vec::new();
        for prefix in prefixes {
            let mut continuation_token: Option<String> = None;
            loop {
                let mut req = self
                    .client
                    .list_objects_v2()
                    .bucket(&self.config.bucket_name)
                    .prefix(&prefix);
                if let Some(token) = &continuation_token {
                    req = req.continuation_token(token);
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                for obj in resp.contents() {
                    let key = obj.key().unwrap_or_default();
                    let get_result = self
                        .client
                        .get_object()
                        .bucket(&self.config.bucket_name)
                        .key(key)
                        .send()
                        .await;
                    if let Ok(output) = get_result {
                        let bytes = output
                            .body
                            .collect()
                            .await
                            .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                        let raw = String::from_utf8(bytes.to_vec())
                            .map_err(|e| MindPalaceError::Store(e.to_string()))?;
                        if let Ok(page) = deserialize_page(&raw) {
                            pages.push(page);
                        }
                    }
                }
                if resp.is_truncated() == Some(true) {
                    continuation_token = resp.next_continuation_token().map(|s| s.to_string());
                } else {
                    break;
                }
            }
        }
        Ok(pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mind_palace_core::domain::value_objects::{Confidence, PageId, PageType, Visibility};

    fn page_with_sections(sections: Vec<Section>) -> Page {
        let toc = TableOfContents::from_sections(&sections);
        Page {
            id: PageId::new(),
            slug: Slug::new("test-page").unwrap(),
            title: "Test Page".into(),
            summary: "A summary".into(),
            toc,
            sections,
            page_type: PageType::Leaf,
            visibility: Visibility::General,
            confidence: Confidence::default(),
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            links: vec![],
        }
    }

    fn roundtrip(sections: Vec<Section>) -> Vec<Section> {
        let page = page_with_sections(sections);
        let raw = serialize_page(&page).unwrap();
        deserialize_page(&raw).unwrap().sections
    }

    #[test]
    fn plain_sections_roundtrip() {
        let secs = vec![
            Section {
                heading: "Overview".into(),
                content: "Some text.".into(),
            },
            Section {
                heading: "Details".into(),
                content: "More text.".into(),
            },
        ];
        let out = roundtrip(secs.clone());
        assert_eq!(out, secs);
    }

    #[test]
    fn python_code_with_hash_headings_roundtrips_as_one_section() {
        // A python block containing lines that start with "## " must NOT be
        // split into extra sections.
        let content = "Here is the script:\n\n```python\n## configuration\nx = 1\n## main\nprint(x)\n```\n\nThat's it.";
        let secs = vec![Section {
            heading: "Script".into(),
            content: content.into(),
        }];
        let out = roundtrip(secs);
        assert_eq!(
            out.len(),
            1,
            "code block '## ' lines must not create new sections"
        );
        assert_eq!(out[0].heading, "Script");
        assert!(out[0].content.contains("## configuration"));
        assert!(out[0].content.contains("## main"));
        assert!(out[0].content.contains("print(x)"));
    }

    #[test]
    fn sql_code_block_roundtrips() {
        let content = "```sql\nSELECT * FROM users\nWHERE active = true;\n```";
        let secs = vec![
            Section {
                heading: "Query".into(),
                content: content.into(),
            },
            Section {
                heading: "Notes".into(),
                content: "Filters active users.".into(),
            },
        ];
        let out = roundtrip(secs);
        assert_eq!(out.len(), 2);
        assert!(out[0].content.contains("SELECT * FROM users"));
        assert_eq!(out[1].heading, "Notes");
    }

    #[test]
    fn tilde_fence_roundtrips() {
        let content = "~~~bash\n## step one\necho hi\n~~~";
        let secs = vec![Section {
            heading: "Shell".into(),
            content: content.into(),
        }];
        let out = roundtrip(secs);
        assert_eq!(out.len(), 1);
        assert!(out[0].content.contains("## step one"));
    }

    #[test]
    fn multiple_sections_with_and_without_code() {
        let secs = vec![
            Section {
                heading: "Intro".into(),
                content: "Prose only.".into(),
            },
            Section {
                heading: "Code".into(),
                content: "```py\n## a\nb=2\n```".into(),
            },
            Section {
                heading: "Outro".into(),
                content: "More prose.".into(),
            },
        ];
        let out = roundtrip(secs.clone());
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].heading, "Intro");
        assert_eq!(out[1].heading, "Code");
        assert_eq!(out[2].heading, "Outro");
    }
}
