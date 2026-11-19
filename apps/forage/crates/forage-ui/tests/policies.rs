use forage_ui::components::*;
use forage_ui::policies::PolicyKind;
use sedge_rt::Render;

fn html(r: impl Render) -> String {
    r.render_to_string()
}

fn text(s: &'static str) -> impl Fn(&mut String) {
    move |out: &mut String| out.push_str(s)
}

#[test]
fn policy_kind_chips_name_and_colour_every_kind() {
    let out: Vec<String> = PolicyKind::ALL
        .iter()
        .map(|kind| {
            html(PolicyKindChip {
                kind: *kind,
                class: "",
            })
        })
        .collect();
    assert_eq!(
        out[0],
        r#"<span class="bg-indigo-100 text-indigo-700 px-1.5 py-0.5 rounded">Soak Time</span>"#
    );
    assert!(
        out[1].contains("bg-orange-100 text-orange-700") && out[1].contains(">Branch Restriction<")
    );
    assert!(
        out[2].contains("bg-emerald-100 text-emerald-700")
            && out[2].contains(">Approval Required<")
    );
    assert!(out[3].contains("bg-sky-100 text-sky-700") && out[3].contains(">Supersede Pending<"));
    let edit = html(PolicyKindChip {
        kind: PolicyKind::Approval,
        class: "text-xs font-medium",
    });
    assert!(edit.contains(r#"rounded text-xs font-medium">"#), "{edit}");
}

#[test]
fn policy_kind_slugs_match_the_form_values_the_server_reads() {
    let slugs: Vec<&str> = PolicyKind::ALL.iter().map(|k| k.slug()).collect();
    assert_eq!(
        slugs,
        [
            "soak_time",
            "branch_restriction",
            "approval",
            "supersede_pending"
        ]
    );
    assert_eq!(PolicyKind::default(), PolicyKind::SoakTime);
}

#[test]
fn environment_select_marks_the_selected_environment_and_escapes_names() {
    let envs = vec!["dev".to_string(), "prod".to_string(), "a\"b".to_string()];
    let out = html(EnvironmentSelect {
        name: "target_environment",
        environments: &envs,
        selected: "prod",
        id: "",
        disabled: false,
    });
    assert!(
        out.starts_with(r#"<select name="target_environment" class="#),
        "{out}"
    );
    assert!(out.contains(r#"<option value="dev">dev</option>"#), "{out}");
    assert!(
        out.contains(r#"<option value="prod" selected>prod</option>"#),
        "{out}"
    );
    assert!(
        out.contains(r#"<option value="a&quot;b">a&quot;b</option>"#),
        "{out}"
    );
    assert!(!out.contains("disabled"), "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn a_disabled_environment_select_keeps_its_id() {
    let envs = vec!["dev".to_string()];
    let out = html(EnvironmentSelect {
        name: "target_environment",
        environments: &envs,
        selected: "",
        id: "approval-target-env",
        disabled: true,
    });
    assert!(
        out.contains(
            r#"<select name="target_environment" id="approval-target-env" disabled class="#
        ),
        "{out}"
    );
    assert!(!out.contains("selected"), "{out}");
}

#[test]
fn policy_fields_hide_inactive_sections() {
    let body = text("<input>");
    let active = html(PolicyFields {
        kind: PolicyKind::SoakTime,
        active: true,
        children: &body,
    });
    assert_eq!(active, r#"<div id="soak-time-fields"><input></div>"#);
    let hidden = html(PolicyFields {
        kind: PolicyKind::SupersedePending,
        active: false,
        children: &body,
    });
    assert_eq!(
        hidden,
        r#"<div id="supersede-pending-fields" class="hidden"><input></div>"#
    );
}

#[test]
fn pipeline_builder_points_at_its_textarea() {
    let edit = html(PipelineBuilder {
        target: "edit-a b",
        readonly: false,
    });
    assert_eq!(
        edit,
        r#"<pipeline-builder data-target="edit-a b"></pipeline-builder>"#
    );
    let view = html(PipelineBuilder {
        target: "view-x",
        readonly: true,
    });
    assert_eq!(
        view,
        r#"<pipeline-builder data-target="view-x" data-readonly="true"></pipeline-builder>"#
    );
}

#[test]
fn an_environment_that_no_longer_exists_stays_selected() {
    let envs = vec!["dev".to_string(), "prod".to_string()];
    let html = sedge_rt::Render::render_to_string(&forage_ui::components::EnvironmentSelect {
        name: "target_environment",
        environments: &envs,
        selected: "staging",
        ..Default::default()
    });
    assert!(html.contains(r#"<option value="staging" selected>staging (no longer an environment)</option>"#), "{html}");
    assert!(!html.contains(r#"value="dev" selected"#), "{html}");
}
