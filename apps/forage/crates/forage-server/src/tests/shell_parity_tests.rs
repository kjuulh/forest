use std::path::Path;

use forage_ui::chrome::{Chrome, PublicNav, Tab, Viewer};
use forage_ui::layouts::Shell;
use minijinja::context;
use regex::Regex;
use sedge_rt::Render;

use crate::templates::TemplateEngine;

fn jinja(ctx: minijinja::Value) -> String {
    let engine =
        TemplateEngine::from_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates"))
            .expect("template engine");
    engine
        .render_str(
            r#"{% extends "base.html.jinja" %}{% block content %}<p>body</p>{% endblock %}"#,
            ctx,
        )
        .expect("render base")
}

fn sedge(chrome: &Chrome) -> String {
    let body = |out: &mut String| out.push_str("<p>body</p>");
    Shell {
        chrome,
        children: &body,
        head: None,
        scripts: None,
    }
    .render_to_string()
}

fn decode(s: &str) -> String {
    s.replace("&#x2f;", "/")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&middot;", "·")
        .replace("&copy;", "©")
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn affordances(html: &str) -> Vec<String> {
    let html = decode(html);
    let tag = Regex::new(r"(?s)<[^>]*>").unwrap();
    let link = Regex::new(r#"(?s)<a ([^>]*?)href="([^"]*)"([^>]*)>(.*?)</a>"#).unwrap();
    let class = Regex::new(r#"class="([^"]*)""#).unwrap();
    let script = Regex::new(r#"<script [^>]*src="([^"?]*)"#).unwrap();
    let marker = Regex::new(r#"\b(id|data-toggle|data-fuzzy-switcher|data-fuzzy-list)(="[^"]*")?"#).unwrap();
    let form = Regex::new(r#"<form method="([^"]*)" action="([^"]*)">"#).unwrap();
    let input = Regex::new(r#"<input type="hidden" name="([^"]*)" value="([^"]*)">"#).unwrap();
    let mut out = Vec::new();
    for c in link.captures_iter(&html) {
        let attrs = format!("{} {}", &c[1], &c[3]);
        let mut classes: Vec<&str> = class
            .captures(&attrs)
            .map(|m| m.get(1).unwrap().as_str().split_whitespace().collect())
            .unwrap_or_default();
        classes.sort_unstable();
        out.push(format!(
            "link {} [{}] {}",
            &c[2],
            squash(&tag.replace_all(&c[4], "")),
            classes.join(" ")
        ));
    }
    for c in script.captures_iter(&html) {
        out.push(format!("script {}", &c[1]));
    }
    for c in marker.captures_iter(&html) {
        out.push(format!("marker {}", &c[0]));
    }
    for c in form.captures_iter(&html) {
        out.push(format!("form {} {}", c[1].to_uppercase(), &c[2]));
    }
    for c in input.captures_iter(&html) {
        out.push(format!("field {}={}", &c[1], &c[2]));
    }
    let main_start = html.find("<main").unwrap();
    out.push(format!(
        "text {}",
        squash(&tag.replace_all(&html[..main_start], " "))
    ));
    out.push(format!(
        "footer {}",
        squash(&tag.replace_all(&html[html.find("<footer").unwrap()..], " "))
    ));
    out
}

fn signed_in(
    org: Option<&str>,
    project: Option<&str>,
    orgs: &[&str],
    projects: &[&str],
    tab: Tab,
) -> Chrome {
    Chrome {
        title: "Triggers - api - acme - Forest".into(),
        description: "Release triggers".into(),
        viewer: Some(Viewer {
            username: "testuser".into(),
            csrf_token: "tok".into(),
            orgs: orgs.iter().map(|s| s.to_string()).collect(),
            current_org: org.map(Into::into),
            projects: projects.iter().map(|s| s.to_string()).collect(),
            project: project.map(Into::into),
            tab,
        }),
        ..Default::default()
    }
}

fn jinja_ctx(
    org: Option<&str>,
    project: Option<&str>,
    orgs: &[&str],
    projects: &[&str],
    tab: &str,
) -> minijinja::Value {
    let orgs: Vec<_> = orgs
        .iter()
        .map(|o| context! { name => o, role => "owner" })
        .collect();
    context! {
        title => "Triggers - api - acme - Forest",
        description => "Release triggers",
        user => context! { username => "testuser" },
        csrf_token => "tok",
        orgs => orgs,
        current_org => org,
        projects => projects,
        project_name => project,
        active_tab => tab,
    }
}

#[test]
fn the_sedge_shell_offers_what_base_html_jinja_offers() {
    let cases: Vec<(Chrome, minijinja::Value)> = vec![
        (
            signed_in(
                Some("acme"),
                Some("api"),
                &["acme", "other"],
                &["api", "web"],
                Tab::ProjectSettings,
            ),
            jinja_ctx(
                Some("acme"),
                Some("api"),
                &["acme", "other"],
                &["api", "web"],
                "project_settings",
            ),
        ),
        (
            signed_in(Some("acme"), None, &["acme"], &[], Tab::Destinations),
            jinja_ctx(Some("acme"), None, &["acme"], &[], "destinations"),
        ),
        (
            signed_in(
                Some("acme"),
                Some("api"),
                &["acme"],
                &[],
                Tab::ProjectReleases,
            ),
            jinja_ctx(
                Some("acme"),
                Some("api"),
                &["acme"],
                &[],
                "project_releases",
            ),
        ),
        (
            signed_in(None, None, &[], &[], Tab::Dashboard),
            jinja_ctx(None, None, &[], &[], "dashboard"),
        ),
        (
            Chrome {
                title: "Forest".into(),
                description: "d".into(),
                ..Default::default()
            },
            context! { title => "Forest", description => "d" },
        ),
        (
            Chrome {
                title: "Sign in".into(),
                description: "d".into(),
                public: PublicNav {
                    is_auth_page: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            context! { title => "Sign in", description => "d", is_auth_page => true },
        ),
    ];
    for (chrome, ctx) in cases {
        let (a, b) = (jinja(ctx), sedge(&chrome));
        assert!(
            affordances(&a)
                .iter()
                .filter(|x| x.starts_with("link"))
                .count()
                >= 2
        );
        assert_eq!(
            affordances(&a),
            affordances(&b),
            "\n--- jinja\n{a}\n--- sedge\n{b}"
        );
    }
}

#[test]
fn the_current_tab_is_marked_for_assistive_technology() {
    let html = sedge(&signed_in(
        Some("acme"),
        Some("api"),
        &["acme"],
        &[],
        Tab::ProjectReleases,
    ));
    assert!(
        html.contains(r#"<a href="/orgs/acme/projects/api/releases" aria-current="page""#),
        "{html}"
    );
}

#[test]
fn neither_shell_has_inline_script_or_handlers() {
    let jinja = jinja(jinja_ctx(
        Some("acme"),
        Some("api"),
        &["acme", "b"],
        &["api"],
        "project_settings",
    ));
    let sedge = sedge(&signed_in(
        Some("acme"),
        Some("api"),
        &["acme", "b"],
        &["api"],
        Tab::ProjectSettings,
    ));
    let handler = Regex::new(r#"\son[a-z]+="#).unwrap();
    for html in [jinja, sedge] {
        assert!(!html.contains("<script>"), "{html}");
        assert!(!html.contains("<style>"), "{html}");
        assert!(!handler.is_match(&html), "{html}");
    }
}
