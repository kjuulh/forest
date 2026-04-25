//! Per-destination egress allowlist: when `allowed_egress_cidrs` is non-empty,
//! the VM may only reach those CIDRs; everything else (including the
//! otherwise-public internet) is dropped.
//!
//! Probe shape:
//!   - 1.1.1.1 (allowlisted)        → must succeed
//!   - 8.8.8.8 (NOT allowlisted)    → must time out (DROP, not REJECT)
//!   - 169.254.169.254 (always blocked) → must time out
//!
//! Connect-timeout 3s on the negative probes keeps the test fast even though
//! we're using DROP (which would otherwise hang until the OS gives up).

use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::JobSpec;

const PROBE_SCRIPT: &str = r#"
set +e
# 1. Allowlisted destination should succeed.
if curl -sf --connect-timeout 5 --max-time 6 https://1.1.1.1/ > /dev/null 2>&1; then
  ALLOWED=OK
else
  ALLOWED=FAIL
fi
echo ALLOWED=$ALLOWED

# 2. A public IP NOT in the allowlist (8.8.8.8) must be unreachable.
if curl -sf --connect-timeout 3 --max-time 4 https://8.8.8.8/ > /dev/null 2>&1; then
  PUBLIC_OUTSIDE=LEAK
else
  PUBLIC_OUTSIDE=BLOCKED
fi
echo PUBLIC_OUTSIDE=$PUBLIC_OUTSIDE

# 3. IMDS stays blocked unconditionally.
if curl -sf --connect-timeout 3 --max-time 4 http://169.254.169.254/latest/meta-data/ > /dev/null 2>&1; then
  IMDS=LEAK
else
  IMDS=BLOCKED
fi
echo IMDS=$IMDS

echo ALLOWLIST_PROBE_DONE
exit 0
"#;

#[test]
fn egress_allowlist_restricts_to_listed_cidrs() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let result = harness.run(JobSpec {
        command: vec!["/bin/sh".into(), "-c".into(), PROBE_SCRIPT.into()],
        network: true,
        timeout_seconds: Some(60),
        capture_console: true,
        // Pin egress to just 1.1.1.1 (Cloudflare). Everything else from
        // this VM should be dropped at the FORWARD chain.
        allowed_egress_cidrs: vec!["1.1.1.1/32".into()],
        ..Default::default()
    })?;

    assert_eq!(
        result.exit_code, 0,
        "probe script itself should exit 0 (curl failures are recorded as labels, not exit codes). logs:\n{:#?}",
        result
            .logs
            .iter()
            .filter(|l| l.channel != "console")
            .collect::<Vec<_>>(),
    );

    let stdout: Vec<&str> = result
        .logs
        .iter()
        .filter(|l| l.channel == "stdout")
        .map(|l| l.line.as_str())
        .collect();

    let assert_marker = |needle: &str, hint: &str| {
        assert!(
            stdout.iter().any(|l| l.trim() == needle),
            "{hint} — looked for `{needle}`. stdout:\n{stdout:#?}"
        );
    };

    assert_marker(
        "ALLOWED=OK",
        "1.1.1.1 should be reachable when in the allowlist",
    );
    assert_marker(
        "PUBLIC_OUTSIDE=BLOCKED",
        "8.8.8.8 must be unreachable when not in the allowlist (otherwise it isn't actually an allowlist)",
    );
    assert_marker(
        "IMDS=BLOCKED",
        "169.254.169.254 must always be blocked",
    );
    assert_marker(
        "ALLOWLIST_PROBE_DONE",
        "probe didn't run to completion — earlier check hung?",
    );

    Ok(())
}
