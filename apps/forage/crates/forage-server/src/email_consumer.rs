use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::AckKind;
use async_nats::jetstream::consumer::PullConsumer;
use async_nats::jetstream::stream;
use forage_core::integrations::email::{
    EmailEnvelope, EMAIL_CONSUMER_NAME, EMAIL_STREAM_NAME, EMAIL_STREAM_SUBJECTS,
};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: String,
    pub use_tls: bool,
}

impl SmtpConfig {
    pub fn from_env() -> Option<Self> {
        let nonempty = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());

        let host = nonempty("SMTP_HOST")?;
        let port = nonempty("SMTP_PORT")
            .and_then(|p| p.parse().ok())
            .unwrap_or(587);
        let username = nonempty("SMTP_USERNAME");
        let password = nonempty("SMTP_PASSWORD");
        let from_address = nonempty("SMTP_FROM").unwrap_or_else(|| "noreply@forage.dev".into());
        let use_tls = nonempty("SMTP_TLS")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);
        Some(Self {
            host,
            port,
            username,
            password,
            from_address,
            use_tls,
        })
    }
}

pub struct EmailConsumer {
    pub jetstream: jetstream::Context,
    pub smtp_config: SmtpConfig,
}

impl Component for EmailConsumer {
    fn info(&self) -> ComponentInfo {
        "forage/email-consumer".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        self.ensure_stream()
            .await
            .map_err(|e| MadError::Inner(anyhow::anyhow!(e)))?;

        let mut backoff = 1u64;

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    tracing::info!("email consumer shutting down");
                    break;
                }
                result = self.consume_loop(&cancellation_token) => {
                    match result {
                        Ok(()) => {
                            backoff = 1;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, backoff_secs = backoff, "email consumer error, reconnecting");
                        }
                    }

                    tokio::select! {
                        _ = cancellation_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                    }
                    backoff = (backoff * 2).min(60);
                }
            }
        }

        Ok(())
    }
}

impl EmailConsumer {
    async fn ensure_stream(&self) -> Result<(), String> {
        self.jetstream
            .get_or_create_stream(stream::Config {
                name: EMAIL_STREAM_NAME.to_string(),
                subjects: vec![EMAIL_STREAM_SUBJECTS.to_string()],
                retention: stream::RetentionPolicy::WorkQueue,
                max_age: Duration::from_secs(24 * 3600),
                max_bytes: 104_857_600,
                discard: stream::DiscardPolicy::Old,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("create email stream: {e}"))?;
        Ok(())
    }

    async fn get_or_create_consumer(&self) -> Result<PullConsumer, String> {
        use async_nats::jetstream::consumer;

        let stream = self
            .jetstream
            .get_stream(EMAIL_STREAM_NAME)
            .await
            .map_err(|e| format!("get stream: {e}"))?;

        stream
            .get_or_create_consumer(
                EMAIL_CONSUMER_NAME,
                consumer::pull::Config {
                    durable_name: Some(EMAIL_CONSUMER_NAME.to_string()),
                    ack_wait: Duration::from_secs(60),
                    max_deliver: 10,
                    max_ack_pending: 50,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| format!("create consumer: {e}"))
    }

    fn build_transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
        if self.smtp_config.use_tls {
            let mut builder =
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.smtp_config.host)
                    .map_err(|e| format!("SMTP relay error: {e}"))?
                    .port(self.smtp_config.port);

            if let (Some(user), Some(pass)) = (
                self.smtp_config.username.as_deref(),
                self.smtp_config.password.as_deref(),
            ) {
                builder =
                    builder.credentials(Credentials::new(user.to_string(), pass.to_string()));
            }

            Ok(builder.build())
        } else {
            Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.smtp_config.host)
                .port(self.smtp_config.port)
                .build())
        }
    }

    async fn consume_loop(&self, cancellation_token: &CancellationToken) -> Result<(), String> {
        use futures_util::StreamExt;

        let consumer = self.get_or_create_consumer().await?;
        let transport = self.build_transport()?;
        let mut messages = consumer
            .messages()
            .await
            .map_err(|e| format!("consumer messages: {e}"))?;

        tracing::info!(consumer = EMAIL_CONSUMER_NAME, "pulling email jobs from JetStream");

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    return Ok(());
                }
                msg = messages.next() => {
                    let Some(msg) = msg else {
                        return Ok(());
                    };
                    let msg = msg.map_err(|e| format!("message error: {e}"))?;

                    let sent = Self::send_email(
                        &transport,
                        &self.smtp_config.from_address,
                        &msg.payload,
                    )
                    .await;
                    let ack = match sent {
                        Ok(()) => AckKind::Ack,
                        Err(failure) => {
                            failure.log();
                            ack_kind(&failure)
                        }
                    };
                    if let Err(e) = msg.ack_with(ack).await {
                        tracing::warn!(error = %e, ?ack, "failed to acknowledge email message");
                    }
                }
            }
        }
    }

    async fn send_email(
        transport: &AsyncSmtpTransport<Tokio1Executor>,
        from_address: &str,
        payload: &[u8],
    ) -> Result<(), SendFailure> {
        let envelope: EmailEnvelope = serde_json::from_slice(payload)
            .map_err(|e| SendFailure::Permanent(format!("deserialize email: {e}")))?;

        tracing::info!(to = %envelope.to, email_type = %envelope.email_type, "sending email");

        let from: lettre::message::Mailbox = from_address
            .parse()
            .map_err(|e| SendFailure::Transient(format!("invalid from address: {e}")))?;
        let sender = from.email.to_string();

        let message = Message::builder()
            .from(from)
            .to(envelope
                .to
                .parse()
                .map_err(|e| SendFailure::Permanent(format!("invalid to address: {e}")))?)
            .subject(&envelope.subject)
            .multipart(
                lettre::message::MultiPart::alternative()
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(envelope.body_text),
                    )
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(envelope.body_html),
                    ),
            )
            .map_err(|e| SendFailure::Permanent(format!("build email message: {e}")))?;

        transport.send(message).await.map_err(|e| {
            let message = format!("SMTP send failed: {e}");
            if refuses_only_this_recipient(&e, &envelope.to, &sender) {
                SendFailure::Permanent(message)
            } else {
                SendFailure::Transient(message)
            }
        })?;

        tracing::info!(to = %envelope.to, "email sent successfully");
        Ok(())
    }
}

/// A 5xx can also mean our credentials or sender identity are wrong, and
/// dropping on those would lose every email until someone fixes the config.
/// Only a rejection that names the recipient (SES: "Email address is not
/// verified … <recipient>") is about this one email.
fn refuses_only_this_recipient(
    error: &lettre::transport::smtp::Error,
    recipient: &str,
    sender: &str,
) -> bool {
    let reply = error.to_string().to_ascii_lowercase();
    error.is_permanent()
        && reply.contains(&recipient.to_ascii_lowercase())
        && !reply.contains(&sender.to_ascii_lowercase())
}

#[derive(Debug)]
enum SendFailure {
    /// Redelivering gives the same answer: a malformed job, or a recipient
    /// the relay refuses.
    Permanent(String),
    Transient(String),
}

impl SendFailure {
    fn log(&self) {
        match self {
            SendFailure::Permanent(e) => {
                tracing::warn!(error = %e, "email cannot be sent, dropping it")
            }
            SendFailure::Transient(e) => {
                tracing::error!(error = %e, "failed to send email, retrying in 30s")
            }
        }
    }
}

fn ack_kind(failure: &SendFailure) -> AckKind {
    match failure {
        SendFailure::Permanent(_) => AckKind::Term,
        SendFailure::Transient(_) => AckKind::Nak(Some(Duration::from_secs(30))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// An SMTP relay that answers MAIL FROM with `mail_reply` and RCPT TO with
    /// `rcpt_reply`, the two places a relay refuses the sender or a recipient.
    async fn relay(mail_reply: &'static str, rcpt_reply: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            write.write_all(b"220 relay ready\r\n").await.unwrap();
            while let Ok(Some(line)) = lines.next_line().await {
                let command = line.to_ascii_uppercase();
                let reply = if command.starts_with("EHLO") {
                    "250 relay\r\n"
                } else if command.starts_with("MAIL") {
                    mail_reply
                } else if command.starts_with("RCPT") {
                    rcpt_reply
                } else if command.starts_with("QUIT") {
                    "221 bye\r\n"
                } else {
                    "250 ok\r\n"
                };
                if write.write_all(reply.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
        port
    }

    fn transport(port: u16) -> AsyncSmtpTransport<Tokio1Executor> {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1")
            .port(port)
            .build()
    }

    fn job(to: &str) -> Vec<u8> {
        serde_json::to_vec(&EmailEnvelope {
            to: to.into(),
            subject: "Verify your email".into(),
            body_html: "<p>hi</p>".into(),
            body_text: "hi".into(),
            email_type: "verification".into(),
        })
        .unwrap()
    }

    const OK: &str = "250 ok\r\n";

    async fn send(mail_reply: &'static str, rcpt_reply: &'static str) -> SendFailure {
        send_from("noreply@forage.dev", mail_reply, rcpt_reply).await
    }

    async fn send_from(
        from: &str,
        mail_reply: &'static str,
        rcpt_reply: &'static str,
    ) -> SendFailure {
        let port = relay(mail_reply, rcpt_reply).await;
        EmailConsumer::send_email(&transport(port), from, &job("someone@example.com"))
            .await
            .expect_err("the relay refuses the email")
    }

    fn assert_redelivered(failure: &SendFailure, code: &str) {
        let SendFailure::Transient(message) = failure else {
            panic!("expected a transient failure, got {failure:?}");
        };
        assert!(message.contains(code), "{message}");
        assert!(matches!(ack_kind(failure), AckKind::Nak(Some(_))));
    }

    #[tokio::test]
    async fn a_recipient_the_relay_rejects_is_dropped_not_redelivered() {
        let failure = send(
            OK,
            "554 Message rejected: Email address is not verified. The following identities failed the check in region EU-WEST-1: someone@example.com\r\n",
        )
        .await;

        assert!(matches!(failure, SendFailure::Permanent(_)), "{failure:?}");
        assert!(matches!(ack_kind(&failure), AckKind::Term));
    }

    #[tokio::test]
    async fn an_unverified_sender_is_redelivered_until_the_config_is_fixed() {
        let failure = send(
            OK,
            "554 Message rejected: Email address is not verified. The following identities failed the check in region EU-WEST-1: noreply@forage.dev, someone@example.com\r\n",
        )
        .await;

        assert_redelivered(&failure, "554");
    }

    #[tokio::test]
    async fn an_unverified_sender_with_a_display_name_is_redelivered() {
        let failure = send_from(
            "Forage <noreply@forage.dev>",
            OK,
            "554 Message rejected: Email address is not verified. The following identities failed the check in region EU-WEST-1: noreply@forage.dev, someone@example.com\r\n",
        )
        .await;

        assert_redelivered(&failure, "554");
    }

    #[tokio::test]
    async fn an_unparseable_sender_is_redelivered_until_the_config_is_fixed() {
        let failure = EmailConsumer::send_email(
            &transport(1),
            "Forage noreply@forage.dev",
            &job("someone@example.com"),
        )
        .await
        .unwrap_err();

        assert_redelivered(&failure, "from address");
    }

    #[tokio::test]
    async fn a_relay_refusing_our_session_is_redelivered() {
        let failure = send("530 Authentication required\r\n", OK).await;

        assert_redelivered(&failure, "530");
    }

    #[tokio::test]
    async fn a_relay_asking_to_try_later_is_redelivered() {
        let failure = send(OK, "451 Temporary service failure\r\n").await;

        assert_redelivered(&failure, "451");
    }

    #[tokio::test]
    async fn a_job_that_is_not_an_email_is_dropped() {
        let failure = EmailConsumer::send_email(&transport(1), "noreply@forage.dev", b"not json")
            .await
            .unwrap_err();

        assert!(matches!(ack_kind(&failure), AckKind::Term), "{failure:?}");
    }

    #[tokio::test]
    async fn an_unparseable_recipient_is_dropped() {
        let failure =
            EmailConsumer::send_email(&transport(1), "noreply@forage.dev", &job("not an address"))
                .await
                .unwrap_err();

        assert!(matches!(ack_kind(&failure), AckKind::Term), "{failure:?}");
    }
}
