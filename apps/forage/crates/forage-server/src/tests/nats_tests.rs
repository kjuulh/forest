use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use forage_core::integrations::nats::NotificationEnvelope;
use forage_core::integrations::router::{NotificationEvent, ReleaseContext};
use forage_core::integrations::{
    CreateIntegrationInput, DeliveryStatus, IntegrationConfig, IntegrationStore, IntegrationType,
    InMemoryIntegrationStore,
};
use tokio::net::TcpListener;

use crate::notification_consumer::NotificationConsumer;
use crate::notification_worker::NotificationDispatcher;

// ─── Test webhook receiver (same pattern as webhook_delivery_tests) ──

#[derive(Debug, Clone)]
struct ReceivedWebhook {
    body: String,
    signature: Option<String>,
}

#[derive(Clone)]
struct ReceiverState {
    deliveries: Arc<Mutex<Vec<ReceivedWebhook>>>,
}

async fn webhook_handler(
    State(state): State<ReceiverState>,
    req: Request<Body>,
) -> impl IntoResponse {
    let sig = req
        .headers()
        .get("x-forage-signature")
        .map(|v| v.to_str().unwrap_or("").to_string());

    let bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes).to_string();

    state.deliveries.lock().unwrap().push(ReceivedWebhook {
        body,
        signature: sig,
    });

    StatusCode::OK
}

async fn start_receiver() -> (String, ReceiverState) {
    let state = ReceiverState {
        deliveries: Arc::new(Mutex::new(Vec::new())),
    };

    let app = Router::new()
        .route("/hook", post(webhook_handler))
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://127.0.0.1:{}/hook", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (url, state)
}

fn test_event(org: &str) -> NotificationEvent {
    NotificationEvent {
        id: format!("nats-test-{}", uuid::Uuid::new_v4()),
        notification_type: "release_succeeded".into(),
        title: "Deploy v3.0 succeeded".into(),
        body: "All checks passed".into(),
        organisation: org.into(),
        project: "my-svc".into(),
        timestamp: "2026-03-09T16:00:00Z".into(),
        release: Some(ReleaseContext {
            slug: "v3.0".into(),
            artifact_id: "art_nats".into(),
            release_intent_id: "ri_nats".into(),
            destination: "prod".into(),
            environment: "production".into(),
            source_username: "alice".into(),
            source_user_id: String::new(),
            commit_sha: "aabbccdd".into(),
            commit_branch: "main".into(),
            context_title: "Deploy v3.0 succeeded".into(),
            context_web: String::new(),
            destination_count: 1,
            error_message: None,
        }),
    }
}

fn failed_event(org: &str) -> NotificationEvent {
    NotificationEvent {
        id: format!("nats-fail-{}", uuid::Uuid::new_v4()),
        notification_type: "release_failed".into(),
        title: "Deploy v3.0 failed".into(),
        body: "OOM killed".into(),
        organisation: org.into(),
        project: "my-svc".into(),
        timestamp: "2026-03-09T16:05:00Z".into(),
        release: Some(ReleaseContext {
            slug: "v3.0".into(),
            artifact_id: "art_nats".into(),
            release_intent_id: "ri_nats".into(),
            destination: "prod".into(),
            environment: "production".into(),
            source_username: "bob".into(),
            source_user_id: String::new(),
            commit_sha: "deadbeef".into(),
            commit_branch: "hotfix".into(),
            context_title: "Deploy v3.0 failed".into(),
            context_web: String::new(),
            destination_count: 1,
            error_message: Some("OOM killed".into()),
        }),
    }
}

// ─── Unit tests: process_payload without NATS ────────────────────────

#[tokio::test]
async fn process_payload_routes_and_dispatches_to_webhook() {
    let (url, receiver) = start_receiver().await;
    let store = Arc::new(InMemoryIntegrationStore::new());

    store
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "nats-hook".into(),
            config: IntegrationConfig::Webhook {
                url,
                secret: Some("nats-secret".into()),
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    let event = test_event("testorg");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());
    NotificationConsumer::process_payload(&payload, store.as_ref(), &dispatcher)
        .await
        .unwrap();

    let deliveries = receiver.deliveries.lock().unwrap();
    assert_eq!(deliveries.len(), 1, "webhook should receive the event");

    let d = &deliveries[0];
    assert!(d.signature.is_some(), "should be signed");

    let body: serde_json::Value = serde_json::from_str(&d.body).unwrap();
    assert_eq!(body["event"], "release_succeeded");
    assert_eq!(body["organisation"], "testorg");
    assert_eq!(body["project"], "my-svc");
}

#[tokio::test]
async fn process_payload_skips_when_no_matching_integrations() {
    let store = Arc::new(InMemoryIntegrationStore::new());

    // No integrations created — should skip silently
    let event = test_event("testorg");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());
    let result = NotificationConsumer::process_payload(&payload, store.as_ref(), &dispatcher).await;
    assert!(result.is_ok(), "should succeed with no matching integrations");
}

#[tokio::test]
async fn process_payload_rejects_invalid_json() {
    let store = Arc::new(InMemoryIntegrationStore::new());
    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());

    let result =
        NotificationConsumer::process_payload(b"not-json", store.as_ref(), &dispatcher).await;
    assert!(result.is_err(), "invalid JSON should fail");
    assert!(
        result.unwrap_err().contains("deserialize"),
        "error should mention deserialization"
    );
}

#[tokio::test]
async fn process_payload_respects_disabled_rules() {
    let (url, receiver) = start_receiver().await;
    let store = Arc::new(InMemoryIntegrationStore::new());

    let integration = store
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "rule-hook".into(),
            config: IntegrationConfig::Webhook {
                url,
                secret: None,
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    // Disable release_succeeded
    store
        .set_rule_enabled(&integration.id, "release_succeeded", false)
        .await
        .unwrap();

    let event = test_event("testorg"); // release_succeeded
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());
    NotificationConsumer::process_payload(&payload, store.as_ref(), &dispatcher)
        .await
        .unwrap();

    assert!(
        receiver.deliveries.lock().unwrap().is_empty(),
        "disabled rule should prevent delivery"
    );

    // But release_failed should still work
    let event = failed_event("testorg");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    NotificationConsumer::process_payload(&payload, store.as_ref(), &dispatcher)
        .await
        .unwrap();

    assert_eq!(
        receiver.deliveries.lock().unwrap().len(),
        1,
        "release_failed should still deliver"
    );
}

#[tokio::test]
async fn process_payload_dispatches_to_multiple_integrations() {
    let (url1, receiver1) = start_receiver().await;
    let (url2, receiver2) = start_receiver().await;
    let store = Arc::new(InMemoryIntegrationStore::new());

    store
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "hook-a".into(),
            config: IntegrationConfig::Webhook {
                url: url1,
                secret: None,
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    store
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "hook-b".into(),
            config: IntegrationConfig::Webhook {
                url: url2,
                secret: None,
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    let event = test_event("testorg");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());
    NotificationConsumer::process_payload(&payload, store.as_ref(), &dispatcher)
        .await
        .unwrap();

    assert_eq!(receiver1.deliveries.lock().unwrap().len(), 1);
    assert_eq!(receiver2.deliveries.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn process_payload_records_delivery_status() {
    let (url, _receiver) = start_receiver().await;
    let store = Arc::new(InMemoryIntegrationStore::new());

    let integration = store
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "status-hook".into(),
            config: IntegrationConfig::Webhook {
                url,
                secret: None,
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    let event = test_event("testorg");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());
    NotificationConsumer::process_payload(&payload, store.as_ref(), &dispatcher)
        .await
        .unwrap();

    // Verify delivery was recorded
    let deliveries = store.list_deliveries(&integration.id, 10).await.unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].status, DeliveryStatus::Delivered);
    assert!(deliveries[0].error_message.is_none());
}

#[tokio::test]
async fn process_payload_records_failed_delivery() {
    let store = Arc::new(InMemoryIntegrationStore::new());

    let integration = store
        .create_integration(&CreateIntegrationInput {
            organisation: "testorg".into(),
            integration_type: IntegrationType::Webhook,
            name: "dead-hook".into(),
            config: IntegrationConfig::Webhook {
                // Unreachable port — will fail all retries
                url: "http://127.0.0.1:1/hook".into(),
                secret: None,
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    let event = test_event("testorg");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());
    NotificationConsumer::process_payload(&payload, store.as_ref(), &dispatcher)
        .await
        .unwrap();

    let deliveries = store.list_deliveries(&integration.id, 10).await.unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].status, DeliveryStatus::Failed);
    assert!(deliveries[0].error_message.is_some());
}

// ─── Integration tests: full JetStream publish → consume → dispatch ──
// These require NATS running on localhost:4223 (docker-compose).

async fn connect_nats() -> Option<async_nats::jetstream::Context> {
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4223".into());
    match async_nats::connect(&nats_url).await {
        Ok(client) => Some(async_nats::jetstream::new(client)),
        Err(_) => {
            eprintln!("NATS not available at {nats_url}, skipping integration test");
            None
        }
    }
}

/// Create a unique test stream to avoid interference between tests.
async fn create_test_stream(
    js: &async_nats::jetstream::Context,
    name: &str,
    subjects: &[String],
) -> async_nats::jetstream::stream::Stream {
    use async_nats::jetstream::stream;

    // Delete if exists from a previous test run
    let _ = js.delete_stream(name).await;

    js.create_stream(stream::Config {
        name: name.to_string(),
        subjects: subjects.to_vec(),
        retention: stream::RetentionPolicy::WorkQueue,
        max_age: Duration::from_secs(60),
        ..Default::default()
    })
    .await
    .expect("failed to create test stream")
}

#[tokio::test]
async fn jetstream_publish_and_consume_delivers_webhook() {
    let Some(js) = connect_nats().await else {
        return;
    };

    let (url, receiver) = start_receiver().await;
    let store = Arc::new(InMemoryIntegrationStore::new());

    store
        .create_integration(&CreateIntegrationInput {
            organisation: "js-org".into(),
            integration_type: IntegrationType::Webhook,
            name: "js-hook".into(),
            config: IntegrationConfig::Webhook {
                url,
                secret: Some("js-secret".into()),
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    // Create a unique stream for this test
    let stream_name = "TEST_NATS_DELIVER";
    let subject = "test.notifications.js-org.release_succeeded";
    let stream = create_test_stream(&js, stream_name, &[format!("test.notifications.>")]).await;

    // Publish an envelope
    let event = test_event("js-org");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    let ack = js
        .publish(subject, payload.into())
        .await
        .expect("publish failed");
    ack.await.expect("publish ack failed");

    // Create a consumer and pull the message
    use async_nats::jetstream::consumer;
    let consumer_name = "test-consumer-deliver";
    let pull_consumer = stream
        .create_consumer(consumer::pull::Config {
            durable_name: Some(consumer_name.to_string()),
            ack_wait: Duration::from_secs(30),
            ..Default::default()
        })
        .await
        .expect("create consumer failed");

    use futures_util::StreamExt;
    let mut messages = pull_consumer.messages().await.expect("messages failed");

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("timeout waiting for message")
        .expect("stream ended")
        .expect("message error");

    // Process through the consumer logic
    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());
    NotificationConsumer::process_payload(&msg.payload, store.as_ref(), &dispatcher)
        .await
        .unwrap();

    msg.ack().await.expect("ack failed");

    // Verify webhook was delivered
    let deliveries = receiver.deliveries.lock().unwrap();
    assert_eq!(deliveries.len(), 1, "webhook should receive the event");

    let d = &deliveries[0];
    assert!(d.signature.is_some(), "should be HMAC signed");

    let body: serde_json::Value = serde_json::from_str(&d.body).unwrap();
    assert_eq!(body["event"], "release_succeeded");
    assert_eq!(body["organisation"], "js-org");

    // Cleanup
    let _ = js.delete_stream(stream_name).await;
}

#[tokio::test]
async fn jetstream_multiple_messages_all_delivered() {
    let Some(js) = connect_nats().await else {
        return;
    };

    let (url, receiver) = start_receiver().await;
    let store = Arc::new(InMemoryIntegrationStore::new());

    store
        .create_integration(&CreateIntegrationInput {
            organisation: "multi-org".into(),
            integration_type: IntegrationType::Webhook,
            name: "multi-hook".into(),
            config: IntegrationConfig::Webhook {
                url,
                secret: None,
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    let stream_name = "TEST_NATS_MULTI";
    let stream = create_test_stream(&js, stream_name, &["test.multi.>".into()]).await;

    // Publish 3 events
    for i in 0..3 {
        let mut event = test_event("multi-org");
        event.id = format!("multi-{i}");
        let envelope = NotificationEnvelope::from(&event);
        let payload = serde_json::to_vec(&envelope).unwrap();
        let ack = js
            .publish(
                format!("test.multi.multi-org.release_succeeded"),
                payload.into(),
            )
            .await
            .unwrap();
        ack.await.unwrap();
    }

    // Consume all 3
    use async_nats::jetstream::consumer;
    use futures_util::StreamExt;

    let pull_consumer = stream
        .create_consumer(consumer::pull::Config {
            durable_name: Some("test-consumer-multi".to_string()),
            ack_wait: Duration::from_secs(30),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut messages = pull_consumer.messages().await.unwrap();
    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());

    for _ in 0..3 {
        let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("error");

        NotificationConsumer::process_payload(&msg.payload, store.as_ref(), &dispatcher)
            .await
            .unwrap();
        msg.ack().await.unwrap();
    }

    let deliveries = receiver.deliveries.lock().unwrap();
    assert_eq!(deliveries.len(), 3, "all 3 events should be delivered");

    // Verify each has a unique notification_id
    let ids: Vec<String> = deliveries
        .iter()
        .map(|d| {
            let v: serde_json::Value = serde_json::from_str(&d.body).unwrap();
            v["notification_id"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(ids.len(), 3);
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);

    let _ = js.delete_stream(stream_name).await;
}

#[tokio::test]
async fn jetstream_message_for_wrong_org_skips_dispatch() {
    let Some(js) = connect_nats().await else {
        return;
    };

    let (url, receiver) = start_receiver().await;
    let store = Arc::new(InMemoryIntegrationStore::new());

    // Integration for "org-a" only
    store
        .create_integration(&CreateIntegrationInput {
            organisation: "org-a".into(),
            integration_type: IntegrationType::Webhook,
            name: "org-a-hook".into(),
            config: IntegrationConfig::Webhook {
                url,
                secret: None,
                headers: HashMap::new(),
            },
            created_by: "user-1".into(),
        })
        .await
        .unwrap();

    let stream_name = "TEST_NATS_WRONG_ORG";
    let stream = create_test_stream(&js, stream_name, &["test.wrongorg.>".into()]).await;

    // Publish event for "org-b" (no integration)
    let event = test_event("org-b");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();
    let ack = js
        .publish("test.wrongorg.org-b.release_succeeded", payload.into())
        .await
        .unwrap();
    ack.await.unwrap();

    use async_nats::jetstream::consumer;
    use futures_util::StreamExt;

    let pull_consumer = stream
        .create_consumer(consumer::pull::Config {
            durable_name: Some("test-consumer-wrongorg".to_string()),
            ack_wait: Duration::from_secs(30),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut messages = pull_consumer.messages().await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let dispatcher = NotificationDispatcher::new(store.clone(), String::new());
    NotificationConsumer::process_payload(&msg.payload, store.as_ref(), &dispatcher)
        .await
        .unwrap();
    msg.ack().await.unwrap();

    // org-a's webhook should NOT have been called
    assert!(
        receiver.deliveries.lock().unwrap().is_empty(),
        "wrong org should not trigger delivery"
    );

    let _ = js.delete_stream(stream_name).await;
}

#[tokio::test]
async fn jetstream_stream_creation_is_idempotent() {
    let Some(js) = connect_nats().await else {
        return;
    };

    use async_nats::jetstream::stream;

    let stream_name = "TEST_NATS_IDEMPOTENT";
    let _ = js.delete_stream(stream_name).await;

    let config = stream::Config {
        name: stream_name.to_string(),
        subjects: vec!["test.idempotent.>".to_string()],
        retention: stream::RetentionPolicy::WorkQueue,
        max_age: Duration::from_secs(60),
        ..Default::default()
    };

    // Create twice — should not error
    js.get_or_create_stream(config.clone()).await.unwrap();
    js.get_or_create_stream(config).await.unwrap();

    let _ = js.delete_stream(stream_name).await;
}

#[tokio::test]
async fn jetstream_envelope_roundtrip_through_nats() {
    let Some(js) = connect_nats().await else {
        return;
    };

    let stream_name = "TEST_NATS_ROUNDTRIP";
    let stream = create_test_stream(&js, stream_name, &["test.roundtrip.>".into()]).await;

    // Publish an event with release context including error_message
    let event = failed_event("roundtrip-org");
    let envelope = NotificationEnvelope::from(&event);
    let payload = serde_json::to_vec(&envelope).unwrap();

    let ack = js
        .publish("test.roundtrip.roundtrip-org.release_failed", payload.into())
        .await
        .unwrap();
    ack.await.unwrap();

    use async_nats::jetstream::consumer;
    use futures_util::StreamExt;

    let pull_consumer = stream
        .create_consumer(consumer::pull::Config {
            durable_name: Some("test-consumer-roundtrip".to_string()),
            ack_wait: Duration::from_secs(30),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut messages = pull_consumer.messages().await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Deserialize and verify all fields survived the roundtrip
    let restored: NotificationEnvelope = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(restored.notification_type, "release_failed");
    assert_eq!(restored.organisation, "roundtrip-org");
    assert_eq!(restored.title, "Deploy v3.0 failed");

    let release = restored.release.unwrap();
    assert_eq!(release.error_message.as_deref(), Some("OOM killed"));
    assert_eq!(release.source_username, "bob");
    assert_eq!(release.commit_branch, "hotfix");

    msg.ack().await.unwrap();
    let _ = js.delete_stream(stream_name).await;
}
