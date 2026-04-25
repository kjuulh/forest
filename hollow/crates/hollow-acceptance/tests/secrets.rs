//! Verifies the secret-shipping channel: secrets are written to the guest's
//! filesystem at the requested path with the requested mode, the command can
//! read them, and the *content* never appears in any log channel (only the
//! `name`/`target_path`/`mode` triple, used for diagnostics).
//!
//! The probe ships a known plaintext under `/run/test-secret`, has the guest
//! command sha256sum the file, and asserts the digest matches the
//! locally-computed digest. The plaintext itself is never printed by the
//! command, so any leak via `[stdout]`/`[stderr]`/`[console]` channels would
//! be a real-world bug.

use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::{JobSecret, JobSpec};
use sha2::{Digest, Sha256};

const SECRET_PLAINTEXT: &[u8] = b"-----BEGIN FAKE SSH KEY-----\nrgC0FF33-N0t-A-ReaL-Key-Just-Probe-Material-1234567890\n-----END FAKE SSH KEY-----\n";
const SECRET_PATH: &str = "/run/forest-test-secret";

#[test]
fn secret_is_delivered_and_not_leaked_to_logs() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let expected_digest_hex = {
        let mut h = Sha256::new();
        h.update(SECRET_PLAINTEXT);
        format!("{:x}", h.finalize())
    };

    // The script reads the secret, prints its SHA-256, and prints the file's
    // permission bits — both checks the test asserts. It deliberately never
    // `cat`s or otherwise echoes the contents.
    let probe = format!(
        r#"set -e
test -f {path}
echo "MODE=$(stat -c '%a' {path})"
echo "SHA=$(sha256sum {path} | awk '{{print $1}}')"
echo PROBE_DONE
"#,
        path = SECRET_PATH,
    );

    let result = harness.run(JobSpec {
        command: vec!["/bin/sh".into(), "-c".into(), probe],
        secrets: vec![JobSecret {
            name: "test-secret".into(),
            target_path: SECRET_PATH.into(),
            content: SECRET_PLAINTEXT.to_vec(),
            mode: 0o600,
        }],
        capture_console: true,
        ..Default::default()
    })?;

    assert_eq!(
        result.exit_code, 0,
        "probe should exit 0; logs:\n{:#?}",
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

    assert!(
        stdout.iter().any(|l| l.trim() == "PROBE_DONE"),
        "probe didn't run to completion. stdout:\n{stdout:#?}"
    );
    assert!(
        stdout
            .iter()
            .any(|l| l.contains(&format!("SHA={expected_digest_hex}"))),
        "secret content sha mismatch — file wasn't written with the expected bytes. \
         expected={expected_digest_hex}; stdout:\n{stdout:#?}"
    );
    assert!(
        stdout.iter().any(|l| l.contains("MODE=600")),
        "secret mode was not 0600. stdout:\n{stdout:#?}"
    );

    // Critical: a substring of the plaintext must NOT appear anywhere in
    // any log channel. We don't print it from the probe; if it shows up
    // we have a real-world leak through the agent or guest tracing.
    let needle = "rgC0FF33-N0t-A-ReaL-Key";
    for log in &result.logs {
        assert!(
            !log.line.contains(needle),
            "secret material leaked into channel `{}`: line=`{}`",
            log.channel,
            log.line,
        );
    }

    Ok(())
}
