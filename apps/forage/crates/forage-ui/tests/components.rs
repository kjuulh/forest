use forage_ui::components::*;
use forage_ui::{AlertKind, ButtonVariant, Crumb, NavItem, Tone};
use sedge_rt::Render;

fn html(r: impl Render) -> String {
    r.render_to_string()
}

fn text(s: &'static str) -> impl Fn(&mut String) {
    move |out: &mut String| out.push_str(s)
}

#[test]
fn badges() {
    let out = [
        html(Badge {
            label: "plain",
            ..Default::default()
        }),
        html(Badge {
            label: "ok",
            tone: Tone::Green,
        }),
        html(EnvBadge {
            env: "platform-prod",
        }),
        html(EnvBadge { env: "preprod" }),
        html(KindBadge { kind: "binary" }),
        html(KindBadge {
            kind: "anything-else",
        }),
        html(VisibilityBadge {
            visibility: "public",
        }),
        html(VisibilityBadge { visibility: "" }),
        html(ToolShapeBadge { shape: "hybrid" }),
        html(ToolShapeBadge {
            shape: "tool_external",
        }),
        html(ToolShapeBadge { shape: "unknown" }),
    ]
    .join("\n");
    insta::assert_snapshot!(out);
}

#[test]
fn badge_labels_are_escaped() {
    let out = html(Badge {
        label: "<b>&",
        ..Default::default()
    });
    assert!(out.contains("&lt;b&gt;&amp;"), "{out}");
}

#[test]
fn alerts() {
    let msg = text("Saved.");
    let out = [
        html(Alert {
            kind: AlertKind::Success,
            children: &msg,
        }),
        html(Alert {
            children: &msg,
            ..Default::default()
        }),
    ]
    .join("\n");
    insta::assert_snapshot!(out);
}

#[test]
fn error_block_is_inline_when_short_and_a_box_when_long() {
    assert_eq!(
        html(ErrorBlock {
            message: "  \n ",
            ..Default::default()
        })
        .trim(),
        ""
    );
    let short = html(ErrorBlock {
        message: " denied ",
        ..Default::default()
    });
    assert!(
        short.starts_with("<p class=\"text-xs text-red-600"),
        "{short}"
    );
    assert!(short.contains(">denied</p>"), "{short}");
    let multiline = html(ErrorBlock {
        message: "line one\nline two",
        class: "mt-2",
        ..Default::default()
    });
    assert!(multiline.starts_with("<div"), "{multiline}");
    assert!(
        multiline.contains("whitespace-pre-wrap break-words mt-2"),
        "{multiline}"
    );
    let long = "x".repeat(65);
    assert!(
        html(ErrorBlock {
            message: &long,
            ..Default::default()
        })
        .starts_with("<div")
    );
}

#[test]
fn empty_states() {
    let out = [
        html(EmptyState {
            message: "No releases yet.",
            ..Default::default()
        }),
        html(EmptyState {
            message: "No releases yet.",
            hint: "Push an artifact to start.",
        }),
    ]
    .join("\n");
    insta::assert_snapshot!(out);
}

#[test]
fn pagination_hides_on_one_page_and_encodes_its_filters() {
    assert_eq!(
        html(Pagination {
            base_url: "/x",
            page: 1,
            total_pages: 1,
            params: &[]
        })
        .trim(),
        ""
    );
    let out = html(Pagination {
        base_url: "/orgs/acme/releases",
        page: 2,
        total_pages: 3,
        params: &[("env", "prod & dev"), ("empty", "")],
    });
    insta::assert_snapshot!(out);
}

#[test]
fn code_blocks() {
    let out = [
        html(CodeBlock {
            content: "forest login",
            ..Default::default()
        }),
        html(CodeBlock {
            content: "forest login",
            copyable: true,
            id: "cmd",
            class: "mt-2",
        }),
    ]
    .join("\n");
    assert!(
        !out.contains("onclick"),
        "copying is wired by ui.js, not inline handlers"
    );
    insta::assert_snapshot!(out);
}

#[test]
fn text_fields_render_an_unset_value_as_empty() {
    let unset: Option<String> = None;
    let out = html(TextField {
        label: "Title pattern",
        name: "title_pattern",
        value: unset.as_deref().unwrap_or_default(),
        placeholder: "e.g. ^Release.*",
        mono: true,
        ..Default::default()
    });
    assert!(out.contains("name=\"title_pattern\" value=\"\""), "{out}");
    assert!(!out.contains("value=\"none\""), "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn checkboxes_and_choice_chips() {
    let label = text("platform-dev");
    let out = [
        html(ChoiceChip {
            name: "target_environments",
            value: "platform-dev",
            checked: true,
            children: &label,
        }),
        html(ChoiceChip {
            name: "target_environments",
            value: "prod",
            checked: false,
            children: &label,
        }),
        html(Checkbox {
            id: "force-release",
            name: "force_release",
            label: "Force release",
            checked: false,
            ..Default::default()
        }),
        html(Checkbox {
            id: "use-pipeline",
            name: "use_pipeline",
            label: "Use release pipeline",
            checked: true,
            strong: true,
            ..Default::default()
        }),
        html(CsrfField {
            token: "tok\"en",
            ..Default::default()
        }),
    ]
    .join("\n");
    insta::assert_snapshot!(out);
}

#[test]
fn buttons() {
    let out = [
        html(Button {
            label: "Save",
            ..Default::default()
        }),
        html(Button {
            label: "Cancel",
            variant: ButtonVariant::Secondary,
            href: Some("/back"),
            ..Default::default()
        }),
        html(Button {
            label: "Delete",
            variant: ButtonVariant::SmallDanger,
            ..Default::default()
        }),
    ]
    .join("\n");
    insta::assert_snapshot!(out);
}

#[test]
fn navigation() {
    let crumbs = [
        Crumb::link("acme", "/orgs/acme/projects"),
        Crumb::link("api", "/orgs/acme/projects/api"),
        Crumb::here("Triggers"),
    ];
    let items = [
        NavItem::new("Triggers", "/t", true),
        NavItem::new("Policies", "/p", false),
    ];
    let out = [
        html(Breadcrumbs { crumbs: &crumbs, ..Default::default() }),
        html(BackLink {
            href: "/orgs/acme/projects/api",
            label: "api",
        }),
        html(SideNav {
            items: &items,
            label: "Project settings sections",
        }),
        html(PillNav {
            items: &items,
            label: "Project settings sections",
        }),
        html(Tabs { items: &items }),
    ]
    .join("\n");
    insta::assert_snapshot!(out);
}

#[test]
fn unknown_icons_fall_back_instead_of_rendering_nothing() {
    let out = html(Icon {
        name: "no-such-icon",
        ..Default::default()
    });
    assert!(out.starts_with("<svg"), "{out}");
}

#[test]
fn page_heading_with_and_without_actions() {
    let actions = text("<a href=\"/new\">New</a>");
    let out = [
        html(PageHeading {
            title: "Releases",
            ..Default::default()
        }),
        html(PageHeading {
            title: "Releases",
            actions: Some(&actions),
        }),
    ]
    .join("\n");
    insta::assert_snapshot!(out);
}
