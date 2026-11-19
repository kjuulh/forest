use forage_ui::Tone;
use forage_ui::components::*;
use forage_ui::release::{DestinationRun, RunStatus, Stage, StageKind};
use sedge_rt::Render;

fn html(r: impl Render) -> String {
    r.render_to_string()
}

fn stage(kind: StageKind, status: RunStatus) -> Stage {
    Stage {
        id: "plan prod".into(),
        anchor: "plan-prod".into(),
        kind,
        environment: "prod".into(),
        status,
        ..Default::default()
    }
}

#[test]
fn stage_rows() {
    let actions = |out: &mut String| out.push_str("<form></form>");
    let awaiting = Stage {
        plan_output: Some("+ table <new>".into()),
        error_message: Some("short reason".into()),
        ..stage(StageKind::Plan, RunStatus::AwaitingApproval)
    };
    let out = [
        html(StageRow {
            stage: &awaiting,
            actions: Some(&actions),
        }),
        html(StageRow {
            stage: &stage(StageKind::Deploy, RunStatus::Pending),
            actions: None,
        }),
        html(StageRow {
            stage: &Stage {
                duration_seconds: Some(30),
                started_at: Some("2026-03-07T12:00:00Z".into()),
                ..stage(StageKind::Wait, RunStatus::Running)
            },
            actions: None,
        }),
    ];
    assert!(out[0].contains(r#"<details data-plan-output class="px-4 pb-3" open>"#));
    assert!(
        out[0].contains("+ table &lt;new&gt;"),
        "plan output is escaped"
    );
    assert!(out[0].contains("<form></form>"));
    assert!(out[0].contains("bg-pink-100 text-pink-800\">prod</span>"));
    assert!(out[1].contains("opacity-50") && out[1].contains("Deploy to"));
    assert!(
        !out[1].contains("<details"),
        "no plan output, no plan block"
    );
    assert!(
        !out[2].contains(">prod</span>"),
        "a wait has no environment badge"
    );
    assert!(out[2].contains("Waiting 30s"));
    let masked = regex_free_mask(&out.join("\n"));
    insta::assert_snapshot!(masked);
}

fn regex_free_mask(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<time ") {
        let close = rest[i..].find('>').unwrap() + i + 1;
        let end = rest[close..].find("</time>").unwrap() + close;
        out.push_str(&rest[..close]);
        out.push_str("AGO");
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[test]
fn destination_rows_and_status_icons() {
    let queued = DestinationRun {
        name: "prod-eu".into(),
        anchor: "prod-eu".into(),
        environment: "prod".into(),
        status: RunStatus::Queued,
        queue_position: Some(2),
        ..Default::default()
    };
    let out = html(DestinationRow { dest: &queued });
    assert!(out.contains(r#"id="dest-prod-eu""#));
    assert!(out.contains("Queued #2"));
    assert!(out.contains("text-blue-400"), "queued clock icon");

    let idle = DestinationRun::default();
    let out = html(DestinationRow { dest: &idle });
    assert!(
        out.contains("M9 12l2 2 4-4"),
        "an idle destination keeps the check outline"
    );
    assert!(
        !out.contains("text-xs text-"),
        "no status text for an idle destination"
    );

    let pending = html(PendingDestinationRow {
        name: "prod-us",
        environment: "prod",
    });
    assert!(pending.contains("Not deployed"));
}

#[test]
fn chips_badges_and_links() {
    let label = |out: &mut String| out.push_str("ci-bot");
    let chip = html(OutlineChip {
        tone: Tone::Amber,
        icon: "user",
        href: Some("javascript:alert(1)"),
        children: &label,
    });
    assert!(!chip.contains("javascript:"), "{chip}");
    assert!(chip.contains("bg-amber-50"));

    assert!(
        html(PolicyTypeBadge {
            policy_type: "soak_time"
        })
        .contains("Soak Time")
    );
    assert_eq!(
        html(PolicyTypeBadge {
            policy_type: "unknown"
        })
        .trim(),
        ""
    );
    assert!(
        html(ReleaseStatusDot {
            status: RunStatus::Running
        })
        .contains("animate-pulse")
    );

    let text = |out: &mut String| out.push_str("Web");
    let link = html(ExternalLinkButton {
        href: "https://example.com",
        children: &text,
    });
    assert!(link.contains(r#"target="_blank" rel="noopener""#));
}
