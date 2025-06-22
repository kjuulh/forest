use sqlx::PgPool;
use uuid::Uuid;

use crate::state::State;

pub struct FilesRepository {
    db: PgPool,
}

impl FilesRepository {
    pub async fn upload(
        &self,
        component_id: &Uuid,
        file_path: &str,
        file_content: &[u8],
    ) -> anyhow::Result<()> {
        sqlx::query!(
            r#"
                    INSERT INTO component_files (
                        component_id,
                        file_path,
                        file_content
                    )
                    VALUES (
                        $1,
                        $2,
                        $3
                    )
                "#,
            component_id,
            file_path,
            file_content,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }
}

pub trait FilesRepositoryState {
    fn files_repository(&self) -> FilesRepository;
}

impl FilesRepositoryState for State {
    fn files_repository(&self) -> FilesRepository {
        FilesRepository {
            db: self.db.clone(),
        }
    }
}
