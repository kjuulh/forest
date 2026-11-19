use forage_ui::about::{About, AboutLink, AboutLinkKind};
use forage_ui::components::{AboutBlock, JoinOfferBanner};
use sedge_rt::Render;

fn link(kind: AboutLinkKind, href: &str, label: &str) -> AboutLink {
    AboutLink {
        kind,
        href: href.into(),
        label: label.into(),
    }
}

#[test]
fn about_block_lists_links_then_facts() {
    let about = About {
        links: vec![
            link(AboutLinkKind::Git, "https://github.com/o/r", "o/r"),
            link(
                AboutLinkKind::Docs,
                "https://docs.example.com",
                "docs.example.com",
            ),
        ],
        domain: "payments".into(),
        owner: "team <a>".into(),
    };
    let html = AboutBlock { about: &about }.render_to_string();
    assert!(html.contains("team &lt;a&gt;"));
    assert!(html.contains("border-t border-gray-100"));
    insta::assert_snapshot!(html);
}

#[test]
fn about_block_renders_nothing_without_links_or_facts() {
    let html = AboutBlock {
        about: &About::default(),
    }
    .render_to_string();
    assert_eq!(html.trim(), "");
}

#[test]
fn about_block_with_only_facts_has_no_divider() {
    let about = About {
        owner: "team-a".into(),
        ..Default::default()
    };
    let html = AboutBlock { about: &about }.render_to_string();
    assert!(html.contains("Owner"));
    assert!(!html.contains("Domain"));
    assert!(!html.contains("border-t"));
    assert!(!html.contains("<ul"));
}

#[test]
fn join_offer_banner_posts_to_the_offer_with_the_csrf_token() {
    let html = JoinOfferBanner {
        organisation_id: "org/1",
        organisation_name: "Acme & Co",
        matched_domain: "acme.test",
        csrf_token: "tok",
    }
    .render_to_string();
    assert!(
        html.contains(r#"action="/join-offers/org%2F1/accept""#),
        "{html}"
    );
    assert!(html.contains(r#"<input type="hidden" name="_csrf" value="tok">"#));
    assert!(html.contains("Acme &amp; Co"));
    insta::assert_snapshot!(html);
}

#[test]
fn an_about_link_with_a_script_scheme_is_neutralised() {
    let about = forage_ui::about::About {
        links: vec![forage_ui::about::AboutLink {
            kind: forage_ui::about::AboutLinkKind::Homepage,
            href: "javascript:alert(1)".into(),
            label: "evil".into(),
        }],
        ..Default::default()
    };
    let html = sedge_rt::Render::render_to_string(&forage_ui::components::AboutBlock { about: &about });
    assert!(!html.contains(r#"href="javascript:"#), "{html}");
    assert!(html.contains("about:invalid"), "{html}");
}
