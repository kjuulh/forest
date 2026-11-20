use std::sync::{Arc, Mutex};

use forage_core::platform::PlatformError;
use tracing_subscriber::fmt::MakeWriter;

use super::default_unless_missing;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn logs_of(f: impl FnOnce()) -> String {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = captured.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn a_missing_manifest_logs_at_debug_and_renders_empty() {
    let mut value = None;
    let logs = logs_of(|| {
        value = Some(default_unless_missing::<String>(
            "get_public_component_manifest",
            Err(PlatformError::NotFound(
                "manifest not found for forest/sdk@0.9.0".into(),
            )),
        ));
    });
    assert_eq!(value.as_deref(), Some(""));
    assert!(logs.contains("DEBUG"), "{logs}");
    assert!(!logs.contains("WARN"), "{logs}");
}

#[test]
fn any_other_manifest_error_still_warns() {
    let logs = logs_of(|| {
        default_unless_missing::<String>(
            "get_public_component_manifest",
            Err(PlatformError::Other("connection refused".into())),
        );
    });
    assert!(logs.contains("WARN"), "{logs}");
}
