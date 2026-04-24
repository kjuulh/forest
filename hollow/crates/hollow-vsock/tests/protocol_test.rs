//! Integration test for the vsock wire protocol.
//! Spins up a Unix socket listener, simulates the host→guest protocol flow,
//! and verifies messages are correctly serialized and deserialized.

use std::collections::HashMap;

use hollow_vsock::protocol::{CompletionMsg, JobDefinition, JobFile, LogLineMsg, Message};
use hollow_vsock::transport;

#[tokio::test]
async fn roundtrip_job_definition() {
    let (mut host_write, mut guest_read) = unix_socket_pair().await;

    let job = JobDefinition {
        job_id: "test-123".to_string(),
        command: vec!["echo".to_string(), "hello".to_string()],
        environment: HashMap::from([("FOO".to_string(), "bar".to_string())]),
        files: vec![JobFile {
            path: "main.tf".to_string(),
            content: b"resource {}".to_vec(),
            mode: 0o644,
        }],
        mode: "deploy".to_string(),
        timeout_seconds: 60,
    };

    transport::send_message(&mut host_write, &Message::JobDefinition(job.clone()))
        .await
        .unwrap();

    let received = transport::recv_message(&mut guest_read)
        .await
        .unwrap()
        .unwrap();
    match received {
        Message::JobDefinition(j) => {
            assert_eq!(j.job_id, "test-123");
            assert_eq!(j.command, vec!["echo", "hello"]);
            assert_eq!(j.environment.get("FOO").unwrap(), "bar");
            assert_eq!(j.files.len(), 1);
            assert_eq!(j.files[0].path, "main.tf");
            assert_eq!(j.files[0].content, b"resource {}");
            assert_eq!(j.mode, "deploy");
        }
        other => panic!("expected JobDefinition, got {:?}", other.message_type()),
    }
}

#[tokio::test]
async fn roundtrip_ready_and_heartbeat() {
    let (mut write, mut read) = unix_socket_pair().await;

    transport::send_message(&mut write, &Message::Ready)
        .await
        .unwrap();
    transport::send_message(&mut write, &Message::Heartbeat)
        .await
        .unwrap();

    assert!(matches!(
        transport::recv_message(&mut read).await.unwrap().unwrap(),
        Message::Ready
    ));
    assert!(matches!(
        transport::recv_message(&mut read).await.unwrap().unwrap(),
        Message::Heartbeat
    ));
}

#[tokio::test]
async fn roundtrip_log_and_completion() {
    let (mut write, mut read) = unix_socket_pair().await;

    transport::send_message(
        &mut write,
        &Message::LogLine(LogLineMsg {
            channel: "stdout".to_string(),
            line: "hello world".to_string(),
            timestamp: 12345,
        }),
    )
    .await
    .unwrap();

    transport::send_message(
        &mut write,
        &Message::Completion(CompletionMsg {
            exit_code: 0,
            plan_output: Some("plan output here".to_string()),
        }),
    )
    .await
    .unwrap();

    match transport::recv_message(&mut read).await.unwrap().unwrap() {
        Message::LogLine(l) => {
            assert_eq!(l.channel, "stdout");
            assert_eq!(l.line, "hello world");
            assert_eq!(l.timestamp, 12345);
        }
        other => panic!("expected LogLine, got {:?}", other.message_type()),
    }

    match transport::recv_message(&mut read).await.unwrap().unwrap() {
        Message::Completion(c) => {
            assert_eq!(c.exit_code, 0);
            assert_eq!(c.plan_output.unwrap(), "plan output here");
        }
        other => panic!("expected Completion, got {:?}", other.message_type()),
    }
}

#[tokio::test]
async fn eof_returns_none() {
    let (write, mut read) = unix_socket_pair().await;
    drop(write); // close the writer

    let msg = transport::recv_message(&mut read).await.unwrap();
    assert!(msg.is_none());
}

/// Create a connected pair of Unix streams for testing.
async fn unix_socket_pair() -> (
    tokio::io::WriteHalf<tokio::net::UnixStream>,
    tokio::io::BufReader<tokio::io::ReadHalf<tokio::net::UnixStream>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let sock_path = dir.path().join("test.sock");
    let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

    let connect = tokio::net::UnixStream::connect(&sock_path);
    let accept = listener.accept();

    let (client, server) = tokio::join!(connect, accept);
    let client = client.unwrap();
    let (server, _) = server.unwrap();

    let (_, write) = tokio::io::split(client);
    let (read, _) = tokio::io::split(server);

    (write, tokio::io::BufReader::new(read))
}
