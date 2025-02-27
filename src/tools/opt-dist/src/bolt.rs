use anyhow::Context;

use crate::exec::cmd;
use crate::training::LlvmBoltProfile;
use camino::{Utf8Path, Utf8PathBuf};

use crate::utils::io::copy_file;

/// Instruments an artifact at the given `path` (in-place) with BOLT and then calls `func`.
/// After this function finishes, the original file will be restored.
pub fn with_bolt_instrumented<F: FnOnce() -> anyhow::Result<R>, R>(
    path: &Utf8Path,
    func: F,
) -> anyhow::Result<R> {
    // Back up the original file.
    // It will be restored to its original state when this function exits.
    // By copying it, we break any existing hard links, so that they are not affected by the
    // instrumentation.
    let _backup_file = BackedUpFile::new(path)?;

    let instrumented_path = tempfile::NamedTempFile::new()?.into_temp_path();

    // Instrument the original file with BOLT, saving the result into `instrumented_path`
    cmd(&["llvm-bolt"])
        .arg("-instrument")
        .arg(path)
        // Make sure that each process will write its profiles into a separate file
        .arg("--instrumentation-file-append-pid")
        .arg("-o")
        .arg(instrumented_path.display())
        .run()
        .with_context(|| anyhow::anyhow!("Could not instrument {path} using BOLT"))?;

    // Copy the instrumented artifact over the original one
    copy_file(&instrumented_path, path)?;

    // Run the function that will make use of the instrumented artifact.
    // The original file will be restored when `_backup_file` is dropped.
    func()
}

/// Optimizes the file at `path` with BOLT in-place using the given `profile`.
pub fn bolt_optimize(path: &Utf8Path, profile: &LlvmBoltProfile) -> anyhow::Result<()> {
    // Copy the artifact to a new location, so that we do not use the same input and output file.
    // BOLT cannot handle optimizing when the input and output is the same file, because it performs
    // in-place patching.
    let temp_path = tempfile::NamedTempFile::new()?.into_temp_path();
    copy_file(path, &temp_path)?;

    cmd(&["llvm-bolt"])
        .arg(temp_path.display())
        .arg("-data")
        .arg(&profile.0)
        .arg("-o")
        .arg(path)
        // Reorder basic blocks within functions
        .arg("-reorder-blocks=ext-tsp")
        // Reorder functions within the binary
        .arg("-reorder-functions=hfsort+")
        // Split function code into hot and code regions
        .arg("-split-functions")
        // Split as many basic blocks as possible
        .arg("-split-all-cold")
        // Move jump tables to a separate section
        .arg("-jump-tables=move")
        // Fold functions with identical code
        .arg("-icf=1")
        // Try to reuse old text segments to reduce binary size
        .arg("--use-old-text")
        // Update DWARF debug info in the final binary
        .arg("-update-debug-sections")
        // Print optimization statistics
        .arg("-dyno-stats")
        .run()
        .with_context(|| anyhow::anyhow!("Could not optimize {path} with BOLT"))?;

    Ok(())
}

/// Copies a file to a temporary location and restores it (copies it back) when it is dropped.
pub struct BackedUpFile {
    original: Utf8PathBuf,
    backup: tempfile::TempPath,
}

impl BackedUpFile {
    pub fn new(file: &Utf8Path) -> anyhow::Result<Self> {
        let temp_path = tempfile::NamedTempFile::new()?.into_temp_path();
        copy_file(file, &temp_path)?;
        Ok(Self { backup: temp_path, original: file.to_path_buf() })
    }
}

impl Drop for BackedUpFile {
    fn drop(&mut self) {
        copy_file(&self.backup, &self.original).expect("Cannot restore backed up file");
    }
}
