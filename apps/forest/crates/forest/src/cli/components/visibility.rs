use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::ValueEnum, Clone, Copy)]
pub(crate) enum Visibility {
    /// Anyone signed in to this server can resolve and download it.
    Public,
    /// Only members of the owning organisation can.
    Private,
}

impl Visibility {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

#[derive(clap::Parser)]
pub struct VisibilityCommand {
    /// `<org>/<name>` reference.
    component: String,

    visibility: Visibility,
}

impl VisibilityCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let (org, name) = self
            .component
            .split_once('/')
            .filter(|(org, name)| !org.is_empty() && !name.is_empty() && !name.contains('/'))
            .ok_or_else(|| anyhow::anyhow!("expected <org>/<name>, got {:?}", self.component))?;

        let visibility = self.visibility.as_str();

        state
            .grpc_client()
            .set_project_visibility(org, name, visibility)
            .await?;

        println!("{org}/{name} is now {visibility}");
        Ok(())
    }
}
