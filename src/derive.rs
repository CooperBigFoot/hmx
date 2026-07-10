//! Transactional standalone package derivation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use hmx_core::manifest::{self, Manifest};
use hmx_core::readers::cog_reader::read_cog_metadata;
use hmx_core::readers::control_plane::read_domain_attributes;
use hmx_core::readers::parameter_scalars_reader::read_parameter_scalars;
use hmx_core::registry::{FieldRegistry, FieldSpec};
use hmx_core::types::{ArtifactFormat, Extent, FieldId, SemanticRole, Sha256};
use serde_json::Value;
use time::OffsetDateTime;
use tracing::warn;

use crate::derivation::{
    CleanupGuard, DerivationRecord, ReplacementRecord, copy_declared, copy_regular,
    create_unique_directory, digest_file, publish_staged, require_conformant, resolve_new_path,
    validate_record_path,
};

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
        records.push(ReplacementRecord::replacement(
            artifacts[plan.artifact_index].role.as_str().to_string(),
            plan.affected_ids
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            plan.old_sha256.clone(),
            digest,
        ));
    }
    if let Some(plan) = &scalar_plan {
        let artifact = &manifest.artifacts()[plan.artifact_index];
        let staged_path = staging.join(artifact.path.as_str());
        let (digest, size) = digest_file(&staged_path)?;
        artifacts[plan.artifact_index].sha256 = Sha256::new(&digest);
        artifacts[plan.artifact_index].size_bytes = Some(size);
        records.push(ReplacementRecord::replacement(
            artifact.role.as_str().to_string(),
            plan.changed_ids
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            artifact.sha256.as_str().to_string(),
            digest,
        ));
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

    let overrides = replacement_plans
        .iter()
        .flat_map(|plan| plan.non_parameter_ids.iter())
        .map(|id| id.as_str().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let record = DerivationRecord::new(
        base_hash,
        created_at,
        &derived_hash,
        request.name.clone(),
        overrides,
        records,
    )?;
    let record_bytes = record.to_bytes()?;
    publish_staged(staging, out, record_path, &record_bytes, guard)?;
    Ok(record_bytes)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::fs::File;
    use std::path::Path;

    use hmx_core::manifest;

    use hmx_core::registry::FieldRegistry;
    use hmx_core::types::FieldId;
    use tiff::encoder::{TiffEncoder, colortype};

    use crate::derivation::TestDirectory;

    use super::{
        DeriveRequest, ResolvedSource, exact_source, execute, parse_set, require_single_band,
        validate_scalar_value,
    };

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
