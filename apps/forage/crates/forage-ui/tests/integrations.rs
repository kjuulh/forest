use forage_ui::AlertKind;
use forage_ui::components::*;
use forage_ui::org_settings::OrgSection;
use sedge_rt::Render;

fn html(r: impl Render) -> String {
    r.render_to_string()
}

fn text(s: &'static str) -> impl Fn(&mut String) {
    move |out: &mut String| out.push_str(s)
}

#[test]
fn settings_sidebar_marks_the_current_section() {
    let out = html(SettingsSidebar {
        org: "acme",
        section: OrgSection::Integrations,
    });
    assert!(
        out.contains(r#"<a href="/orgs/acme/settings/integrations" aria-current="page""#),
        "{out}"
    );
    assert_eq!(out.matches("aria-current").count(), 1, "{out}");
    assert!(!out.contains("onclick"), "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn settings_links_encode_the_organisation() {
    let out = html(SettingsSidebar {
        org: "a b/c",
        ..Default::default()
    });
    assert!(
        out.contains(r#"href="/orgs/a%20b%2Fc/settings/members""#),
        "{out}"
    );
    let back = html(BackToIntegrations { org: "a b/c" });
    assert!(
        back.contains(r#"href="/orgs/a%20b%2Fc/settings/integrations""#),
        "{back}"
    );
}

#[test]
fn integration_logos_pills_and_flashes() {
    let saved = text("Saved.");
    let out = [
        html(IntegrationLogo {
            kind: "webhook",
            ..Default::default()
        }),
        html(IntegrationIcon {
            kind: "slack",
            class: "w-5 h-5",
        }),
        html(IntegrationIcon {
            kind: "unknown",
            ..Default::default()
        }),
        html(ActivePill { enabled: true }),
        html(ActivePill { enabled: false }),
        html(Flash {
            kind: AlertKind::Success,
            children: &saved,
        }),
        html(Flash {
            children: &saved,
            ..Default::default()
        }),
        html(SectionLabel {
            class: "mb-4",
            children: &text("Installed"),
        }),
    ]
    .join("\n");
    assert!(
        out.contains("<svg"),
        "an unknown kind still renders an icon: {out}"
    );
    insta::assert_snapshot!(out);
}

#[test]
fn integration_cards_link_when_installable_and_say_coming_soon_otherwise() {
    let out = [
        html(IntegrationCard {
            kind: "slack",
            name: "Slack",
            description: "Post to <channels>.",
            href: Some("/orgs/acme/settings/integrations/install/slack"),
        }),
        html(IntegrationCard {
            kind: "discord",
            name: "Discord",
            description: "Later.",
            href: None,
        }),
    ]
    .join("\n");
    assert!(out.contains("Post to &lt;channels&gt;."), "{out}");
    assert_eq!(out.matches("Coming soon").count(), 1, "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn install_fields_and_steps() {
    let hint = text("Must use <code>HTTPS</code>");
    let step = text("Forest sends a request");
    let steps = |out: &mut String| {
        Step {
            n: 1,
            children: &step,
        }
        .render(out)
    };
    let out = [
        html(InstallField {
            name: "url",
            label: "Payload URL",
            kind: "url",
            placeholder: "https://example.com",
            required: true,
            mono: true,
            hint: &hint,
            ..Default::default()
        }),
        html(InstallField {
            name: "secret",
            label: "Secret",
            optional: true,
            hint: &text("Optional"),
            ..Default::default()
        }),
        html(Steps {
            title: "How it works",
            children: &steps,
        }),
    ]
    .join("\n");
    assert!(
        out.contains(
            r#"<input type="url" id="url" name="url" placeholder="https://example.com" required"#
        ),
        "{out}"
    );
    assert!(
        out.contains(r#"<input type="text" id="secret" name="secret" placeholder="" class"#),
        "{out}"
    );
    insta::assert_snapshot!(out);
}
