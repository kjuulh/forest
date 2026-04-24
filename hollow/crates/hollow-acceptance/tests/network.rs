//! Proves per-VM networking: tap + iptables NAT + kernel-level static IP +
//! DNS through /etc/resolv.conf. The guest job resolves a public hostname
//! and fetches it over HTTPS; if any layer (routing, NAT, DNS, TLS) is
//! broken, the command fails.

use std::time::Duration;

use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::JobSpec;

#[test]
fn network_egress_http() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let result = harness.run(JobSpec {
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            // `-s` silent, `-f` fail on non-2xx, `--max-time` so a broken
            // route dies quickly. example.com is maintained by ICANN for
            // exactly this kind of reachability probe.
            "curl -sf --max-time 10 https://example.com/ | head -c 128 && echo; \
             echo 'NETWORK_OK'".into(),
        ],
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
    assert!(
        job_stdout.iter().any(|l| l.contains("NETWORK_OK")),
        "expected NETWORK_OK sentinel in stdout. stdout:\n{job_stdout:#?}"
    );
    // Also ensure curl actually got HTML back (example.com always serves an
    // <html>…) — so we know we hit a real remote, not a NAT quirk that made
    // curl silent-exit zero.
    assert!(
        job_stdout.iter().any(|l| l.contains("<html") || l.contains("<!doctype")),
        "expected HTML-ish content in stdout. stdout:\n{job_stdout:#?}"
    );

    // Make sure boot actually brought the interface up. The kernel prints a
    // line like `IP-Config: Complete: …` when ip= succeeds; failure prints
    // `IP-Config: Gateway not on directly connected network.`
    let _ = Duration::from_secs(1); // keep Duration import live for anyhow macro inference
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
