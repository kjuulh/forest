use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use rusty_s3::{Bucket, Credentials, S3Action};

use crate::state::SharedState;

#[derive(Parser)]
#[command(author, version, about, long_about = None, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Serve {
        #[arg(env = "FOREST_HOST", long, default_value = "127.0.0.1:3000")]
        host: SocketAddr,

        #[arg(env = "FOREST_S3_ENDPOINT", long = "s3-endpoint")]
        s3_endpoint: String,

        #[arg(env = "FOREST_S3_REGION", long = "s3-region")]
        s3_region: String,

        #[arg(env = "FOREST_S3_BUCKET", long = "s3-bucket")]
        s3_bucket: String,

        #[arg(env = "FOREST_S3_USER", long = "s3-user")]
        s3_user: String,

        #[arg(env = "FOREST_S3_PASSWORD", long = "s3-password")]
        s3_password: String,
    },
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();

    if let Some(Commands::Serve {
        host,
        s3_endpoint,
        s3_bucket,
        s3_region,
        s3_user,
        s3_password,
    }) = cli.command
    {
        tracing::info!("Starting server");

        let creds = Credentials::new(s3_user, s3_password);
        let bucket = Bucket::new(
            url::Url::parse(&s3_endpoint)?,
            rusty_s3::UrlStyle::Path,
            s3_bucket,
            s3_region,
        )?;

        let put_object = bucket.put_object(Some(&creds), "some-object");
        let _url = put_object.sign(std::time::Duration::from_secs(30));

        let _state = SharedState::new().await?;
    }

    Ok(())
}
