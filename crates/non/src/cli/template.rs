use crate::state::State;

#[derive(clap::Parser)]
pub struct TemplateCommand {}

impl TemplateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // Schedule garbage collection of old templates in temp dir

        // Sync local components, make sure everything is where it needs to be
        // Run template(s) needed
        // Output to tmp destination

        Ok(())
    }
}
