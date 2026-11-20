use forage_ui::components::*;
use sedge_rt::Render;

fn html(r: impl Render) -> String {
    r.render_to_string()
}

#[test]
fn environment_dots_follow_the_environment_name() {
    let dot = |env| html(EnvDot { env });
    assert!(dot("prod").contains("bg-pink-500"));
    assert!(dot("preprod").contains("bg-orange-500"));
    assert!(dot("pre-prod").contains("bg-orange-500"));
    assert!(dot("staging").contains("bg-yellow-500"));
    assert!(dot("dev").contains("bg-violet-500"));
    assert!(dot("sandbox").contains("bg-gray-400"));
}

#[test]
fn a_type_chip_shows_a_version_only_when_there_is_one() {
    assert_eq!(
        html(TypeChip {
            name: "flux",
            version: 3
        }),
        r#"<span class="text-xs px-1.5 py-0.5 rounded-full bg-blue-50 text-blue-700">flux v3</span>"#
    );
    assert!(
        html(TypeChip {
            name: "flux",
            ..Default::default()
        })
        .contains(">flux</span>")
    );
    assert!(
        html(TypeChip {
            name: "<b>",
            version: 1
        })
        .contains("&lt;b&gt; v1")
    );
}

#[test]
fn metadata_row_controls_carry_no_inline_handlers() {
    let out = [
        html(SecretToggle {
            title: "Mark \"secret\"",
        }),
        html(RemoveRowButton {}),
    ]
    .join("\n");
    assert!(!out.contains("onclick"), "{out}");
    assert!(
        out.contains(r#"class="meta-sensitive accent-green-600""#),
        "{out}"
    );
    assert!(out.contains(r#"title="Mark &quot;secret&quot;""#), "{out}");
    assert!(out.contains(r#"class="remove-meta-row "#), "{out}");
    insta::assert_snapshot!(out);
}
