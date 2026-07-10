//! Shared derivation-record and transactional publication primitives.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use hmx_core::hash::ContentHash;
use hmx_core::report::CheckResult;
use serde::Serialize;
use sha2::{Digest, Sha256 as Sha256Hasher};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub(crate) struct TestDirectory(PathBuf);

#[cfg(test)]
impl TestDirectory {
    pub(crate) fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "hmx-derive-{label}-{}-{}",
            std::process::id(),
            UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create unique test directory");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) struct CleanupGuard {
    paths: Vec<PathBuf>,
    armed: bool,
}

impl CleanupGuard {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            paths: vec![path],
            armed: true,
        }
    }

    fn add(&mut self, path: PathBuf) {
        self.paths.push(path);
    }

    fn remove(&mut self, path: &Path) {
        self.paths.retain(|candidate| candidate != path);
    }

    pub(crate) fn cleanup(&mut self) -> Result<()> {
        let mut failures = Vec::new();
        for path in self.paths.iter().rev() {
            let result = match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
                Ok(_) => fs::remove_file(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
        self.armed = false;
        if failures.is_empty() {
            Ok(())
        } else {
            bail!("cleanup failed: {}", failures.join("; "))
        }
    }

    pub(crate) fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.cleanup();
        }
    }
}

#[derive(Serialize)]
struct ContentHashRecord {
    algo: String,
    value: String,
}

impl From<&ContentHash> for ContentHashRecord {
    fn from(hash: &ContentHash) -> Self {
        Self {
            algo: hash.hash_algo().to_string(),
            value: hash.as_str().to_string(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ReplacementRecord {
    artifact_role: String,
    field_ids: Vec<String>,
    new_sha256: Option<String>,
    old_sha256: Option<String>,
}

impl ReplacementRecord {
    pub(crate) fn replacement(
        artifact_role: String,
        field_ids: Vec<String>,
        old_sha256: String,
        new_sha256: String,
    ) -> Self {
        Self::new(artifact_role, field_ids, Some(old_sha256), Some(new_sha256))
    }

    pub(crate) fn removal(
        artifact_role: String,
        field_ids: Vec<String>,
        old_sha256: String,
    ) -> Self {
        Self::new(artifact_role, field_ids, Some(old_sha256), None)
    }

    pub(crate) fn addition(
        artifact_role: String,
        field_ids: Vec<String>,
        new_sha256: String,
    ) -> Self {
        Self::new(artifact_role, field_ids, None, Some(new_sha256))
    }

    fn new(
        artifact_role: String,
        mut field_ids: Vec<String>,
        old_sha256: Option<String>,
        new_sha256: Option<String>,
    ) -> Self {
        field_ids.sort();
        field_ids.dedup();
        Self {
            artifact_role,
            field_ids,
            new_sha256,
            old_sha256,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct DerivationRecord {
    base_content_hash: ContentHashRecord,
    created_at: String,
    derived_content_hash: ContentHashRecord,
    derived_name: String,
    non_parameter_overrides: Vec<String>,
    replaced: Vec<ReplacementRecord>,
    tool_version: &'static str,
}

impl DerivationRecord {
    pub(crate) fn new(
        base_content_hash: &ContentHash,
        created_at: OffsetDateTime,
        derived_content_hash: &ContentHash,
        derived_name: String,
        non_parameter_overrides: Vec<String>,
        replaced: Vec<ReplacementRecord>,
    ) -> Result<Self> {
        let created_at = created_at
            .format(&Rfc3339)
            .context("formatting derivation record timestamp")?;
        Ok(Self::from_parts(
            ContentHashRecord::from(base_content_hash),
            created_at,
            ContentHashRecord::from(derived_content_hash),
            derived_name,
            non_parameter_overrides,
            replaced,
        ))
    }

    fn from_parts(
        base_content_hash: ContentHashRecord,
        created_at: String,
        derived_content_hash: ContentHashRecord,
        derived_name: String,
        non_parameter_overrides: Vec<String>,
        mut replaced: Vec<ReplacementRecord>,
    ) -> Self {
        let non_parameter_overrides = non_parameter_overrides
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        replaced.sort_by(|left, right| {
            (&left.artifact_role, &left.field_ids).cmp(&(&right.artifact_role, &right.field_ids))
        });
        Self {
            base_content_hash,
            created_at,
            derived_content_hash,
            derived_name,
            non_parameter_overrides,
            replaced,
            tool_version: env!("CARGO_PKG_VERSION"),
        }
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self).context("serializing derivation record")?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

pub(crate) fn validate_record_path(out: &Path, record: &Path) -> Result<()> {
    if record == out || record.starts_with(out) {
        bail!(
            "record path must be outside output package: {}",
            record.display()
        );
    }
    Ok(())
}

pub(crate) fn resolve_new_path(path: &Path, label: &str) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("{label} path must name a destination: {}", path.display()))?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let parent = fs::canonicalize(parent)
        .with_context(|| format!("resolving existing {label} parent {}", parent.display()))?;
    Ok(parent.join(file_name))
}

pub(crate) fn require_conformant(path: &Path, label: &str) -> Result<()> {
    let report = hmx_core::validate::validate(path)
        .with_context(|| format!("running full {label} validation at {}", path.display()))?;
    if report.conformant() {
        return Ok(());
    }
    let failures = report
        .checks()
        .iter()
        .filter(|check| check.result() == Some(CheckResult::Fail))
        .map(|check| {
            format!(
                "{}: {}",
                check.id().as_str(),
                check.detail().unwrap_or("no detail")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    bail!("{label} package is non-conformant: {failures}")
}

pub(crate) fn create_unique_directory(destination: &Path, kind: &str) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("destination lacks parent"))?;
    let name = destination
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("hmx"))
        .to_string_lossy();
    loop {
        let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{name}.hmx-{kind}-{}-{counter}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("creating {}", path.display()));
            }
        }
    }
}

fn create_unique_file(destination: &Path, kind: &str) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("destination lacks parent"))?;
    let name = destination
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("hmx"))
        .to_string_lossy();
    loop {
        let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{name}.hmx-{kind}-{}-{counter}",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("creating {}", path.display()));
            }
        }
    }
}

pub(crate) fn copy_regular(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("inspecting declared artifact {}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "replacement or declared file is not a regular file: {}",
            source.display()
        );
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "copying artifact {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

pub(crate) fn copy_declared(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("inspecting declared artifact {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "symlink declared artifact is forbidden: {}",
            source.display()
        );
    }
    if metadata.is_file() {
        return copy_regular(source, destination);
    }
    if !metadata.is_dir() {
        bail!("unsupported declared artifact type: {}", source.display());
    }
    fs::create_dir(destination)
        .with_context(|| format!("creating artifact directory {}", destination.display()))?;
    let mut entries = fs::read_dir(source)
        .with_context(|| format!("reading artifact directory {}", source.display()))?
        .map(|entry| entry.map(|value| (value.file_name(), value.path())))
        .collect::<std::io::Result<Vec<(OsString, PathBuf)>>>()?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, child) in entries {
        copy_declared(&child, &destination.join(name))?;
    }
    Ok(())
}

pub(crate) fn digest_file(path: &Path) -> Result<(String, u64)> {
    let mut file =
        File::open(path).with_context(|| format!("opening {} for hashing", path.display()))?;
    let mut hasher = Sha256Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("hashing {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let size = file
        .metadata()
        .with_context(|| format!("reading size for {}", path.display()))?
        .len();
    Ok((format!("{:x}", hasher.finalize()), size))
}

pub(crate) fn publish_staged(
    staging: &Path,
    out: &Path,
    record_path: Option<&Path>,
    record_bytes: &[u8],
    guard: &mut CleanupGuard,
) -> Result<()> {
    let record_temp = if let Some(path) = record_path {
        let temp = create_unique_file(path, "record")?;
        guard.add(temp.clone());
        let mut file = OpenOptions::new()
            .write(true)
            .open(&temp)
            .with_context(|| format!("opening staged record {}", temp.display()))?;
        file.write_all(record_bytes)
            .with_context(|| format!("writing staged record {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing staged record {}", temp.display()))?;
        Some(temp)
    } else {
        None
    };

    if let (Some(temp), Some(final_path)) = (&record_temp, record_path) {
        if final_path.exists() {
            bail!(
                "record path appeared before publication: {}",
                final_path.display()
            );
        }
        fs::rename(temp, final_path).with_context(|| {
            format!(
                "publishing record {} to {}",
                temp.display(),
                final_path.display()
            )
        })?;
        guard.remove(temp);
        guard.add(final_path.to_path_buf());
    }
    if out.exists() {
        bail!("output path appeared before publication: {}", out.display());
    }
    if let Err(error) = fs::rename(staging, out) {
        return Err(anyhow!(error)).with_context(|| {
            format!(
                "publishing staged package {} to {}",
                staging.display(),
                out.display()
            )
        });
    }
    guard.remove(staging);
    guard.add(out.to_path_buf());
    guard.remove(out);
    if let Some(path) = record_path {
        guard.remove(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{
        ContentHashRecord, DerivationRecord, ReplacementRecord, TestDirectory, copy_declared,
        resolve_new_path, validate_record_path,
    };

    #[test]
    fn record_serialization_has_exact_nested_order_and_one_newline() {
        let record = DerivationRecord::from_parts(
            ContentHashRecord {
                algo: "sha256".into(),
                value: "base".into(),
            },
            "2026-07-10T00:00:00Z".into(),
            ContentHashRecord {
                algo: "sha256".into(),
                value: "derived".into(),
            },
            "name".into(),
            vec!["z".into(), "a".into(), "z".into()],
            vec![ReplacementRecord::replacement(
                "parameter.scalars".into(),
                vec!["z".into(), "a".into(), "z".into()],
                "old".into(),
                "new".into(),
            )],
        );
        let bytes = record.to_bytes().expect("serialize record");
        assert_eq!(
            std::str::from_utf8(&bytes).expect("record is UTF-8"),
            concat!(
                "{\"base_content_hash\":{\"algo\":\"sha256\",\"value\":\"base\"},",
                "\"created_at\":\"2026-07-10T00:00:00Z\",",
                "\"derived_content_hash\":{\"algo\":\"sha256\",\"value\":\"derived\"},",
                "\"derived_name\":\"name\",\"non_parameter_overrides\":[\"a\",\"z\"],",
                "\"replaced\":[{\"artifact_role\":\"parameter.scalars\",",
                "\"field_ids\":[\"a\",\"z\"],\"new_sha256\":\"new\",\"old_sha256\":\"old\"}],",
                "\"tool_version\":\"",
                env!("CARGO_PKG_VERSION"),
                "\"}\n"
            )
        );
    }

    #[test]
    fn removal_and_addition_emit_explicit_nulls_in_deterministic_order() {
        let record = DerivationRecord::from_parts(
            ContentHashRecord {
                algo: "sha256".into(),
                value: "base".into(),
            },
            "2026-07-10T00:00:00Z".into(),
            ContentHashRecord {
                algo: "sha256".into(),
                value: "derived".into(),
            },
            "name".into(),
            Vec::new(),
            vec![
                ReplacementRecord::removal(
                    "z.removed".into(),
                    vec!["z".into(), "a".into(), "a".into()],
                    "old".into(),
                ),
                ReplacementRecord::addition("a.added".into(), vec!["field".into()], "new".into()),
            ],
        );
        let bytes = record.to_bytes().expect("serialize record");
        let text = std::str::from_utf8(&bytes).expect("record is UTF-8");
        assert!(text.contains(
            "\"replaced\":[{\"artifact_role\":\"a.added\",\"field_ids\":[\"field\"],\"new_sha256\":\"new\",\"old_sha256\":null},{\"artifact_role\":\"z.removed\",\"field_ids\":[\"a\",\"z\"],\"new_sha256\":null,\"old_sha256\":\"old\"}]"
        ));
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes[..bytes.len() - 1].ends_with(b"\n"));
    }

    #[test]
    fn recursive_copy_preserves_files_empty_directories_and_rejects_symlinks() {
        let root = TestDirectory::new("copy");
        let root = root.path();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested/empty")).expect("create source tree");
        fs::write(source.join("nested/file"), b"exact bytes").expect("write source file");
        copy_declared(&source, &destination).expect("copy tree");
        assert_eq!(
            fs::read(destination.join("nested/file")).expect("read copy"),
            b"exact bytes"
        );
        assert!(destination.join("nested/empty").is_dir());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(source.join("nested/file"), source.join("link"))
                .expect("create symlink");
            let second = root.join("second");
            assert!(copy_declared(&source, &second).is_err());
        }
    }

    #[test]
    fn resolved_path_safety_handles_dotdot_and_symlinked_ancestors() {
        let root = TestDirectory::new("paths");
        let root = root.path();
        let out = root.join("out");
        fs::create_dir(&out).expect("create prospective output root");
        fs::create_dir(out.join("nested")).expect("create nested directory");
        let resolved_out = resolve_new_path(&out, "output").expect("resolve output");
        let equal = resolve_new_path(&out, "record").expect("resolve equal record");
        let direct = resolve_new_path(&out.join("nested/record.json"), "record")
            .expect("resolve direct descendant");
        let normalized = resolve_new_path(&out.join("nested/../record.json"), "record")
            .expect("resolve normalized descendant");
        assert!(validate_record_path(&resolved_out, &equal).is_err());
        assert!(validate_record_path(&resolved_out, &direct).is_err());
        assert!(validate_record_path(&resolved_out, &normalized).is_err());
        #[cfg(unix)]
        {
            let link = root.join("linked-out");
            std::os::unix::fs::symlink(&out, &link).expect("create output symlink");
            let via_link = resolve_new_path(&link.join("nested/record.json"), "record")
                .expect("resolve symlink descendant");
            assert!(validate_record_path(&resolved_out, &via_link).is_err());
        }
        let sibling =
            resolve_new_path(&root.join("record.json"), "record").expect("resolve sibling record");
        validate_record_path(&resolved_out, &sibling).expect("sibling record is safe");
    }

    #[test]
    fn resolves_bare_relative_destination_in_current_directory() {
        let resolved =
            resolve_new_path(Path::new("derived"), "output").expect("resolve bare destination");
        assert_eq!(
            resolved,
            std::env::current_dir()
                .expect("read current directory")
                .join("derived")
        );
    }
}
