use std::time::Duration;

use super::reconnect_delay_secs;

#[test]
fn a_stream_that_ran_for_a_while_reconnects_after_a_second() {
    assert_eq!(reconnect_delay_secs(60, Duration::from_secs(3 * 3600)), 1);
}

#[test]
fn a_stream_that_keeps_failing_keeps_its_backoff() {
    assert_eq!(reconnect_delay_secs(16, Duration::from_millis(200)), 16);
}
