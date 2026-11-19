use forage_ui::components::*;
use forage_ui::org_settings::OrgSection;
use sedge_rt::Render;

fn html(r: impl Render) -> String {
    r.render_to_string()
}

#[test]
fn the_visibility_form_posts_to_the_org_or_the_project() {
    let org = html(VisibilityForm {
        org: "acme",
        project: None,
        csrf_token: "tok",
        public: true,
    });
    assert!(
        org.contains(r#"action="/orgs/acme/settings/visibility""#),
        "{org}"
    );
    assert!(
        org.contains(r#"<input type="hidden" name="_csrf" value="tok">"#),
        "{org}"
    );
    assert!(org.contains(r#"value="public" checked"#), "{org}");
    assert!(org.contains(r#"value="private" class"#), "{org}");
    assert!(org.contains("Visibility of acme</legend>"), "{org}");

    let project = html(VisibilityForm {
        org: "acme",
        project: Some("my api"),
        csrf_token: "tok",
        public: false,
    });
    assert!(
        project.contains(r#"action="/orgs/acme/projects/my%20api/settings/visibility""#),
        "{project}"
    );
    assert!(project.contains(r#"value="private" checked"#), "{project}");
    assert!(
        project.contains("Visibility of acme/my api</legend>"),
        "{project}"
    );
    insta::assert_snapshot!(project);
}

#[test]
fn visibility_state_and_notes() {
    let out = [
        html(VisibilityState { public: true }),
        html(VisibilityState { public: false }),
        html(VisibilityReadOnlyNote {}),
        html(VisibilityAdminOnlyNote {}),
    ]
    .join("\n");
    assert!(out.contains(">Public</span>"));
    assert!(out.contains(">Private</span>"));
    assert!(out.contains("Public is read-only."));
    insta::assert_snapshot!(out);
}

#[test]
fn usage_and_compute_are_reachable_but_not_in_the_sidebar() {
    let out = html(SettingsSidebar {
        org: "acme",
        section: OrgSection::Compute,
    });
    assert!(!out.contains("aria-current"), "{out}");
    assert!(!out.contains("/compute"), "{out}");
    assert_eq!(OrgSection::Usage.href("acme"), "/orgs/acme/settings/usage");
    assert_eq!(
        OrgSection::Compute.href("acme"),
        "/orgs/acme/settings/compute"
    );
}
