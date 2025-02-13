use std::path::Path;

use anyhow::Context;

pub async fn reconcile(source: &Path, dest: &Path) -> anyhow::Result<()> {
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(source)?;
        let metadata = entry.metadata()?;

        if metadata.is_file() {
            tracing::trace!("copying file: {}", rel.display());
            let dest_path = dest.join(rel);

            tokio::fs::copy(entry.path(), &dest_path)
                .await
                .context(anyhow::anyhow!(
                    "failed to file directory at: {}",
                    dest_path.display()
                ))?;
        } else if metadata.is_dir() {
            let dest_path = dest.join(rel);

            tracing::trace!("creating directory: {}", dest_path.display());
            tokio::fs::create_dir_all(&dest_path)
                .await
                .context(anyhow::anyhow!(
                    "failed to create directory at: {}",
                    dest_path.display()
                ))?;
        }
    }

    Ok(())
}
