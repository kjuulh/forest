use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct PreferencesCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// List current notification preferences
    List(ListPrefsCommand),
    /// Set a notification preference
    Set(SetPrefCommand),
}

impl PreferencesCommand {
    pub fn is_mutation(&self) -> bool {
        matches!(self.commands, Commands::Set(_))
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Set(cmd) => cmd.execute(state).await,
        }
    }
}

#[derive(clap::Parser)]
pub struct ListPrefsCommand;

impl ListPrefsCommand {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let client = state.grpc_client();

        let prefs = client.get_notification_preferences().await?;

        if prefs.is_empty() {
            eprintln!("No preferences set (all notifications enabled by default).");
            return Ok(());
        }

        for pref in &prefs {
            let ntype = format_notification_type(pref.notification_type());
            let channel = format_channel(pref.channel());
            let status = if pref.enabled { "enabled" } else { "disabled" };
            println!("{ntype} / {channel}: {status}");
        }

        Ok(())
    }
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum NotifType {
    Annotated,
    Started,
    Succeeded,
    Failed,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum NotifChannel {
    Cli,
    Slack,
}

#[derive(clap::Parser)]
pub struct SetPrefCommand {
    /// Notification type
    #[arg(long, value_enum)]
    r#type: NotifType,

    /// Notification channel
    #[arg(long, value_enum, default_value = "cli")]
    channel: NotifChannel,

    /// Enable or disable
    #[arg(long)]
    enabled: bool,
}

impl SetPrefCommand {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let client = state.grpc_client();

        let ntype = match self.r#type {
            NotifType::Annotated => forest_grpc_interface::NotificationType::ReleaseAnnotated,
            NotifType::Started => forest_grpc_interface::NotificationType::ReleaseStarted,
            NotifType::Succeeded => forest_grpc_interface::NotificationType::ReleaseSucceeded,
            NotifType::Failed => forest_grpc_interface::NotificationType::ReleaseFailed,
        };

        let channel = match self.channel {
            NotifChannel::Cli => forest_grpc_interface::NotificationChannel::Cli,
            NotifChannel::Slack => forest_grpc_interface::NotificationChannel::Slack,
        };

        let pref = client
            .set_notification_preference(ntype, channel, self.enabled)
            .await?;

        match pref {
            Some(p) => {
                let status = if p.enabled { "enabled" } else { "disabled" };
                eprintln!(
                    "Set {} / {}: {}",
                    format_notification_type(p.notification_type()),
                    format_channel(p.channel()),
                    status,
                );
            }
            None => eprintln!("Preference updated."),
        }

        Ok(())
    }
}

fn format_notification_type(t: forest_grpc_interface::NotificationType) -> &'static str {
    match t {
        forest_grpc_interface::NotificationType::ReleaseAnnotated => "ANNOTATED",
        forest_grpc_interface::NotificationType::ReleaseStarted => "STARTED",
        forest_grpc_interface::NotificationType::ReleaseSucceeded => "SUCCEEDED",
        forest_grpc_interface::NotificationType::ReleaseFailed => "FAILED",
        forest_grpc_interface::NotificationType::Unspecified => "UNKNOWN",
    }
}

fn format_channel(c: forest_grpc_interface::NotificationChannel) -> &'static str {
    match c {
        forest_grpc_interface::NotificationChannel::Cli => "CLI",
        forest_grpc_interface::NotificationChannel::Slack => "SLACK",
        forest_grpc_interface::NotificationChannel::Unspecified => "UNKNOWN",
    }
}
