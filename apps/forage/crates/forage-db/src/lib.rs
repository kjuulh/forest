mod integrations;
mod magic_link;
mod profile_pictures;
mod sessions;

pub use integrations::PgIntegrationStore;
pub use magic_link::PgMagicLinkStore;
pub use profile_pictures::{PgProfilePictureStore, ProfilePicture};
pub use sessions::PgSessionStore;
pub use sqlx::PgPool;

/// Run all pending migrations.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("src/migrations").run(pool).await
}
