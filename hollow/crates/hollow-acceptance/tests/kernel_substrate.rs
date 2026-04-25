//! Probes the pinned Firecracker guest kernel (vmlinux-6.1.166) for the
//! features an OCI runtime (podman / buildah / runc) needs. Decides whether
//! the v1 `uses:` mode of `forest/exec/1` can rely on container actions or
//! whether we'd need a custom kernel build first.
//!
//! The probe runs *inside* an exec-v1 VM via a `metadata.command` override —
//! cheap, no image bloat, no podman-side risk masking kernel-side findings.
//! The test prints the full probe output through the controller's log
//! channel so the result is greppable in CI; specific asserts focus on
//! the bits that block container runtimes outright (cgroup v2,
//! user-namespace cap, /proc namespace files).

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kernel_has_oci_runtime_prerequisites() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-kprobe-{}", short_token());

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "kernel-probe".into(),
            ..Default::default()
        },
    );

    // Probe script. Plain busybox sh — base image has bash but we don't want
    // to depend on it for a probe whose entire job is to be portable.
    //
    // Each line is `KEY=value` so the test code can grep without parsing.
    // PROBE_END is the sentinel that the probe ran to completion.
    let probe = r#"
echo PROBE_BEGIN
echo "PROBE_KERNEL=$(uname -r)"

if [ -r /sys/fs/cgroup/cgroup.controllers ]; then
  echo "PROBE_CGROUP_V2_CONTROLLERS=$(cat /sys/fs/cgroup/cgroup.controllers | tr ' ' ',')"
else
  echo "PROBE_CGROUP_V2_CONTROLLERS=missing"
fi

if [ -r /sys/fs/cgroup/cgroup.subtree_control ]; then
  echo "PROBE_CGROUP_SUBTREE_CONTROL=$(cat /sys/fs/cgroup/cgroup.subtree_control | tr ' ' ',' || true)"
fi

# /proc/cgroups — present even on cgroup v2 kernels; tells us which
# controllers are compiled in.
if [ -r /proc/cgroups ]; then
  echo "PROBE_PROC_CGROUPS_NAMES=$(awk 'NR>1 {print $1}' /proc/cgroups | tr '\n' ',')"
fi

# User-namespace cap (0 = disabled by sysctl, even if compiled in).
if [ -r /proc/sys/user/max_user_namespaces ]; then
  echo "PROBE_USER_NS_MAX=$(cat /proc/sys/user/max_user_namespaces)"
else
  echo "PROBE_USER_NS_MAX=missing"
fi

# Each /proc/self/ns/* entry implies the corresponding namespace type is
# compiled in (the file is a magic symlink whose mere existence tells us).
for n in user pid mnt net ipc uts cgroup time; do
  if [ -e "/proc/self/ns/$n" ]; then
    echo "PROBE_NS_$n=present"
  else
    echo "PROBE_NS_$n=missing"
  fi
done

# Fuse: needed for fuse-overlayfs as a userspace overlay fallback.
if [ -e /dev/fuse ]; then
  echo "PROBE_DEV_FUSE=present"
else
  echo "PROBE_DEV_FUSE=missing"
fi

# Overlayfs: shows up as a filesystem in /proc/filesystems if compiled in.
echo "PROBE_OVERLAYFS=$(grep -c overlay /proc/filesystems 2>/dev/null || echo 0)"

# iptables / nftables — needed for podman's bridge-mode networking.
echo "PROBE_NETFILTER_TABLES=$(ls /proc/net 2>/dev/null | grep -E '^(ip_tables|nf_tables|ip6_tables)' | tr '\n' ',' || true)"

# Try forking into a fresh user namespace. If the kernel has user-NS
# compiled in AND the sysctl is non-zero, this prints OK; otherwise it
# prints whatever error the kernel returned.
if command -v unshare >/dev/null 2>&1; then
  unshare -U /bin/echo PROBE_UNSHARE_USER=ok 2>&1 || echo "PROBE_UNSHARE_USER=failed"
else
  echo "PROBE_UNSHARE_USER=no-unshare-cmd"
fi

echo PROBE_END
"#;

    let mut metadata = HashMap::new();
    metadata.insert("command".to_string(), probe.to_string());

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-kprobe-1".into(),
        release_intent_id: "int-kprobe-1".into(),
        artifact_id: "art-kprobe-1".into(),
        destination_id: "dest-kprobe-1".into(),
        destination: Some(DestinationInfo {
            name: "kernel-probe-dest".into(),
            environment: "test".into(),
            metadata,
            r#type: Some(DestinationCapability {
                organisation: "forest".into(),
                name: "exec".into(),
                version: 1,
            }),
            organisation: "forest".into(),
        }),
        mode: ReleaseMode::Deploy.into(),
        artifact_store: None,
    };

    orchestrator.fake_server.dispatch(assignment)?;

    let (completion, logs) = orchestrator
        .fake_server
        .wait_for_completion(&release_token, Duration::from_secs(120))
        .await?;

    let stdout: Vec<String> = logs
        .iter()
        .filter(|l| l.channel == "stdout" || l.channel == "stderr")
        .map(|l| l.line.clone())
        .collect();

    // Always print so failures show the full picture without re-running.
    eprintln!("\n--- kernel probe output ---");
    for line in &stdout {
        eprintln!("    {line}");
    }
    eprintln!("---");

    if completion.outcome != ReleaseOutcome::Success {
        panic!(
            "probe job did not complete successfully: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    let probe_value = |key: &str| -> Option<String> {
        let prefix = format!("{key}=");
        stdout
            .iter()
            .find_map(|l| l.strip_prefix(&prefix).map(|v| v.to_string()))
    };

    assert!(
        stdout.iter().any(|l| l == "PROBE_END"),
        "probe didn't reach PROBE_END — script aborted early"
    );

    // Hard requirements for any modern OCI runtime:
    //  * cgroup v2 unified hierarchy mounted with cpu/memory/pids
    //  * user, pid, mnt, net namespaces compiled in
    //  * max_user_namespaces sysctl non-zero
    let cgv2 = probe_value("PROBE_CGROUP_V2_CONTROLLERS")
        .expect("PROBE_CGROUP_V2_CONTROLLERS line missing");
    assert_ne!(
        cgv2, "missing",
        "kernel has no cgroup v2 unified hierarchy — podman v5+ won't run. \
         A custom kernel with CONFIG_CGROUPS=y + cgroup v2 is required."
    );
    for needed in &["cpu", "memory", "pids"] {
        assert!(
            cgv2.contains(needed),
            "cgroup v2 controllers ({cgv2}) missing '{needed}' — required for OCI runtime"
        );
    }

    for ns in &["user", "pid", "mnt", "net"] {
        let v = probe_value(&format!("PROBE_NS_{ns}"))
            .unwrap_or_else(|| panic!("PROBE_NS_{ns} line missing"));
        assert_eq!(
            v, "present",
            "/proc/self/ns/{ns} missing — kernel was built without {ns} namespaces"
        );
    }

    let user_ns_max = probe_value("PROBE_USER_NS_MAX")
        .expect("PROBE_USER_NS_MAX line missing");
    assert_ne!(
        user_ns_max, "missing",
        "max_user_namespaces sysctl absent — kernel may have user-NS disabled"
    );
    assert_ne!(
        user_ns_max, "0",
        "max_user_namespaces=0 — user namespaces compiled in but sysctl-disabled"
    );

    Ok(())
}

fn short_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}
