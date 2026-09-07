#[derive(clap::Parser)]
#[command(author, version, about, long_about = None)]
pub struct Command {
    /// Replace application responses with maintenance-safe defaults.
    #[arg(long, env = "FORAGE_MAINTENANCE_MODE", default_value = "false")]
    pub maintenance_mode: bool,
}
