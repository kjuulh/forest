use sqlx::PgPool;
use uuid::Uuid;

pub struct PgProfilePictureStore {
    pool: PgPool,
}

pub struct ProfilePicture {
    pub content_type: String,
    pub data: Vec<u8>,
}

impl PgProfilePictureStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn upsert(
        &self,
        user_id: &str,
        content_type: &str,
        data: &[u8],
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO profile_pictures (id, user_id, content_type, data)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (user_id) DO UPDATE
            SET content_type = EXCLUDED.content_type,
                data = EXCLUDED.data
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(content_type)
        .bind(data)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, user_id: &str) -> anyhow::Result<Option<ProfilePicture>> {
        let row: Option<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT content_type, data FROM profile_pictures WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(content_type, data)| ProfilePicture { content_type, data }))
    }

    pub async fn delete(&self, user_id: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM profile_pictures WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
