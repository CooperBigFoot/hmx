//! Transactional standalone package derivation.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use hmx_core::manifest::{self, Manifest};
use hmx_core::readers::cog_reader::read_cog_metadata;
use hmx_core::readers::control_plane::read_domain_attributes;
use hmx_core::readers::parameter_scalars_reader::read_parameter_scalars;
use hmx_core::registry::{FieldRegistry, FieldSpec};
use hmx_core::report::CheckResult;
use hmx_core::types::{ArtifactFormat, Extent, FieldId, SemanticRole, Sha256};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256 as Sha256Hasher};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::warn;

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct DeriveRequest {
    base: PathBuf,
    out: PathBuf,
    name: String,
    replacements: Vec<ReplaceMutation>,
    sets: Vec<SetMutation>,
    non_parameter_policy: NonParameterPolicy,
    record: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonParameterPolicy {
    ParameterOnly,
    Allow,
}

impl From<bool> for NonParameterPolicy {
    fn from(allow: bool) -> Self {
        if allow {
            Self::Allow
        } else {
            Self::ParameterOnly
        }
    }
}

#[derive(Debug)]
struct ReplaceMutation {
    field_id: FieldId,
    source: PathBuf,
}

#[derive(Debug)]
struct SetMutation {
    field_id: FieldId,
    value: Value,
}

#[derive(Debug, Clone)]
enum ResolvedSource {
    Scalar { artifact_index: usize },
    Physical { artifact_index: usize },
}

struct ReplacementPlan {
    artifact_index: usize,
    affected_ids: Vec<FieldId>,
    old_sha256: String,
    replacement_source: PathBuf,
    non_parameter_ids: Vec<FieldId>,
}

struct ScalarRewritePlan {
    artifact_index: usize,
    values: BTreeMap<String, Value>,
    changed_ids: Vec<FieldId>,
}

struct CleanupGuard {
    paths: Vec<PathBuf>,
    armed: bool,
}

impl CleanupGuard {
    fn new(path: PathBuf) -> Self {
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

    fn cleanup(&mut self) -> Result<()> {
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

    fn disarm(mut self) {
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

#[derive(Serialize)]
struct ReplacementRecord {
    artifact_role: String,
    field_ids: Vec<String>,
    new_sha256: String,
    old_sha256: String,
}

#[derive(Serialize)]
struct DerivationRecord {
    base_content_hash: ContentHashRecord,
    created_at: String,
    derived_content_hash: ContentHashRecord,
    derived_name: String,
    non_parameter_overrides: Vec<String>,
    replaced: Vec<ReplacementRecord>,
    tool_version: &'static str,
}

impl DeriveRequest {
    pub(crate) fn new(
        base: PathBuf,
        out: PathBuf,
        name: String,
        replace: Vec<String>,
        set: Vec<String>,
        allow_non_parameter: bool,
        record: Option<PathBuf>,
    ) -> Result<Self> {
        if name.is_empty() {
            bail!("--name must be non-empty");
        }
        if replace.is_empty() && set.is_empty() {
            bail!("at least one --replace or --set mutation is required");
        }

        let replacements = replace
            .into_iter()
            .map(|raw| parse_replace(&raw))
            .collect::<Result<Vec<_>>>()?;
        let sets = set
            .into_iter()
            .map(|raw| parse_set(&raw))
            .collect::<Result<Vec<_>>>()?;
        let mut targets = BTreeSet::new();
        for id in replacements
            .iter()
            .map(|mutation| &mutation.field_id)
            .chain(sets.iter().map(|mutation| &mutation.field_id))
        {
            if !targets.insert(id.as_str().to_string()) {
                bail!("duplicate mutation target `{}`", id.as_str());
            }
        }

        Ok(Self {
            base,
            out,
            name,
            replacements,
            sets,
            non_parameter_policy: allow_non_parameter.into(),
            record,
        })
    }
}

pub(crate) fn execute(request: DeriveRequest) -> Result<Vec<u8>> {
    let out = resolve_new_path(&request.out, "output")?;
    if out.exists() {
        bail!("output path already exists: {}", out.display());
    }
    let record = request
        .record
        .as_deref()
        .map(|path| resolve_new_path(path, "record"))
        .transpose()?;
    if let Some(record) = &record {
        if record.exists() {
            bail!("record path already exists: {}", record.display());
        }
        validate_record_path(&out, record)?;
    }

    let manifest = manifest::read(&request.base)
        .with_context(|| format!("reading base manifest at {}", request.base.display()))?;
    require_conformant(&request.base, "base")?;
    let base_hash = manifest
        .content_hash()
        .context("hashing conformant base manifest")?;
    let registry = load_registry(&request.base, &manifest)?;
    let (inventory, scalar_values) = build_inventory(&request.base, &manifest, &registry)?;

    let scalar_plan = plan_sets(&request, &manifest, &registry, &inventory, scalar_values)?;
    let replacement_plans = plan_replacements(&request, &manifest, &registry, &inventory)?;
    for plan in &replacement_plans {
        if !plan.non_parameter_ids.is_empty() {
            warn!(
                artifact_role = %manifest.artifacts()[plan.artifact_index].role.as_str(),
                field_ids = ?plan.non_parameter_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
                "allowing non-parameter artifact replacement"
            );
        }
    }

    let staging = create_unique_directory(&out, "staging")?;
    let mut guard = CleanupGuard::new(staging.clone());
    let result = stage_and_publish(
        &request,
        &out,
        record.as_deref(),
        &manifest,
        &base_hash,
        scalar_plan,
        replacement_plans,
        &staging,
        &mut guard,
    );
    match result {
        Ok(bytes) => {
            guard.disarm();
            Ok(bytes)
        }
        Err(error) => match guard.cleanup() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(cleanup)),
        },
    }
}

fn validate_record_path(out: &Path, record: &Path) -> Result<()> {
    if record == out || record.starts_with(out) {
        bail!(
            "record path must be outside output package: {}",
            record.display()
        );
    }
    Ok(())
}

fn parse_replace(raw: &str) -> Result<ReplaceMutation> {
    let (id, remainder) = split_mutation(raw, "--replace")?;
    if remainder.is_empty() {
        bail!("--replace target `{id}` has an empty file path");
    }
    Ok(ReplaceMutation {
        field_id: FieldId::new(id),
        source: PathBuf::from(remainder),
    })
}

fn parse_set(raw: &str) -> Result<SetMutation> {
    let (id, remainder) = split_mutation(raw, "--set")?;
    let value = serde_json::from_str(remainder)
        .with_context(|| format!("parsing --set JSON for exact FieldId `{id}`"))?;
    Ok(SetMutation {
        field_id: FieldId::new(id),
        value,
    })
}

fn split_mutation<'a>(raw: &'a str, option: &str) -> Result<(&'a str, &'a str)> {
    let (id, remainder) = raw
        .split_once('=')
        .ok_or_else(|| anyhow!("{option} mutation must contain `=`: `{raw}`"))?;
    if id.is_empty() {
        bail!("{option} mutation has an empty FieldId");
    }
    Ok((id, remainder))
}

fn resolve_new_path(path: &Path, label: &str) -> Result<PathBuf> {
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

fn require_conformant(path: &Path, label: &str) -> Result<()> {
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

fn load_registry(base: &Path, manifest: &Manifest) -> Result<FieldRegistry> {
    let matches = manifest
        .artifacts()
        .iter()
        .filter(|artifact| {
            artifact.format == ArtifactFormat::FieldRegistryV1
                && artifact.role.as_str() == "registry.fields"
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        bail!("expected exactly one registry.fields hmx/field_registry_v1 artifact");
    }
    let path = base.join(matches[0].path.as_str());
    let json = fs::read_to_string(&path)
        .with_context(|| format!("reading registry artifact {}", path.display()))?;
    FieldRegistry::from_json(&json).context("parsing typed field registry")
}

fn build_inventory(
    base: &Path,
    manifest: &Manifest,
    registry: &FieldRegistry,
) -> Result<(
    BTreeMap<FieldId, Vec<ResolvedSource>>,
    Option<BTreeMap<String, Value>>,
)> {
    let mut inventory: BTreeMap<FieldId, Vec<ResolvedSource>> = BTreeMap::new();
    let scalar_artifacts = manifest
        .artifacts()
        .iter()
        .enumerate()
        .filter(|(_, artifact)| artifact.format == ArtifactFormat::ParameterScalarsV1)
        .collect::<Vec<_>>();
    let scalar_values = if scalar_artifacts.is_empty() {
        None
    } else if scalar_artifacts.len() > 1 {
        bail!("multiple hmx/parameter_scalars_v1 artifacts are ambiguous");
    } else {
        let (index, artifact) = scalar_artifacts[0];
        let path = base.join(artifact.path.as_str());
        read_parameter_scalars(&path, registry)
            .with_context(|| format!("typing scalar artifact {}", path.display()))?;
        let bytes = fs::read_to_string(&path)
            .with_context(|| format!("reading scalar artifact {}", path.display()))?;
        let values: BTreeMap<String, Value> = serde_json::from_str(&bytes)
            .with_context(|| format!("parsing scalar JSON {}", path.display()))?;
        for id in values.keys() {
            inventory
                .entry(FieldId::new(id))
                .or_default()
                .push(ResolvedSource::Scalar {
                    artifact_index: index,
                });
        }
        Some(values)
    };

    for (index, artifact) in manifest.artifacts().iter().enumerate() {
        if artifact.format == ArtifactFormat::ParquetDomainAttributesV1 {
            let path = base.join(artifact.path.as_str());
            let table = read_domain_attributes(&path)
                .with_context(|| format!("reading domain attributes {}", path.display()))?;
            for column in table.attributes() {
                let id = FieldId::new(column.field_id());
                if registry.get(&id).is_some() {
                    inventory
                        .entry(id)
                        .or_default()
                        .push(ResolvedSource::Physical {
                            artifact_index: index,
                        });
                }
            }
        } else if artifact.format != ArtifactFormat::ParameterScalarsV1 {
            if let Some(variable) = &artifact.variable {
                let id = FieldId::new(variable.as_str());
                if registry.get(&id).is_some() {
                    inventory
                        .entry(id)
                        .or_default()
                        .push(ResolvedSource::Physical {
                            artifact_index: index,
                        });
                }
            }
        }
    }
    Ok((inventory, scalar_values))
}

fn exact_source<'a>(
    id: &FieldId,
    registry: &FieldRegistry,
    inventory: &'a BTreeMap<FieldId, Vec<ResolvedSource>>,
) -> Result<&'a ResolvedSource> {
    registry
        .require(id)
        .with_context(|| format!("resolving undeclared exact FieldId `{}`", id.as_str()))?;
    let sources = inventory
        .get(id)
        .ok_or_else(|| anyhow!("exact FieldId `{}` has no source", id.as_str()))?;
    if sources.len() != 1 {
        bail!("exact FieldId `{}` has ambiguous sources", id.as_str());
    }
    Ok(&sources[0])
}

fn plan_sets(
    request: &DeriveRequest,
    manifest: &Manifest,
    registry: &FieldRegistry,
    inventory: &BTreeMap<FieldId, Vec<ResolvedSource>>,
    scalar_values: Option<BTreeMap<String, Value>>,
) -> Result<Option<ScalarRewritePlan>> {
    if request.sets.is_empty() {
        return Ok(None);
    }
    let mut values =
        scalar_values.ok_or_else(|| anyhow!("--set requires exactly one scalar artifact"))?;
    let mut artifact_index = None;
    let mut changed_ids = Vec::new();
    for mutation in &request.sets {
        let spec = registry.require(&mutation.field_id).with_context(|| {
            format!(
                "resolving --set exact FieldId `{}`",
                mutation.field_id.as_str()
            )
        })?;
        if spec.role() != SemanticRole::Parameter {
            bail!(
                "--set exact FieldId `{}` has semantic role `{}`; parameter required",
                mutation.field_id.as_str(),
                spec.role().as_str()
            );
        }
        validate_scalar_value(&mutation.field_id, spec, &mutation.value)?;
        match exact_source(&mutation.field_id, registry, inventory)? {
            ResolvedSource::Scalar {
                artifact_index: index,
            } => artifact_index = Some(*index),
            ResolvedSource::Physical { .. } => {
                bail!(
                    "--set exact FieldId `{}` is a physical source",
                    mutation.field_id.as_str()
                )
            }
        }
        if !values.contains_key(mutation.field_id.as_str()) {
            bail!(
                "--set exact scalar key `{}` is missing",
                mutation.field_id.as_str()
            );
        }
        values.insert(
            mutation.field_id.as_str().to_string(),
            mutation.value.clone(),
        );
        changed_ids.push(mutation.field_id.clone());
    }
    changed_ids.sort();
    let index = artifact_index.ok_or_else(|| anyhow!("--set requires a scalar source"))?;
    if manifest.artifacts()[index].format != ArtifactFormat::ParameterScalarsV1 {
        bail!("resolved --set artifact is not hmx/parameter_scalars_v1");
    }
    Ok(Some(ScalarRewritePlan {
        artifact_index: index,
        values,
        changed_ids,
    }))
}

fn validate_scalar_value(id: &FieldId, spec: &FieldSpec, value: &Value) -> Result<()> {
    match spec.extent() {
        Extent::Scalar if finite_number(value) => Ok(()),
        Extent::Scalar => bail!(
            "--set exact FieldId `{}` requires one finite JSON number",
            id.as_str()
        ),
        Extent::PerLayer => {
            let values = value.as_array().ok_or_else(|| {
                anyhow!(
                    "--set exact FieldId `{}` requires a JSON number array",
                    id.as_str()
                )
            })?;
            let expected = spec
                .layer_count()
                .ok_or_else(|| anyhow!("per_layer FieldId `{}` lacks layer_count", id.as_str()))?
                .get();
            if values.len() != expected {
                bail!(
                    "--set exact FieldId `{}` requires {expected} layers, got {}",
                    id.as_str(),
                    values.len()
                );
            }
            if !values.iter().all(finite_number) {
                bail!(
                    "--set exact FieldId `{}` requires only finite JSON numbers",
                    id.as_str()
                );
            }
            Ok(())
        }
    }
}

fn finite_number(value: &Value) -> bool {
    value.as_f64().is_some_and(f64::is_finite)
}

fn plan_replacements(
    request: &DeriveRequest,
    manifest: &Manifest,
    registry: &FieldRegistry,
    inventory: &BTreeMap<FieldId, Vec<ResolvedSource>>,
) -> Result<Vec<ReplacementPlan>> {
    let mut claimed_artifacts = BTreeSet::new();
    let mut plans = Vec::new();
    for mutation in &request.replacements {
        let metadata = fs::metadata(&mutation.source).with_context(|| {
            format!(
                "reading --replace file for `{}`",
                mutation.field_id.as_str()
            )
        })?;
        if !metadata.is_file() {
            bail!(
                "--replace exact FieldId `{}` requires a regular file",
                mutation.field_id.as_str()
            );
        }
        let artifact_index = match exact_source(&mutation.field_id, registry, inventory)? {
            ResolvedSource::Physical { artifact_index } => *artifact_index,
            ResolvedSource::Scalar { .. } => {
                bail!(
                    "--replace exact FieldId `{}` is a scalar source",
                    mutation.field_id.as_str()
                )
            }
        };
        if !claimed_artifacts.insert(artifact_index) {
            bail!(
                "conflicting --replace operations address artifact `{}`",
                manifest.artifacts()[artifact_index].role.as_str()
            );
        }
        let artifact = &manifest.artifacts()[artifact_index];
        let affected_ids = affected_fields(
            &request.base,
            artifact,
            &mutation.source,
            &mutation.field_id,
            registry,
        )?;
        if artifact.format == ArtifactFormat::Cog {
            require_single_band(&request.base.join(artifact.path.as_str()), "existing")?;
            require_single_band(&mutation.source, "replacement")?;
        }
        let non_parameter_ids = affected_ids
            .iter()
            .filter_map(|id| {
                registry
                    .get(id)
                    .filter(|spec| spec.role() != SemanticRole::Parameter)
                    .map(|_| id.clone())
            })
            .collect::<Vec<_>>();
        if request.non_parameter_policy == NonParameterPolicy::ParameterOnly
            && !non_parameter_ids.is_empty()
        {
            let details = non_parameter_ids
                .iter()
                .map(|id| {
                    let role = registry.get(id).map(FieldSpec::role);
                    format!(
                        "`{}` ({})",
                        id.as_str(),
                        role.map_or("undeclared", |r| r.as_str())
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            bail!("replacement affected non-parameter fields: {details}");
        }
        plans.push(ReplacementPlan {
            artifact_index,
            affected_ids,
            old_sha256: artifact.sha256.as_str().to_string(),
            replacement_source: mutation.source.clone(),
            non_parameter_ids,
        });
    }
    Ok(plans)
}

fn affected_fields(
    base: &Path,
    artifact: &hmx_core::types::Artifact,
    replacement: &Path,
    target: &FieldId,
    registry: &FieldRegistry,
) -> Result<Vec<FieldId>> {
    if artifact.format != ArtifactFormat::ParquetDomainAttributesV1 {
        let variable = artifact.variable.as_ref().ok_or_else(|| {
            anyhow!(
                "physical artifact `{}` lacks variable",
                artifact.role.as_str()
            )
        })?;
        return Ok(vec![FieldId::new(variable.as_str())]);
    }
    let old = read_domain_attributes(base.join(artifact.path.as_str())).with_context(|| {
        format!(
            "reading existing domain attributes for `{}`",
            target.as_str()
        )
    })?;
    let new = read_domain_attributes(replacement).with_context(|| {
        format!(
            "reading replacement domain attributes for `{}`",
            target.as_str()
        )
    })?;
    let new_ids = new
        .attributes()
        .iter()
        .map(|column| FieldId::new(column.field_id()))
        .collect::<BTreeSet<_>>();
    if !new_ids.contains(target) {
        bail!(
            "replacement domain attributes dropped targeted FieldId `{}`",
            target.as_str()
        );
    }
    let mut union = old
        .attributes()
        .iter()
        .map(|column| FieldId::new(column.field_id()))
        .chain(new_ids)
        .filter(|id| registry.get(id).is_some())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    union.sort();
    Ok(union)
}

fn require_single_band(path: &Path, label: &str) -> Result<()> {
    let metadata = read_cog_metadata(path)
        .with_context(|| format!("reading {label} COG metadata at {}", path.display()))?;
    if metadata.band_count() != 1 {
        bail!(
            "{label} COG {} has {} bands; exactly one required",
            path.display(),
            metadata.band_count()
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stage_and_publish(
    request: &DeriveRequest,
    out: &Path,
    record_path: Option<&Path>,
    manifest: &Manifest,
    base_hash: &hmx_core::hash::ContentHash,
    scalar_plan: Option<ScalarRewritePlan>,
    replacement_plans: Vec<ReplacementPlan>,
    staging: &Path,
    guard: &mut CleanupGuard,
) -> Result<Vec<u8>> {
    let replacement_by_index = replacement_plans
        .iter()
        .map(|plan| (plan.artifact_index, plan))
        .collect::<BTreeMap<_, _>>();
    for (index, artifact) in manifest.artifacts().iter().enumerate() {
        let destination = staging.join(artifact.path.as_str());
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating staged artifact parent {}", parent.display()))?;
        }
        if let Some(plan) = replacement_by_index.get(&index) {
            copy_regular(&plan.replacement_source, &destination)?;
        } else if scalar_plan
            .as_ref()
            .is_some_and(|plan| plan.artifact_index == index)
        {
            let plan = scalar_plan
                .as_ref()
                .ok_or_else(|| anyhow!("missing scalar plan"))?;
            let mut bytes =
                serde_json::to_vec(&plan.values).context("serializing scalar artifact")?;
            bytes.push(b'\n');
            fs::write(&destination, bytes).with_context(|| {
                format!("writing staged scalar artifact {}", destination.display())
            })?;
        } else {
            copy_declared(&request.base.join(artifact.path.as_str()), &destination)?;
        }
    }

    let mut artifacts = manifest.artifacts().to_vec();
    let mut records = Vec::new();
    for plan in &replacement_plans {
        let staged_path = staging.join(artifacts[plan.artifact_index].path.as_str());
        let (digest, size) = digest_file(&staged_path)?;
        artifacts[plan.artifact_index].sha256 = Sha256::new(&digest);
        artifacts[plan.artifact_index].size_bytes = Some(size);
        records.push(ReplacementRecord {
            artifact_role: artifacts[plan.artifact_index].role.as_str().to_string(),
            field_ids: plan
                .affected_ids
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            new_sha256: digest,
            old_sha256: plan.old_sha256.clone(),
        });
    }
    if let Some(plan) = &scalar_plan {
        let artifact = &manifest.artifacts()[plan.artifact_index];
        let staged_path = staging.join(artifact.path.as_str());
        let (digest, size) = digest_file(&staged_path)?;
        artifacts[plan.artifact_index].sha256 = Sha256::new(&digest);
        artifacts[plan.artifact_index].size_bytes = Some(size);
        records.push(ReplacementRecord {
            artifact_role: artifact.role.as_str().to_string(),
            field_ids: plan
                .changed_ids
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            new_sha256: digest,
            old_sha256: artifact.sha256.as_str().to_string(),
        });
    }

    let created_at = OffsetDateTime::now_utc();
    let derived = manifest
        .reconstruct_for_derivation(request.name.clone(), created_at, artifacts)
        .context("reconstructing derived manifest")?;
    let manifest_bytes = derived
        .deterministic_json_bytes()
        .context("serializing deterministic derived manifest")?;
    fs::write(staging.join("manifest.json"), manifest_bytes)
        .context("writing staged manifest.json")?;
    require_conformant(staging, "staged derived")?;
    let staged_manifest =
        manifest::read(staging).context("re-reading validated staged manifest")?;
    let derived_hash = staged_manifest
        .content_hash()
        .context("hashing validated staged manifest")?;

    records.sort_by(|left, right| {
        (&left.artifact_role, &left.field_ids).cmp(&(&right.artifact_role, &right.field_ids))
    });
    let overrides = replacement_plans
        .iter()
        .flat_map(|plan| plan.non_parameter_ids.iter())
        .map(|id| id.as_str().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let timestamp = created_at
        .format(&Rfc3339)
        .context("formatting derivation record timestamp")?;
    let record = DerivationRecord {
        base_content_hash: ContentHashRecord {
            algo: base_hash.hash_algo().to_string(),
            value: base_hash.as_str().to_string(),
        },
        created_at: timestamp,
        derived_content_hash: ContentHashRecord {
            algo: derived_hash.hash_algo().to_string(),
            value: derived_hash.as_str().to_string(),
        },
        derived_name: request.name.clone(),
        non_parameter_overrides: overrides,
        replaced: records,
        tool_version: env!("CARGO_PKG_VERSION"),
    };
    let mut record_bytes = serde_json::to_vec(&record).context("serializing derivation record")?;
    record_bytes.push(b'\n');

    let record_temp = if let Some(path) = record_path {
        let temp = create_unique_file(path, "record")?;
        guard.add(temp.clone());
        let mut file = OpenOptions::new()
            .write(true)
            .open(&temp)
            .with_context(|| format!("opening staged record {}", temp.display()))?;
        file.write_all(&record_bytes)
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
    Ok(record_bytes)
}

fn create_unique_directory(destination: &Path, kind: &str) -> Result<PathBuf> {
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

fn copy_regular(source: &Path, destination: &Path) -> Result<()> {
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

fn copy_declared(source: &Path, destination: &Path) -> Result<()> {
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

fn digest_file(path: &Path) -> Result<(String, u64)> {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::fs::File;
    use std::path::{Path, PathBuf};

    use hmx_core::manifest;

    use hmx_core::registry::FieldRegistry;
    use hmx_core::types::FieldId;
    use tiff::encoder::{TiffEncoder, colortype};

    use super::{
        ContentHashRecord, DerivationRecord, DeriveRequest, ReplacementRecord, ResolvedSource,
        copy_declared, exact_source, execute, parse_set, require_single_band, resolve_new_path,
        validate_record_path, validate_scalar_value,
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "hmx-derive-{label}-{}-{}",
                std::process::id(),
                super::UNIQUE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create unique test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn copy_valid_fixture(destination: &Path) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/valid");
        fs::create_dir_all(destination.join("registry")).expect("create registry directory");
        fs::create_dir_all(destination.join("parameter")).expect("create parameter directory");
        for relative in [
            "manifest.json",
            "registry/fields.json",
            "parameter/scalars.json",
        ] {
            fs::copy(fixture.join(relative), destination.join(relative))
                .expect("copy fixture file");
        }
    }

    fn policy_registry() -> FieldRegistry {
        FieldRegistry::from_json(
            r#"{"registry_version":"1","fields":[
{"id":"scalar","domain":"cell","quantity":"q","units":"u","value_type":"f64","time_meaning":"instant","role":"parameter","conservation_class":"none","extent":"scalar"},
{"id":"layers","domain":"cell","quantity":"q","units":"u","value_type":"f64","time_meaning":"instant","role":"parameter","conservation_class":"none","extent":"per_layer","layer_count":2},
{"id":"forcing","domain":"cell","quantity":"q","units":"u","value_type":"f64","time_meaning":"instant","role":"forcing","conservation_class":"none","extent":"scalar"}
]}"#,
        )
        .expect("policy registry parses")
    }

    #[test]
    fn mutation_parser_preserves_exact_bytes_and_first_separator() {
        let parsed = parse_set(" Field=1").expect("set parses");
        assert_eq!(parsed.field_id.as_str(), " Field");
        assert!(parse_set("x=\"a=b\"").is_ok());
        assert!(parse_set("missing").is_err());
        assert!(parse_set("=1").is_err());
        assert!(
            DeriveRequest::new(
                "base".into(),
                "out".into(),
                "name".into(),
                vec!["x=file".into()],
                vec!["x=1".into()],
                false,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn exact_resolution_rejects_alternate_names_ambiguity_and_wrong_source_kind() {
        let registry = policy_registry();
        let scalar = FieldId::new("scalar");
        let mut inventory = BTreeMap::new();
        inventory.insert(
            scalar.clone(),
            vec![ResolvedSource::Scalar { artifact_index: 1 }],
        );
        assert!(matches!(
            exact_source(&scalar, &registry, &inventory).expect("exact scalar resolves"),
            ResolvedSource::Scalar { artifact_index: 1 }
        ));
        for alternate in ["parameter.scalars", "Scalar", " scalar", "missing"] {
            assert!(exact_source(&FieldId::new(alternate), &registry, &inventory).is_err());
        }
        inventory.insert(
            scalar.clone(),
            vec![
                ResolvedSource::Scalar { artifact_index: 1 },
                ResolvedSource::Physical { artifact_index: 2 },
            ],
        );
        assert!(exact_source(&scalar, &registry, &inventory).is_err());
    }

    #[test]
    fn scalar_extent_validation_is_typed_finite_and_parameter_role_is_not_overridden() {
        let registry = policy_registry();
        let scalar = registry.get(&FieldId::new("scalar")).expect("scalar field");
        let layers = registry.get(&FieldId::new("layers")).expect("layer field");
        assert!(
            validate_scalar_value(&FieldId::new("scalar"), scalar, &serde_json::json!(1.5)).is_ok()
        );
        assert!(
            validate_scalar_value(&FieldId::new("scalar"), scalar, &serde_json::json!([1.0]))
                .is_err()
        );
        assert!(
            validate_scalar_value(&FieldId::new("scalar"), scalar, &serde_json::json!(null))
                .is_err()
        );
        assert!(
            validate_scalar_value(&FieldId::new("scalar"), scalar, &serde_json::json!(true))
                .is_err()
        );
        assert!(
            validate_scalar_value(&FieldId::new("scalar"), scalar, &serde_json::json!("1"))
                .is_err()
        );
        assert!(
            validate_scalar_value(&FieldId::new("layers"), layers, &serde_json::json!([1, 2]))
                .is_ok()
        );
        assert!(
            validate_scalar_value(&FieldId::new("layers"), layers, &serde_json::json!([1]))
                .is_err()
        );
        assert!(
            validate_scalar_value(
                &FieldId::new("layers"),
                layers,
                &serde_json::json!([[1], [2]])
            )
            .is_err()
        );
        assert!(parse_set("scalar=NaN").is_err());
        assert!(parse_set("scalar=1 trailing").is_err());

        let request = DeriveRequest::new(
            "base".into(),
            "out".into(),
            "name".into(),
            Vec::new(),
            vec!["forcing=1".into()],
            true,
            None,
        )
        .expect("override request parses");
        assert_eq!(
            request.non_parameter_policy,
            super::NonParameterPolicy::Allow
        );
    }

    #[test]
    fn cog_policy_reads_real_tiff_metadata_and_refuses_multiband() {
        let root = TestDirectory::new("cog");
        let one = root.path().join("one.tif");
        let three = root.path().join("three.tif");
        TiffEncoder::new(File::create(&one).expect("create one-band TIFF"))
            .expect("create TIFF encoder")
            .write_image::<colortype::Gray8>(1, 1, &[1])
            .expect("write one-band TIFF");
        TiffEncoder::new(File::create(&three).expect("create multi-band TIFF"))
            .expect("create TIFF encoder")
            .write_image::<colortype::RGB8>(1, 1, &[1, 2, 3])
            .expect("write multi-band TIFF");
        require_single_band(&one, "existing").expect("one-band existing accepted");
        require_single_band(&one, "replacement").expect("one-band replacement accepted");
        assert!(require_single_band(&three, "existing").is_err());
        assert!(require_single_band(&three, "replacement").is_err());
    }

    #[test]
    fn production_preflight_refuses_multiband_old_or_new_cog_with_override() {
        let root = TestDirectory::new("cog-preflight");
        let one = root.path().join("one.tif");
        let three = root.path().join("three.tif");
        TiffEncoder::new(File::create(&one).expect("create one-band TIFF"))
            .expect("create TIFF encoder")
            .write_image::<colortype::Gray8>(1, 1, &[1])
            .expect("write one-band TIFF");
        TiffEncoder::new(File::create(&three).expect("create multi-band TIFF"))
            .expect("create TIFF encoder")
            .write_image::<colortype::RGB8>(1, 1, &[1, 2, 3])
            .expect("write multi-band TIFF");
        for (label, old, new) in [("old", &three, &one), ("new", &one, &three)] {
            let base = root.path().join(format!("base-{label}"));
            fs::create_dir_all(base.join("registry")).expect("create registry directory");
            fs::create_dir(base.join("data")).expect("create data directory");
            fs::copy(old, base.join("data/p.tif")).expect("copy old COG");
            fs::write(
                base.join("registry/fields.json"),
                r#"{"registry_version":"1","fields":[{"id":"p","domain":"cell","quantity":"q","units":"u","value_type":"f64","time_meaning":"instant","role":"parameter","conservation_class":"none","extent":"scalar"}]}"#,
            )
            .expect("write registry");
            fs::write(
                base.join("manifest.json"),
                r#"{"format_version":"0.2","name":"base","created_at":"2026-07-10T00:00:00Z","producer":"test","producer_version":"1","package_kind":"input","crs":"EPSG:32645","grid":{"crs":"EPSG:32645","extent":{"xmin":0.0,"ymin":0.0,"xmax":1.0,"ymax":1.0},"cell_size":1.0,"nx":1,"ny":1,"origin":"upper_left"},"domains":[{"id":"cell","entity_count":1,"index_base":"dense_zero_based"}],"mappings":[],"artifacts":[{"role":"registry.fields","path":"registry/fields.json","format":"hmx/field_registry_v1","sha256":"0000000000000000000000000000000000000000000000000000000000000000","size_bytes":1},{"role":"parameter.p","path":"data/p.tif","format":"cog","sha256":"1111111111111111111111111111111111111111111111111111111111111111","size_bytes":1,"variable":"p"}]}"#,
            )
            .expect("write manifest");
            assert!(
                hmx_core::validate::validate(&base)
                    .expect("validate COG base")
                    .conformant()
            );
            let out = root.path().join(format!("out-{label}"));
            let request = DeriveRequest::new(
                base,
                out.clone(),
                "derived".into(),
                vec![format!("p={}", new.display())],
                Vec::new(),
                true,
                None,
            )
            .expect("request parses");
            assert!(execute(request).is_err());
            assert!(!out.exists());
        }
    }

    #[test]
    fn record_serialization_has_exact_nested_order_and_one_newline() {
        let record = DerivationRecord {
            base_content_hash: ContentHashRecord {
                algo: "sha256".into(),
                value: "base".into(),
            },
            created_at: "2026-07-10T00:00:00Z".into(),
            derived_content_hash: ContentHashRecord {
                algo: "sha256".into(),
                value: "derived".into(),
            },
            derived_name: "name".into(),
            non_parameter_overrides: vec!["a".into(), "z".into()],
            replaced: vec![ReplacementRecord {
                artifact_role: "parameter.scalars".into(),
                field_ids: vec!["a".into(), "z".into()],
                new_sha256: "new".into(),
                old_sha256: "old".into(),
            }],
            tool_version: env!("CARGO_PKG_VERSION"),
        };
        let mut bytes = serde_json::to_vec(&record).expect("serialize record");
        bytes.push(b'\n');
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
    fn recursive_copy_preserves_files_empty_directories_and_rejects_symlinks() {
        let root = std::env::temp_dir().join(format!(
            "hmx-derive-copy-{}-{}",
            std::process::id(),
            super::UNIQUE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
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
        fs::remove_dir_all(root).expect("remove test tree");
    }

    #[test]
    fn scalar_derivation_is_standalone_valid_and_deterministic() {
        let root = TestDirectory::new("success");
        let base = root.path().join("base");
        let out = root.path().join("derived");
        let record_path = root.path().join("record.json");
        fs::create_dir(&base).expect("create base package");
        copy_valid_fixture(&base);
        let base_manifest = manifest::read(&base).expect("read base manifest");
        let registry_before = fs::read(base.join("registry/fields.json")).expect("read registry");
        let request = DeriveRequest::new(
            base.clone(),
            out.clone(),
            "derived-name".to_string(),
            Vec::new(),
            vec!["cells.flow_dir=2".to_string()],
            false,
            Some(record_path.clone()),
        )
        .expect("request parses");
        let stdout = execute(request).expect("derive succeeds");

        assert_eq!(stdout, fs::read(&record_path).expect("read record"));
        assert_eq!(
            fs::read(out.join("parameter/scalars.json")).expect("read scalars"),
            b"{\"cells.flow_dir\":2}\n"
        );
        assert_eq!(
            fs::read(out.join("registry/fields.json")).expect("read copied registry"),
            registry_before
        );
        assert!(
            hmx_core::validate::validate(&out)
                .expect("validate derived")
                .conformant()
        );
        let derived_manifest = manifest::read(&out).expect("read derived manifest");
        assert_eq!(derived_manifest.name().as_str(), "derived-name");
        assert_ne!(derived_manifest.created_at(), base_manifest.created_at());
        assert_eq!(
            derived_manifest.artifacts()[0],
            base_manifest.artifacts()[0]
        );
        assert_ne!(
            derived_manifest.artifacts()[1].sha256,
            base_manifest.artifacts()[1].sha256
        );
        let record: serde_json::Value = serde_json::from_slice(&stdout).expect("parse record");
        assert_eq!(
            record["derived_content_hash"]["value"],
            derived_manifest
                .content_hash()
                .expect("hash derived")
                .as_str()
        );
        assert_eq!(record["tool_version"], env!("CARGO_PKG_VERSION"));
        assert!(stdout.ends_with(b"\n"));
        assert!(!stdout[..stdout.len() - 1].ends_with(b"\n"));
    }

    #[test]
    fn existing_destinations_and_unsafe_record_paths_are_refused_before_staging() {
        let root = TestDirectory::new("paths");
        let base = root.path().join("base");
        fs::create_dir(&base).expect("create base package");
        copy_valid_fixture(&base);
        let out = root.path().join("out");
        fs::write(&out, b"existing").expect("create existing output");
        let request = DeriveRequest::new(
            base.clone(),
            out,
            "derived".to_string(),
            Vec::new(),
            vec!["cells.flow_dir=2".to_string()],
            false,
            None,
        )
        .expect("request parses");
        assert!(execute(request).is_err());

        let out = root.path().join("future-out");
        let request = DeriveRequest::new(
            base,
            out.clone(),
            "derived".to_string(),
            Vec::new(),
            vec!["cells.flow_dir=2".to_string()],
            false,
            Some(out),
        )
        .expect("request parses");
        assert!(execute(request).is_err());
    }

    #[test]
    fn resolved_path_safety_handles_dotdot_and_symlinked_ancestors() {
        let root = TestDirectory::new("resolved-paths");
        let out = root.path().join("out");
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
            let link = root.path().join("linked-out");
            std::os::unix::fs::symlink(&out, &link).expect("create output symlink");
            let via_link = resolve_new_path(&link.join("nested/record.json"), "record")
                .expect("resolve symlink descendant");
            assert!(validate_record_path(&resolved_out, &via_link).is_err());
        }
        let sibling = resolve_new_path(&root.path().join("record.json"), "record")
            .expect("resolve sibling record");
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

    #[test]
    fn replacement_parameter_policy_reports_every_exact_override() {
        let root = TestDirectory::new("override");
        let base = root.path().join("base");
        fs::create_dir_all(base.join("registry")).expect("create registry directory");
        let registry = r#"{"registry_version":"1","fields":[{"id":"forcing","domain":"cell","quantity":"q","units":"u","value_type":"f64","time_meaning":"instant","role":"forcing","conservation_class":"none","extent":"scalar"}]}"#;
        fs::write(base.join("registry/fields.json"), registry).expect("write registry");
        fs::write(
            base.join("manifest.json"),
            r#"{"format_version":"0.2","name":"base","created_at":"2026-07-10T00:00:00Z","producer":"test","producer_version":"1","package_kind":"input","crs":"EPSG:32645","grid":{"crs":"EPSG:32645","extent":{"xmin":0.0,"ymin":0.0,"xmax":1.0,"ymax":1.0},"cell_size":1.0,"nx":1,"ny":1,"origin":"upper_left"},"domains":[{"id":"cell","entity_count":1,"index_base":"dense_zero_based"}],"mappings":[],"artifacts":[{"role":"registry.fields","path":"registry/fields.json","format":"hmx/field_registry_v1","sha256":"0000000000000000000000000000000000000000000000000000000000000000","size_bytes":1,"variable":"forcing"}]}"#,
        )
        .expect("write manifest");
        let replacement = root.path().join("replacement.json");
        fs::write(&replacement, registry).expect("write replacement registry");
        let denied_out = root.path().join("denied");
        let denied = DeriveRequest::new(
            base.clone(),
            denied_out.clone(),
            "denied".into(),
            vec![format!("forcing={}", replacement.display())],
            Vec::new(),
            false,
            None,
        )
        .expect("denied request parses");
        assert!(execute(denied).is_err());
        assert!(!denied_out.exists());

        let allowed_out = root.path().join("allowed");
        let allowed = DeriveRequest::new(
            base,
            allowed_out.clone(),
            "allowed".into(),
            vec![format!("forcing={}", replacement.display())],
            Vec::new(),
            true,
            None,
        )
        .expect("allowed request parses");
        let bytes = execute(allowed).expect("override derivation succeeds");
        let record: serde_json::Value = serde_json::from_slice(&bytes).expect("parse record");
        assert_eq!(
            record["non_parameter_overrides"],
            serde_json::json!(["forcing"])
        );
        assert_eq!(
            record["replaced"][0]["field_ids"],
            serde_json::json!(["forcing"])
        );
        assert!(
            hmx_core::validate::validate(allowed_out)
                .expect("validate output")
                .conformant()
        );
    }

    #[test]
    fn failed_real_staged_validation_cleans_every_created_path() {
        let root = TestDirectory::new("validation-cleanup");
        let base = root.path().join("base");
        let out = root.path().join("out");
        let record = root.path().join("record.json");
        let replacement = root.path().join("invalid-registry.json");
        fs::create_dir_all(base.join("registry")).expect("create registry directory");
        let registry = r#"{"registry_version":"1","fields":[{"id":"p","domain":"cell","quantity":"q","units":"u","value_type":"f64","time_meaning":"instant","role":"parameter","conservation_class":"none","extent":"scalar"}]}"#;
        fs::write(base.join("registry/fields.json"), registry).expect("write registry");
        fs::write(&replacement, b"not json").expect("write invalid replacement");
        fs::write(
            base.join("manifest.json"),
            r#"{"format_version":"0.2","name":"base","created_at":"2026-07-10T00:00:00Z","producer":"test","producer_version":"1","package_kind":"input","crs":"EPSG:32645","grid":{"crs":"EPSG:32645","extent":{"xmin":0.0,"ymin":0.0,"xmax":1.0,"ymax":1.0},"cell_size":1.0,"nx":1,"ny":1,"origin":"upper_left"},"domains":[{"id":"cell","entity_count":1,"index_base":"dense_zero_based"}],"mappings":[],"artifacts":[{"role":"registry.fields","path":"registry/fields.json","format":"hmx/field_registry_v1","sha256":"0000000000000000000000000000000000000000000000000000000000000000","size_bytes":1,"variable":"p"}]}"#,
        )
        .expect("write manifest");
        assert!(
            hmx_core::validate::validate(&base)
                .expect("validate base")
                .conformant()
        );
        let request = DeriveRequest::new(
            base,
            out.clone(),
            "derived".into(),
            vec![format!("p={}", replacement.display())],
            Vec::new(),
            false,
            Some(record.clone()),
        )
        .expect("request parses");
        assert!(execute(request).is_err());
        assert!(!out.exists());
        assert!(!record.exists());
        let leftovers = fs::read_dir(root.path())
            .expect("read test root")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("hmx-staging") || name.contains("hmx-record"))
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "temporary paths remain: {leftovers:?}"
        );
    }
}
