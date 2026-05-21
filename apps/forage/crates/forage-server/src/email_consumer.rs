use std::time::Duration;

use async_nats::jetstream;
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

/// SMTP configuration from environment variables.
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
        // Treat unset and empty-string the same: the infra layer wires
        // forage's SMTP slots through a console-managed secret where
        // unpopulated keys arrive as "". Without this guard a blank
        // SMTP_HOST would let SmtpConfig build, the email consumer
        // would start, and the first send would fail at the TCP layer.
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

/// Background component that pulls email jobs from NATS JetStream
/// and sends them via SMTP.
pub struct EmailConsumer {
    pub jetstream: jetstream::Context,
    pub smtp_config: SmtpConfig,
}

impl Component for EmailConsumer {
    fn info(&self) -> ComponentInfo {
        "forage/email-consumer".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        // Ensure the stream exists.
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
                max_age: Duration::from_secs(24 * 3600), // 24 hours
                max_bytes: 104_857_600, // 100 MB
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
            // Plain SMTP without TLS (for dev tools like Mailpit)
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

                    match self.send_email(&msg.payload, &transport).await {
                        Ok(()) => {
                            if let Err(e) = msg.ack().await {
                                tracing::warn!(error = %e, "failed to ack email message");
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "failed to send email, nacking");
                            if let Err(e) = msg.ack_with(async_nats::jetstream::AckKind::Nak(
                                Some(Duration::from_secs(30)),
                            )).await {
                                tracing::warn!(error = %e, "failed to nak email message");
                            }
                        }
                    }
                }
            }
        }
    }

    async fn send_email(
        &self,
        payload: &[u8],
        transport: &AsyncSmtpTransport<Tokio1Executor>,
    ) -> Result<(), String> {
        let envelope: EmailEnvelope =
            serde_json::from_slice(payload).map_err(|e| format!("deserialize email: {e}"))?;

        tracing::info!(to = %envelope.to, email_type = %envelope.email_type, "sending email");

        let message = Message::builder()
            .from(
                self.smtp_config
                    .from_address
                    .parse()
                    .map_err(|e| format!("invalid from address: {e}"))?,
            )
            .to(envelope
                .to
                .parse()
                .map_err(|e| format!("invalid to address: {e}"))?)
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
            .map_err(|e| format!("build email message: {e}"))?;

        transport
            .send(message)
            .await
            .map_err(|e| format!("SMTP send failed: {e}"))?;

        tracing::info!(to = %envelope.to, "email sent successfully");
        Ok(())
    }
}
