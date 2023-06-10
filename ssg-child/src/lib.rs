mod disk_caching_http_client;
mod file_spec;
pub mod sources;
pub mod target_error;
pub mod target_success;

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
pub use file_spec::FileSpec;
use futures::{future::BoxFuture, stream, FutureExt, Stream, StreamExt};
use relative_path::RelativePathBuf;
use sources::{bytes_with_file_spec_safety::Targets, FileSource};
use target_error::{TargetError, TargetErrorCause};
use target_success::TargetSuccess;
use tokio::{fs, io::AsyncWriteExt};

/// Panics on duplicate `FileSpec` targets
pub fn generate_static_site(
    output_dir: Utf8PathBuf,
    file_specs: impl IntoIterator<Item = FileSpec>,
) -> impl Stream<Item = Result<TargetSuccess, TargetError>> {
    let (paths, file_specs) = file_specs.into_iter().fold(
        (BTreeSet::<RelativePathBuf>::new(), Vec::<FileSpec>::new()),
        |(mut paths, mut file_specs), file_spec| {
            let newly_inserted = paths.insert(file_spec.target().clone());

            if !newly_inserted {
                panic!("Duplicate target: {}", file_spec.target());
            }

            file_specs.push(file_spec);

            (paths, file_specs)
        },
    );

    stream::iter(file_specs)
        .map(move |file_spec| {
            let target = file_spec.target().to_owned();

            generate_file_from_spec(
                file_spec.into_source(),
                paths.clone(),
                target,
                output_dir.clone(),
            )
        })
        .buffer_unordered(usize::MAX)
}

fn generate_file_from_spec(
    source: Box<dyn FileSource + Send>,
    targets: BTreeSet<RelativePathBuf>,
    this_target: RelativePathBuf,
    output_dir: Utf8PathBuf,
) -> BoxFuture<'static, Result<TargetSuccess, TargetError>> {
    async move {
        let targets = Targets::new(this_target.clone(), targets);
        let task = source.obtain_content(targets);

        let file_path = this_target.to_path(output_dir);

        fs::create_dir_all(file_path.parent().unwrap())
            .await
            .map_err(|error| {
                TargetError::new(this_target.clone(), TargetErrorCause::TargetIo(error))
            })?;

        let contents = task.await.map_err(|error| {
            TargetError::new(this_target.clone(), TargetErrorCause::Source(error))
        })?;

        let mut file_handle = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(file_path)
            .await
            .map_err(|error| {
                TargetError::new(this_target.clone(), TargetErrorCause::TargetIo(error))
            })?;

        file_handle.write_all(&contents).await.map_err(|error| {
            TargetError::new(this_target.clone(), TargetErrorCause::TargetIo(error))
        })?;

        Ok(TargetSuccess::new(this_target))
    }
    .boxed()
}
