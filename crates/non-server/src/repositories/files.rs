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

    pub async fn get_file(&self, component_id: &Uuid, page: usize) -> anyhow::Result<Option<File>> {
        let res = sqlx::query!(
            r#"
                SELECT
                    file_path,
                    file_content
                FROM component_files
                WHERE
                    component_id = $1

                ORDER BY file_path ASC
                LIMIT 1
                OFFSET $2
            "#,
            component_id,
            page as i64
        )
        .fetch_optional(&self.db)
        .await?;

        let rec = match res {
            Some(r) => r,
            None => return Ok(None),
        };

        Ok(Some(File {
            path: rec.file_path,
            content: rec.file_content,
        }))
    }
}

pub struct File {
    pub path: String,
    pub content: Vec<u8>,
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
