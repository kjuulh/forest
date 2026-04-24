//! Stage A smoke test: prove the full chain
//!   harness → ssh → hollow-test-runner → firecracker → hollow-guest → vsock
//! works end to end with a trivial command.

use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::JobSpec;

#[test]
fn echo_hello_in_microvm() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let result = harness.run(JobSpec {
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo hello-from-vm".into(),
        ],
        ..Default::default()
    })?;

    assert_eq!(
        result.exit_code, 0,
        "expected clean exit; full result: {result:#?}"
    );
    assert!(
        result
            .log_lines()
            .any(|l| l.trim() == "hello-from-vm"),
        "missing expected log line. got: {:#?}",
        result.logs
    );

    // Sanity-check we walked through every lifecycle stage we expect.
    for required in [
        "spec_loaded",
        "vm_spawn",
        "vm_start",
        "await_guest",
        "guest_ready",
        "job_dispatched",
        "vm_shutdown",
    ] {
        assert!(
            result.stages.iter().any(|s| s == required),
            "missing stage `{required}` — saw {:?}",
            result.stages
        );
    }

    // Guest console (kernel dmesg + hollow-guest PID 1 output) should be
    // surfaced as channel="console" log lines. At minimum the kernel banner
    // should appear.
    let console: Vec<&str> = result
        .logs
        .iter()
        .filter(|l| l.channel == "console")
        .map(|l| l.line.as_str())
        .collect();
    assert!(
        !console.is_empty(),
        "no guest console lines captured — firecracker stdout was empty?"
    );
    assert!(
        console.iter().any(|l| l.contains("Linux version")),
        "console did not contain the kernel banner; first 5 lines: {:?}",
        console.iter().take(5).collect::<Vec<_>>()
    );

    Ok(())
}
