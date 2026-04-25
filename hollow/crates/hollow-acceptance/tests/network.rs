//! Proves per-VM networking: tap + iptables NAT + kernel-level static IP +
//! DNS through /etc/resolv.conf, AND that the egress lockdown rules block
//! traffic to the host LAN, RFC1918 destinations, and cloud IMDS.
//!
//! If any layer (routing, NAT, DNS, TLS) is broken, the public-reach assertion
//! fails. If the lockdown rules are missing, the negative assertions fail —
//! a malicious guest could otherwise grab IMDS credentials in seconds.

use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::JobSpec;

const PROBE_SCRIPT: &str = r#"
set +e
# 1. Public reach should work (ICANN's reference site).
if curl -sf --max-time 10 https://example.com/ | head -c 128 > /tmp/public_body; then
  PUBLIC=OK
  echo BODY=$(cat /tmp/public_body)
else
  PUBLIC=FAIL
fi
echo PUBLIC=$PUBLIC

# 2. Cloud IMDS must be unreachable. We DROP, not REJECT, so connect hangs;
# --connect-timeout 3 catches it without dragging the test out.
if curl -sf --connect-timeout 3 --max-time 4 http://169.254.169.254/latest/meta-data/ > /dev/null 2>&1; then
  IMDS=LEAK
else
  IMDS=BLOCKED
fi
echo IMDS=$IMDS

# 3. Reaching anything on RFC1918 should fail. Pick an address very unlikely
# to host anything (broadcasts to every private subnet). DROP again, so the
# probe just times out.
if curl -sf --connect-timeout 3 --max-time 4 http://10.255.255.254/ > /dev/null 2>&1; then
  RFC1918=LEAK
else
  RFC1918=BLOCKED
fi
echo RFC1918=$RFC1918

# 4. The host's tap IP itself (the VM's gateway) must not expose anything
# either, since the host typically has services bound to 0.0.0.0.
if curl -sf --connect-timeout 3 --max-time 4 http://10.200.0.1/ > /dev/null 2>&1; then
  HOST=LEAK
else
  HOST=BLOCKED
fi
echo HOST=$HOST

echo NETWORK_PROBE_DONE
exit 0
"#;

#[test]
fn network_egress_locked_down() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let result = harness.run(JobSpec {
        command: vec!["/bin/sh".into(), "-c".into(), PROBE_SCRIPT.into()],
        network: true,
        timeout_seconds: Some(60),
        ..Default::default()
    })?;

    assert_eq!(
        result.exit_code, 0,
        "expected clean exit, got {}. logs:\n{:#?}",
        result.exit_code,
        result
            .logs
            .iter()
            .filter(|l| l.channel != "console")
            .collect::<Vec<_>>()
    );

    let job_stdout: Vec<&str> = result
        .logs
        .iter()
        .filter(|l| l.channel == "stdout")
        .map(|l| l.line.as_str())
        .collect();

    let assert_marker = |needle: &str, hint: &str| {
        assert!(
            job_stdout.iter().any(|l| l.trim() == needle),
            "{hint} — looked for `{needle}`. stdout:\n{job_stdout:#?}"
        );
    };

    assert_marker("PUBLIC=OK", "public internet should be reachable");
    assert_marker(
        "IMDS=BLOCKED",
        "cloud IMDS at 169.254.169.254 must be unreachable from a guest",
    );
    assert_marker(
        "RFC1918=BLOCKED",
        "RFC1918 destinations must be unreachable (would expose host LAN)",
    );
    assert_marker(
        "HOST=BLOCKED",
        "host services on the tap IP must be unreachable (INPUT chain hole)",
    );
    assert_marker(
        "NETWORK_PROBE_DONE",
        "probe didn't run to completion — earlier check hung?",
    );
    assert!(
        job_stdout
            .iter()
            .any(|l| l.contains("<html") || l.contains("<!doctype") || l.contains("BODY=")),
        "public probe didn't actually return HTML. stdout:\n{job_stdout:#?}"
    );

    let console: Vec<&str> = result
        .logs
        .iter()
        .filter(|l| l.channel == "console")
        .map(|l| l.line.as_str())
        .collect();
    assert!(
        console.iter().any(|l| l.contains("IP-Config:")),
        "kernel IP-Config log missing — did the `ip=` boot arg get applied?"
    );

    Ok(())
}
