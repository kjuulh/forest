use forage_ui::chrome::{BuildStamp, Chrome, Tab, Viewer};
use forage_ui::layouts::{ProjectSettings, Shell};
use forage_ui::project_settings::Section;
use sedge_rt::Render;

fn chrome() -> Chrome {
    Chrome {
        title: "Triggers - api - acme - Forest".into(),
        description: "Release triggers for api in acme".into(),
        build: BuildStamp {
            commit: "abc1234".into(),
            time: String::new(),
        },
        viewer: Some(Viewer {
            username: "kasper".into(),
            csrf_token: "tok".into(),
            orgs: vec!["acme".into(), "other".into()],
            current_org: Some("acme".into()),
            projects: vec!["api".into(), "web".into()],
            project: Some("api".into()),
            tab: Tab::ProjectSettings,
        }),
        ..Default::default()
    }
}

#[test]
fn shell_for_a_signed_in_project_page() {
    let body = |out: &mut String| out.push_str("<p>content</p>");
    let html = Shell {
        chrome: &chrome(),
        scripts: None,
        children: &body,
    }
    .render_to_string();
    insta::assert_snapshot!(html);
}

#[test]
fn shell_for_a_visitor() {
    let body = |out: &mut String| out.push_str("<p>content</p>");
    let chrome = Chrome {
        title: "Forest".into(),
        ..Default::default()
    };
    let html = Shell {
        chrome: &chrome,
        scripts: None,
        children: &body,
    }
    .render_to_string();
    assert!(html.contains(r#"href="/login""#));
    assert!(html.contains(r#"href="/pricing""#));
    assert!(
        !html.contains(r#"id="mobile-menu""#),
        "a visitor has no account menu"
    );
}

#[test]
fn project_settings_section_and_sub_page() {
    let body = |out: &mut String| out.push_str("<p>content</p>");
    let section = ProjectSettings {
        org: "acme",
        project: "api",
        section: Section::Triggers,
        description: "Automatically release artifacts matching these rules",
        crumb: None,
        children: &body,
    }
    .render_to_string();
    let sub = ProjectSettings {
        org: "acme",
        project: "api",
        section: Section::Triggers,
        description: "Update filter patterns and targets for this trigger.",
        crumb: Some("main-to-dev"),
        children: &body,
    }
    .render_to_string();
    insta::assert_snapshot!(format!("{section}\n=====\n{sub}"));
}
