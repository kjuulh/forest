use std::path::PathBuf;

use crate::{grpc::GrpcClientState, state::State};

/// Read prepared files without preparing, annotating, approving or releasing anything.
///
/// Files are written as <output>/<category>/<server file_name>. The output's
/// parent must exist, and output itself must not exist, even as an empty
/// directory or symlink. All directories are private (0700), files are 0600.
/// Parent-directory references, symlinks, duplicate paths and file/directory
/// conflicts are refused. An I/O failure may leave an incomplete private tree;
/// retry with a new output directory rather than overwriting it.
///
/// Supported on Unix, where descriptor-relative no-follow operations protect
/// against symlink substitution. Other platforms refuse the export.
/// Output contains only paths, metadata, byte counts and SHA-256 hashes, never
/// file contents. Use --format json for an inventory suitable for review.
#[derive(clap::Parser)]
pub struct FilesCommand {
    /// Immutable artifact UUID, not a release-intent UUID or slug.
    #[arg(long)]
    artifact_id: uuid::Uuid,

    /// New private export directory. Its parent must already exist.
    #[arg(long, short = 'o')]
    output: PathBuf,

    /// Export only this category; omit to export every category.
    #[arg(long, value_enum)]
    category: Option<Category>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Category {
    Deployment,
    Spec,
    Attachment,
}

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Self::Deployment => "deployment",
            Self::Spec => "spec",
            Self::Attachment => "attachment",
        }
    }
}

impl FilesCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        #[cfg(not(unix))]
        {
            let _ = state;
            anyhow::bail!("secure artifact export is supported on Unix platforms only");
        }
        #[cfg(unix)]
        {
            let files = state
                .grpc_client()
                .get_artifact_files(&self.artifact_id, self.category.map(Category::as_str))
                .await?;
            let rows = export::files(&self.output, files)?;
            print!(
                "{}",
                crate::cli::output::render(&state.config.format, &rows)
            );
            eprintln!(
                "Exported {} file(s) from artifact {} to {}",
                rows.len(),
                self.artifact_id,
                self.output.display()
            );
            Ok(())
        }
    }
}

#[cfg(unix)]
mod export {
    use std::{
        collections::BTreeSet,
        io::Write,
        path::{Component, Path},
    };

    use anyhow::Context;
    use forest_grpc_interface::ArtifactFile;
    use rustix::{
        fd::OwnedFd,
        fs::{Mode, OFlags, fchmod, mkdirat, open, openat},
        io::Errno,
    };
    use serde::Serialize;
    use sha2::{Digest, Sha256};
    use tabled::Tabled;

    #[derive(Tabled, Serialize)]
    pub(super) struct ExportedFile {
        path: String,
        category: String,
        environment: String,
        destination: String,
        bytes: usize,
        sha256: String,
    }

    const DIR_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    fn safe_segment(segment: &str) -> bool {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !segment
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    }

    fn paths(files: &[ArtifactFile]) -> anyhow::Result<Vec<String>> {
        anyhow::ensure!(
            !files.is_empty(),
            "artifact/category has no files to export"
        );
        let mut paths = Vec::with_capacity(files.len());
        for file in files {
            anyhow::ensure!(
                safe_segment(&file.category) && file.file_name.split('/').all(safe_segment),
                "artifact contains an unsafe category or file path"
            );
            anyhow::ensure!(
                !file
                    .env
                    .chars()
                    .chain(file.destination.chars())
                    .any(char::is_control),
                "artifact contains control characters in file metadata"
            );
            paths.push(format!("{}/{}", file.category, file.file_name));
        }
        let mut unique = BTreeSet::new();
        for path in &paths {
            anyhow::ensure!(
                unique.insert(path.as_str()),
                "artifact contains duplicate export paths"
            );
        }
        for path in &paths {
            let mut ancestor = path.as_str();
            while let Some((parent, _)) = ancestor.rsplit_once('/') {
                anyhow::ensure!(
                    !unique.contains(parent),
                    "artifact contains conflicting file/directory paths"
                );
                ancestor = parent;
            }
        }
        Ok(paths)
    }

    fn output_directory(output: &Path) -> anyhow::Result<OwnedFd> {
        let absolute = if output.is_absolute() {
            output.to_path_buf()
        } else {
            std::env::current_dir()?.join(output)
        };
        let mut segments = Vec::new();
        for part in absolute.components() {
            match part {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(name) => segments.push(name),
                _ => anyhow::bail!("output path must not contain parent-directory references"),
            }
        }
        let (leaf, parents) = segments
            .split_last()
            .context("output must name a new directory")?;
        let mut directory = open("/", DIR_FLAGS, Mode::empty())?;
        for name in parents {
            directory = openat(&directory, *name, DIR_FLAGS, Mode::empty())
                .context("open output parent (symlinks are not allowed)")?;
        }
        let private = Mode::from_raw_mode(0o700);
        mkdirat(&directory, *leaf, private).context(
            "create new output directory (refusing existing files, directories or symlinks)",
        )?;
        let root = openat(&directory, *leaf, DIR_FLAGS, Mode::empty())?;
        fchmod(&root, private)?;
        Ok(root)
    }

    fn write_file(root: &OwnedFd, path: &str, content: &[u8]) -> anyhow::Result<()> {
        let mut directory = None;
        let mut segments = path.split('/').peekable();
        while let Some(segment) = segments.next() {
            let parent = directory.as_ref().unwrap_or(root);
            if segments.peek().is_some() {
                let private = Mode::from_raw_mode(0o700);
                match mkdirat(parent, segment, private) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(e) => return Err(e.into()),
                }
                let child = openat(parent, segment, DIR_FLAGS, Mode::empty())?;
                fchmod(&child, private)?;
                directory = Some(child);
            } else {
                let private = Mode::from_raw_mode(0o600);
                let fd = openat(
                    parent,
                    segment,
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    private,
                )?;
                fchmod(&fd, private)?;
                std::fs::File::from(fd).write_all(content)?;
            }
        }
        Ok(())
    }

    pub(super) fn files(
        output: &Path,
        mut files: Vec<ArtifactFile>,
    ) -> anyhow::Result<Vec<ExportedFile>> {
        files
            .sort_unstable_by(|a, b| (&a.category, &a.file_name).cmp(&(&b.category, &b.file_name)));
        // Validate the entire manifest before creating any destination files.
        let paths = paths(&files)?;
        let root = output_directory(output)?;
        let mut rows = Vec::with_capacity(files.len());
        for (file, path) in files.into_iter().zip(paths) {
            write_file(&root, &path, file.content.as_bytes()).with_context(|| {
                format!("export incomplete at {path}; retry with a new output directory")
            })?;
            rows.push(ExportedFile {
                path,
                category: file.category,
                environment: file.env,
                destination: file.destination,
                bytes: file.content.len(),
                sha256: hex::encode(Sha256::digest(file.content.as_bytes())),
            });
        }
        Ok(rows)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{
            fs,
            os::unix::fs::{PermissionsExt, symlink},
            path::PathBuf,
        };

        fn fixture() -> (tempfile::TempDir, PathBuf) {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            (temp, root)
        }

        fn file(category: &str, name: &str, content: &str) -> ArtifactFile {
            ArtifactFile {
                category: category.into(),
                file_name: name.into(),
                content: content.into(),
                env: "prod".into(),
                destination: "example".into(),
            }
        }

        #[test]
        fn exported_bytes_are_private_and_the_inventory_matches_the_saved_file() {
            let (_temp, root) = fixture();
            let output = root.join("export");
            let rows = files(
                &output,
                vec![file("deployment", "prod/config.yaml", "hello\n")],
            )
            .unwrap();
            assert_eq!(
                fs::read(output.join("deployment/prod/config.yaml")).unwrap(),
                b"hello\n"
            );
            assert_eq!(rows[0].bytes, 6);
            assert_eq!(
                rows[0].sha256,
                "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
            );
            for dir in [
                &output,
                &output.join("deployment"),
                &output.join("deployment/prod"),
            ] {
                assert_eq!(
                    fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
            assert_eq!(
                fs::metadata(output.join("deployment/prod/config.yaml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        #[test]
        fn unsafe_remote_names_never_create_an_export_tree() {
            let (_temp, root) = fixture();
            for name in [
                "",
                "/absolute",
                "../escape",
                "a/../../escape",
                "a/../b",
                "./file",
                "a//b",
                "a/",
                "C:/escape",
                "a\\escape",
                "line\nbreak",
                "zero\0byte",
            ] {
                let output = root.join("export");
                assert!(
                    files(&output, vec![file("deployment", name, "secret")]).is_err(),
                    "{name:?}"
                );
                assert!(!output.exists());
            }
            for category in ["", "..", "/root", "a/b", "C:", "control\u{1b}"] {
                assert!(
                    files(&root.join("export"), vec![file(category, "file", "secret")]).is_err()
                );
                assert!(!root.join("export").exists());
            }
        }

        #[test]
        fn duplicate_and_ancestor_conflicts_are_rejected_before_writing_any_files() {
            let (_temp, root) = fixture();
            for names in [
                ["same", "same"],
                ["parent", "parent/child"],
                ["parent/child", "parent"],
            ] {
                let output = root.join("export");
                assert!(
                    files(
                        &output,
                        names
                            .into_iter()
                            .map(|name| file("spec", name, "secret"))
                            .collect()
                    )
                    .is_err()
                );
                assert!(!output.exists());
            }
            assert!(files(&root.join("export"), Vec::new()).is_err());
            assert!(!root.join("export").exists());
        }

        #[test]
        fn existing_exports_and_other_existing_destinations_are_never_overwritten() {
            let (_temp, root) = fixture();
            let output = root.join("export");
            files(&output, vec![file("spec", "forest.cue", "original")]).unwrap();
            assert!(files(&output, vec![file("spec", "forest.cue", "replacement")]).is_err());
            assert_eq!(
                fs::read(output.join("spec/forest.cue")).unwrap(),
                b"original"
            );
            let empty = root.join("empty");
            fs::create_dir(&empty).unwrap();
            assert!(files(&empty, vec![file("spec", "file", "secret")]).is_err());
            assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
            let regular = root.join("regular");
            fs::write(&regular, "original").unwrap();
            assert!(files(&regular, vec![file("spec", "file", "secret")]).is_err());
            assert_eq!(fs::read(&regular).unwrap(), b"original");
        }

        #[test]
        fn parent_traversal_and_terminal_control_metadata_are_refused() {
            let (_temp, root) = fixture();
            assert!(
                files(
                    &root.join("missing/../escape"),
                    vec![file("spec", "file", "secret")]
                )
                .is_err()
            );
            assert!(!root.join("escape").exists());
            for field in ["env", "destination"] {
                let mut entry = file("spec", "file", "secret");
                if field == "env" {
                    entry.env = "\u{1b}[31m".into();
                } else {
                    entry.destination = "line\nbreak".into();
                }
                assert!(files(&root.join("export"), vec![entry]).is_err());
                assert!(!root.join("export").exists());
            }
        }

        #[test]
        fn symlinked_output_or_parent_is_not_followed() {
            let (_temp, root) = fixture();
            let outside = root.join("outside");
            fs::create_dir(&outside).unwrap();
            let link = root.join("link");
            symlink(&outside, &link).unwrap();
            assert!(files(&link, vec![file("spec", "file", "secret")]).is_err());
            assert!(files(&link.join("export"), vec![file("spec", "file", "secret")]).is_err());
            assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        }

        #[test]
        fn substituted_directory_and_file_symlinks_cannot_escape_or_replace_content() {
            let (_temp, root) = fixture();
            let outside = root.join("outside");
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("file"), "untouched").unwrap();
            let output = root.join("export");
            let fd = output_directory(&output).unwrap();
            symlink(&outside, output.join("deployment")).unwrap();
            assert!(write_file(&fd, "deployment/file", b"secret").is_err());
            fs::remove_file(output.join("deployment")).unwrap();
            fs::create_dir(output.join("deployment")).unwrap();
            symlink(outside.join("file"), output.join("deployment/file")).unwrap();
            assert!(write_file(&fd, "deployment/file", b"secret").is_err());
            assert_eq!(fs::read(outside.join("file")).unwrap(), b"untouched");
            fs::remove_file(output.join("deployment/file")).unwrap();
            fs::write(output.join("deployment/file"), "existing").unwrap();
            assert!(write_file(&fd, "deployment/file", b"replacement").is_err());
            assert_eq!(
                fs::read(output.join("deployment/file")).unwrap(),
                b"existing"
            );
            assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
        }
    }
}
