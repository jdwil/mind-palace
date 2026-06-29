#[cfg(test)]
mod unit_tests {
    use crate::domain::page::{Page, PageValidationError, ReadLevel};
    use crate::domain::tenant::TenantContext;
    use crate::domain::value_objects::*;

    #[test]
    fn slug_valid() {
        assert!(Slug::new("hello-world").is_ok());
        assert!(Slug::new("page123").is_ok());
        assert!(Slug::new("a").is_ok());
    }

    #[test]
    fn slug_rejects_empty() {
        assert_eq!(Slug::new("").unwrap_err(), SlugError::Empty);
    }

    #[test]
    fn slug_rejects_uppercase() {
        assert_eq!(Slug::new("Hello").unwrap_err(), SlugError::InvalidChars);
    }

    #[test]
    fn slug_rejects_leading_hyphen() {
        assert_eq!(Slug::new("-start").unwrap_err(), SlugError::InvalidFormat);
    }

    #[test]
    fn slug_rejects_trailing_hyphen() {
        assert_eq!(Slug::new("end-").unwrap_err(), SlugError::InvalidFormat);
    }

    #[test]
    fn page_creation_valid() {
        let page = Page::new(
            "Test Page".into(),
            Slug::new("test-page").unwrap(),
            "A summary".into(),
            vec![Section {
                heading: "Introduction".into(),
                content: "Some content".into(),
            }],
            PageType::Concept,
            Visibility::General,
        );
        assert!(page.is_ok());
        let page = page.unwrap();
        assert_eq!(page.title, "Test Page");
        assert_eq!(page.version, 1);
        assert_eq!(page.toc.entries.len(), 1);
        assert_eq!(page.toc.entries[0].anchor, "introduction");
    }

    #[test]
    fn page_rejects_empty_title() {
        let result = Page::new(
            "".into(),
            Slug::new("test").unwrap(),
            "summary".into(),
            vec![Section {
                heading: "H".into(),
                content: "C".into(),
            }],
            PageType::Leaf,
            Visibility::General,
        );
        assert!(matches!(result, Err(PageValidationError::MissingTitle)));
    }

    #[test]
    fn page_rejects_empty_summary() {
        let result = Page::new(
            "Title".into(),
            Slug::new("test").unwrap(),
            "".into(),
            vec![Section {
                heading: "H".into(),
                content: "C".into(),
            }],
            PageType::Leaf,
            Visibility::General,
        );
        assert!(matches!(result, Err(PageValidationError::MissingSummary)));
    }

    #[test]
    fn page_rejects_no_sections() {
        let result = Page::new(
            "Title".into(),
            Slug::new("test").unwrap(),
            "Summary".into(),
            vec![],
            PageType::Leaf,
            Visibility::General,
        );
        assert!(matches!(result, Err(PageValidationError::NoSections)));
    }

    #[test]
    fn page_section_lookup() {
        let page = Page::new(
            "Title".into(),
            Slug::new("test").unwrap(),
            "Summary".into(),
            vec![
                Section {
                    heading: "Intro".into(),
                    content: "Hello".into(),
                },
                Section {
                    heading: "Details".into(),
                    content: "World".into(),
                },
            ],
            PageType::Concept,
            Visibility::General,
        )
        .unwrap();

        assert_eq!(page.section_by_heading("Intro").unwrap().content, "Hello");
        assert_eq!(page.section_by_heading("Details").unwrap().content, "World");
        assert!(page.section_by_heading("Missing").is_none());
    }

    #[test]
    fn read_level_enum() {
        let _summary = ReadLevel::Summary;
        let _section = ReadLevel::Section("Intro".into());
        let _full = ReadLevel::Full;
    }

    // --- TenantContext visibility tests ---

    #[test]
    fn global_context_sees_everything() {
        let ctx = TenantContext::global();
        assert!(ctx.can_see(&Visibility::General));
        assert!(ctx.can_see(&Visibility::Tenant(TenantId::new("any"))));
    }

    #[test]
    fn leaf_tenant_sees_general() {
        let ctx = TenantContext::leaf(TenantId::new("client-a"));
        assert!(ctx.can_see(&Visibility::General));
    }

    #[test]
    fn leaf_tenant_sees_own_pages() {
        let ctx = TenantContext::leaf(TenantId::new("client-a"));
        assert!(ctx.can_see(&Visibility::Tenant(TenantId::new("client-a"))));
    }

    #[test]
    fn leaf_tenant_cannot_see_sibling() {
        let ctx = TenantContext::leaf(TenantId::new("client-a"));
        assert!(!ctx.can_see(&Visibility::Tenant(TenantId::new("client-b"))));
    }

    #[test]
    fn parent_tenant_sees_child_pages() {
        let ctx = TenantContext::parent(
            TenantId::new("dashlx"),
            vec![TenantId::new("client-a"), TenantId::new("client-b")],
        );
        assert!(ctx.can_see(&Visibility::Tenant(TenantId::new("client-a"))));
        assert!(ctx.can_see(&Visibility::Tenant(TenantId::new("client-b"))));
        assert!(ctx.can_see(&Visibility::Tenant(TenantId::new("dashlx"))));
        assert!(ctx.can_see(&Visibility::General));
    }

    #[test]
    fn parent_tenant_cannot_see_unrelated() {
        let ctx = TenantContext::parent(TenantId::new("dashlx"), vec![TenantId::new("client-a")]);
        assert!(!ctx.can_see(&Visibility::Tenant(TenantId::new("other-org"))));
    }

    #[test]
    fn confidence_validation() {
        assert!(Confidence::new(0.0).is_some());
        assert!(Confidence::new(1.0).is_some());
        assert!(Confidence::new(0.5).is_some());
        assert!(Confidence::new(-0.1).is_none());
        assert!(Confidence::new(1.1).is_none());
    }

    #[test]
    fn toc_generated_from_sections() {
        let sections = vec![
            Section {
                heading: "First Section".into(),
                content: "...".into(),
            },
            Section {
                heading: "Second Part".into(),
                content: "...".into(),
            },
        ];
        let toc = TableOfContents::from_sections(&sections);
        assert_eq!(toc.entries.len(), 2);
        assert_eq!(toc.entries[0].anchor, "first-section");
        assert_eq!(toc.entries[1].anchor, "second-part");
    }
}
