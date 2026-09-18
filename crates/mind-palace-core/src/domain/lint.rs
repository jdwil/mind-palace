use serde::{Deserialize, Serialize};

use super::graph::KnowledgeGraph;
use super::page::Page;
use super::value_objects::{BaseVisibility, Slug};
use crate::ports::secrets::SecretsResolver;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LintCode {
    MissingSummary,
    MissingToc,
    EmptySection,
    BrokenLink,
    Orphan,
    TitleSlugMismatch,
    MissingSopSection,
    MissingSkillSection,
    UnfencedHeadingInContent,
    /// A Public page carries secret references (Spec 4 §6). A public page must
    /// never gate a secret — anyone, including anonymous, could resolve it.
    PublicPageWithSecrets,
    /// A secret reference fails the configured backend's `validate_reference`
    /// (Spec 4 §6) — it is malformed or looks like a raw secret value.
    InvalidSecretReference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintIssue {
    pub code: LintCode,
    pub severity: Severity,
    pub message: String,
}

/// Runs lint rules against a page. The graph is optional (needed for link/orphan
/// checks). The secrets resolver is optional (needed to validate secret
/// references — Spec 4 §6); when `None`, reference validation is skipped but the
/// Public-page-with-secrets rule still applies.
pub fn lint_page(
    page: &Page,
    graph: Option<&KnowledgeGraph>,
    secrets: Option<&dyn SecretsResolver>,
) -> Vec<LintIssue> {
    let mut issues = Vec::new();

    if page.summary.is_empty() {
        issues.push(LintIssue {
            code: LintCode::MissingSummary,
            severity: Severity::Error,
            message: "Page is missing a summary".into(),
        });
    }

    if page.toc.entries.is_empty() {
        issues.push(LintIssue {
            code: LintCode::MissingToc,
            severity: Severity::Error,
            message: "Page has no table of contents entries".into(),
        });
    }

    for section in &page.sections {
        if section.content.trim().is_empty() {
            issues.push(LintIssue {
                code: LintCode::EmptySection,
                severity: Severity::Warning,
                message: format!("Section '{}' has empty content", section.heading),
            });
        }

        // A line starting with "## " OUTSIDE a fenced code block will be
        // mis-parsed as a new section heading when the page is reloaded from
        // storage. Warn so the author fences it (code) or demotes the heading.
        if let Some(line) = unfenced_h2(&section.content) {
            issues.push(LintIssue {
                code: LintCode::UnfencedHeadingInContent,
                severity: Severity::Warning,
                message: format!(
                    "Section '{}' contains an unfenced '## ' line ({:?}) that will be \
                     read back as a separate section. Wrap code in a fenced block (``` or ~~~), \
                     or change the heading level.",
                    section.heading,
                    line.chars().take(40).collect::<String>()
                ),
            });
        }
    }

    // Check title/slug consistency
    let expected_slug = page.title.to_lowercase().replace(' ', "-");
    if page.slug.as_str() != expected_slug {
        issues.push(LintIssue {
            code: LintCode::TitleSlugMismatch,
            severity: Severity::Info,
            message: format!(
                "Slug '{}' doesn't match title (expected '{}')",
                page.slug.as_str(),
                expected_slug
            ),
        });
    }

    // SOP pages must have required sections
    if page.page_type == super::value_objects::PageType::Sop {
        let required = ["Prerequisites", "Steps", "Constraints", "Verification"];
        for &heading in &required {
            if !page.sections.iter().any(|s| s.heading == heading) {
                issues.push(LintIssue {
                    code: LintCode::MissingSopSection,
                    severity: Severity::Warning,
                    message: format!("SOP page missing required section: '{heading}'"),
                });
            }
        }
    }

    // Skill pages must have required sections
    if page.page_type == super::value_objects::PageType::Skill {
        let required = ["When to Use", "Prompt Pattern", "Example", "Limitations"];
        for &heading in &required {
            if !page.sections.iter().any(|s| s.heading == heading) {
                issues.push(LintIssue {
                    code: LintCode::MissingSkillSection,
                    severity: Severity::Warning,
                    message: format!("Skill page missing required section: '{heading}'"),
                });
            }
        }
    }

    // Spec 4 §6 secret-safety rules.
    if !page.secret_refs.is_empty() {
        // A Public page must never gate a secret (anyone incl. anonymous could
        // resolve it). This is an Error regardless of backend configuration.
        if page.access.base_visibility == BaseVisibility::Public {
            issues.push(LintIssue {
                code: LintCode::PublicPageWithSecrets,
                severity: Severity::Error,
                message: format!(
                    "Public page carries {} secret reference(s); a public page must not gate a \
                     secret. Make the page Private (owner + explicit grants) or remove the refs.",
                    page.secret_refs.len()
                ),
            });
        }

        // Each reference must be a well-formed locator for the configured
        // backend. A value that looks like a raw secret is rejected here (and
        // again at the service layer on save).
        if let Some(resolver) = secrets {
            for sref in &page.secret_refs {
                if let Err(e) = resolver.validate_reference(&sref.reference) {
                    issues.push(LintIssue {
                        code: LintCode::InvalidSecretReference,
                        severity: Severity::Error,
                        message: format!(
                            "Secret ref '{}' is not a valid backend reference: {e}. Store the \
                             secret in the backend and use its reference (e.g. an ARN), never a \
                             raw value.",
                            sref.name
                        ),
                    });
                }
            }
        }
    }

    if let Some(kg) = graph {
        // Check for broken internal links
        for link_slug in &page.links {
            let has_target = kg
                .get_index_pages(&super::tenant::TenantContext::global())
                .iter()
                .any(|n| &n.slug == link_slug)
                || find_by_slug(kg, link_slug);

            if !has_target {
                issues.push(LintIssue {
                    code: LintCode::BrokenLink,
                    severity: Severity::Warning,
                    message: format!("Link to '{}' has no matching page", link_slug.as_str()),
                });
            }
        }

        // Check for orphan (no incoming edges)
        if !kg.has_node(&page.id) {
            issues.push(LintIssue {
                code: LintCode::Orphan,
                severity: Severity::Warning,
                message: "Page is not connected to the knowledge graph".into(),
            });
        }
    }

    issues
}

/// Returns the first line in `content` that starts with "## " while OUTSIDE a
/// fenced code block, or None. Mirrors the fence tracking in the storage-layer
/// section parser so lint agrees with what will actually happen on reload.
fn unfenced_h2(content: &str) -> Option<&str> {
    let mut in_fence = false;
    let mut fence: Option<&str> = None;
    for line in content.lines() {
        let t = line.trim_start();
        let delim = if t.starts_with("```") {
            Some("```")
        } else if t.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(d) = delim {
            if in_fence {
                if fence == Some(d) {
                    in_fence = false;
                    fence = None;
                }
            } else {
                in_fence = true;
                fence = Some(d);
            }
            continue;
        }
        if !in_fence && line.starts_with("## ") {
            return Some(line);
        }
    }
    None
}

fn find_by_slug(kg: &KnowledgeGraph, slug: &Slug) -> bool {
    // Linear scan — acceptable for lint operations on small graphs
    kg.get_index_pages(&super::tenant::TenantContext::global())
        .iter()
        .any(|n| &n.slug == slug)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::page::Page;
    use crate::domain::value_objects::*;

    fn valid_page() -> Page {
        Page::new(
            "Test Page".into(),
            Slug::new("test-page").unwrap(),
            "A valid summary".into(),
            vec![Section {
                heading: "Details".into(),
                content: "Some content here".into(),
            }],
            PageType::Concept,
            Visibility::General,
        )
        .unwrap()
    }

    #[test]
    fn valid_page_passes_lint() {
        let page = valid_page();
        let issues = lint_page(&page, None, None);
        // Only the title/slug mismatch info (title is "Test Page" -> "test-page" matches)
        assert!(issues.iter().all(|i| i.severity != Severity::Error));
    }

    #[test]
    fn empty_section_warning() {
        let mut page = valid_page();
        page.sections.push(Section {
            heading: "Empty".into(),
            content: "".into(),
        });
        let issues = lint_page(&page, None, None);
        assert!(issues.iter().any(|i| i.code == LintCode::EmptySection));
    }

    #[test]
    fn broken_link_detected() {
        let mut page = valid_page();
        page.links.push(Slug::new("nonexistent").unwrap());

        let kg = KnowledgeGraph::new();
        let issues = lint_page(&page, Some(&kg), None);
        assert!(issues.iter().any(|i| i.code == LintCode::BrokenLink));
    }

    #[test]
    fn orphan_detected() {
        let page = valid_page();
        let kg = KnowledgeGraph::new(); // page not in graph
        let issues = lint_page(&page, Some(&kg), None);
        assert!(issues.iter().any(|i| i.code == LintCode::Orphan));
    }

    #[test]
    fn unfenced_h2_line_in_content_warns() {
        let mut page = valid_page();
        page.sections.push(Section {
            heading: "Script".into(),
            content: "## not fenced\nsome code".into(),
        });
        let issues = lint_page(&page, None, None);
        assert!(
            issues
                .iter()
                .any(|i| i.code == LintCode::UnfencedHeadingInContent)
        );
    }

    #[test]
    fn fenced_h2_line_in_content_ok() {
        let mut page = valid_page();
        page.sections.push(Section {
            heading: "Script".into(),
            content: "```python\n## configuration\nx = 1\n```".into(),
        });
        let issues = lint_page(&page, None, None);
        assert!(
            !issues
                .iter()
                .any(|i| i.code == LintCode::UnfencedHeadingInContent),
            "fenced '## ' lines must not trigger the warning"
        );
    }

    #[test]
    fn public_page_with_secret_ref_is_error() {
        use crate::domain::value_objects::SecretRef;
        let mut page = valid_page(); // General → Public base visibility
        page.secret_refs = vec![SecretRef::new("k", "ref://x")];
        let issues = lint_page(&page, None, None);
        assert!(
            issues
                .iter()
                .any(|i| i.code == LintCode::PublicPageWithSecrets
                    && i.severity == Severity::Error),
            "public page with secret refs must be an Error even without a resolver"
        );
    }

    #[test]
    fn private_page_with_secret_ref_no_public_error() {
        use crate::domain::value_objects::{BaseVisibility, PageAccess, SecretRef};
        let mut page = Page::new_with_access(
            "Priv".into(),
            Slug::new("priv").unwrap(),
            "summary".into(),
            vec![Section {
                heading: "H".into(),
                content: "C".into(),
            }],
            PageType::Leaf,
            PageAccess {
                owner: Some("o@x.com".into()),
                base_visibility: BaseVisibility::Private,
                grants: vec![],
            },
        )
        .unwrap();
        page.secret_refs = vec![SecretRef::new("k", "ref://x")];
        let issues = lint_page(&page, None, None);
        assert!(
            !issues
                .iter()
                .any(|i| i.code == LintCode::PublicPageWithSecrets),
            "a Private page with secret refs must NOT trigger the public rule"
        );
    }
}
