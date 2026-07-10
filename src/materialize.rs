//! Transactional scalar-parameter materialization.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use hmx_core::manifest::{self, Manifest};
use hmx_core::readers::parameter_scalars_reader::{ParameterScalarValue, read_parameter_scalars};
use hmx_core::registry::FieldRegistry;
use hmx_core::types::{
    Artifact, ArtifactFormat, ArtifactRole, Crs, FieldId, Grid, GridOrigin, RelativePath, Sha256,
    Variable,
};
use sha2::{Digest, Sha256 as Sha256Hasher};
use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::ColorType;
use tiff::tags::{ExtraSamples, PhotometricInterpretation, SampleFormat, Tag};
use time::OffsetDateTime;

use crate::derivation::{
    CleanupGuard, DerivationRecord, ReplacementRecord, copy_declared, create_unique_directory,
    digest_file, publish_staged, require_conformant, resolve_new_path, validate_record_path,
};

pub(crate) struct MaterializeRequest {
    base: PathBuf,
    out: PathBuf,
    record: Option<PathBuf>,
}

impl MaterializeRequest {
    pub(crate) fn new(base: PathBuf, out: PathBuf, record: Option<PathBuf>) -> Self {
        Self { base, out, record }
    }
}

struct MaterializationPlan {
    field_id: FieldId,
    role: String,
    path: String,
    values: Vec<f64>,
}

struct MaterializedFloat;

impl ColorType for MaterializedFloat {
    type Inner = f64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::IEEEFP];

    fn horizontal_predict(_: &[Self::Inner], _: &mut Vec<Self::Inner>) {
        unreachable!("horizontal prediction is disabled for IEEE floating-point data")
    }
}

pub(crate) fn execute(request: MaterializeRequest) -> Result<Vec<u8>> {
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
    let (scalar_index, scalar_artifact) = require_scalar_artifact(&manifest)?;
    let scalar_path = request.base.join(scalar_artifact.path.as_str());
    let scalars = read_parameter_scalars(&scalar_path, &registry)
        .with_context(|| format!("typing scalar artifact {}", scalar_path.display()))?;
    let epsg = preflight_grid(manifest.grid())?;
    let plans = build_plans(&manifest, &registry, &scalars)?;

    let staging = create_unique_directory(&out, "staging")?;
    let mut guard = CleanupGuard::new(staging.clone());
    let result = stage_and_publish(
        &request,
        &out,
        record.as_deref(),
        &manifest,
        &base_hash,
        scalar_index,
        &plans,
        epsg,
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

fn require_scalar_artifact(manifest: &Manifest) -> Result<(usize, &Artifact)> {
    let matches = manifest
        .artifacts()
        .iter()
        .enumerate()
        .filter(|(_, artifact)| artifact.format == ArtifactFormat::ParameterScalarsV1)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        bail!(
            "materialize requires exactly one hmx/parameter_scalars_v1 artifact; found {}",
            matches.len()
        );
    }
    Ok(matches[0])
}

fn preflight_grid(grid: &Grid) -> Result<u16> {
    if grid.nx == 0 || grid.ny == 0 {
        bail!("materialize grid dimensions must be nonzero");
    }
    if !grid.cell_size.is_finite() || grid.cell_size <= 0.0 {
        bail!("materialize grid cell_size must be finite and positive");
    }
    let extent = grid.extent;
    if ![extent.xmin, extent.ymin, extent.xmax, extent.ymax]
        .iter()
        .all(|value| value.is_finite())
    {
        bail!("materialize grid extent coordinates must be finite");
    }
    if grid.origin != GridOrigin::UpperLeft {
        bail!("materialize requires an upper_left grid origin");
    }
    require_close(
        "x",
        extent.xmax - extent.xmin,
        f64::from(grid.nx) * grid.cell_size,
    )?;
    require_close(
        "y",
        extent.ymax - extent.ymin,
        f64::from(grid.ny) * grid.cell_size,
    )?;
    let observed = grid.crs.as_str();
    let digits = observed.strip_prefix("EPSG:").ok_or_else(|| {
        anyhow!("unsupported materialize grid CRS `{observed}`; expected EPSG:<decimal>")
    })?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("unsupported materialize grid CRS `{observed}`; expected EPSG:<decimal>");
    }
    let code = digits.parse::<u16>().with_context(|| {
        format!("unsupported materialize grid CRS `{observed}`; EPSG code must fit u16")
    })?;
    if code == 0 {
        bail!("unsupported materialize grid CRS `{observed}`; EPSG code must be nonzero");
    }
    Ok(code)
}

fn require_close(axis: &str, actual: f64, expected: f64) -> Result<()> {
    let tolerance = 1e-9 * actual.abs().max(expected.abs()).max(1.0);
    if (actual - expected).abs() > tolerance {
        bail!(
            "materialize grid {axis} extent is inconsistent: actual {actual}, expected {expected}"
        );
    }
    Ok(())
}

fn build_plans(
    manifest: &Manifest,
    registry: &FieldRegistry,
    scalars: &hmx_core::readers::parameter_scalars_reader::ParameterScalars,
) -> Result<Vec<MaterializationPlan>> {
    let mut roles = manifest
        .artifacts()
        .iter()
        .filter(|artifact| artifact.format != ArtifactFormat::ParameterScalarsV1)
        .map(|artifact| artifact.role.as_str().to_string())
        .collect::<BTreeSet<_>>();
    let mut paths = manifest
        .artifacts()
        .iter()
        .filter(|artifact| artifact.format != ArtifactFormat::ParameterScalarsV1)
        .map(|artifact| artifact.path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    let mut plans = Vec::with_capacity(scalars.len());
    for (field_id, value) in scalars.iter() {
        registry.require(field_id).with_context(|| {
            format!(
                "resolving materialized exact FieldId `{}`",
                field_id.as_str()
            )
        })?;
        let values = match value {
            ParameterScalarValue::Scalar { value } => vec![*value],
            ParameterScalarValue::PerLayer { values } => values.clone(),
        };
        require_supported_band_count(field_id, values.len())?;
        let digest = format!("{:x}", Sha256Hasher::digest(field_id.as_str().as_bytes()));
        let role = format!("parameter.materialized.{digest}");
        let path = format!("parameter/materialized/{digest}.tif");
        if !roles.insert(role.clone()) {
            bail!("generated artifact role collision `{role}`");
        }
        if paths.iter().any(|existing| path_conflicts(existing, &path)) {
            bail!("generated artifact path collision `{path}`");
        }
        paths.insert(path.clone());
        plans.push(MaterializationPlan {
            field_id: field_id.clone(),
            role,
            path,
            values,
        });
    }
    Ok(plans)
}

fn require_supported_band_count(field_id: &FieldId, count: usize) -> Result<()> {
    if count > usize::from(u16::MAX) {
        bail!(
            "unsupported layer count {count} for exact FieldId `{}`; TIFF SamplesPerPixel is limited to {}",
            field_id.as_str(),
            u16::MAX
        );
    }
    Ok(())
}

fn path_conflicts(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[allow(clippy::too_many_arguments)]
fn stage_and_publish(
    request: &MaterializeRequest,
    out: &Path,
    record_path: Option<&Path>,
    manifest: &Manifest,
    base_hash: &hmx_core::hash::ContentHash,
    scalar_index: usize,
    plans: &[MaterializationPlan],
    epsg: u16,
    staging: &Path,
    guard: &mut CleanupGuard,
) -> Result<Vec<u8>> {
    for (index, artifact) in manifest.artifacts().iter().enumerate() {
        if index == scalar_index {
            continue;
        }
        let destination = staging.join(artifact.path.as_str());
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating staged artifact parent {}", parent.display()))?;
        }
        copy_declared(&request.base.join(artifact.path.as_str()), &destination)?;
    }

    let mut generated = Vec::with_capacity(plans.len());
    let mut records = Vec::with_capacity(plans.len() + 1);
    for plan in plans {
        let destination = staging.join(&plan.path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("creating materialized artifact parent {}", parent.display())
            })?;
        }
        write_constant_tiff(&destination, manifest.grid(), epsg, &plan.values)?;
        let (digest, size_bytes) = digest_file(&destination)?;
        generated.push(Artifact {
            role: ArtifactRole::new(&plan.role),
            path: RelativePath::new(&plan.path),
            format: ArtifactFormat::Cog,
            sha256: Sha256::new(&digest),
            size_bytes: Some(size_bytes),
            crs: Some(Crs::new(manifest.grid().crs.as_str())),
            domain: None,
            variable: Some(Variable::new(plan.field_id.as_str())),
            unit: None,
            time_meaning: None,
            interval_seconds: None,
            row_count: None,
            first_step_index: None,
            last_step_index: None,
        });
        records.push(ReplacementRecord::addition(
            plan.role.clone(),
            vec![plan.field_id.as_str().to_string()],
            digest,
        ));
    }

    let scalar_artifact = &manifest.artifacts()[scalar_index];
    records.push(ReplacementRecord::removal(
        scalar_artifact.role.as_str().to_string(),
        plans
            .iter()
            .map(|plan| plan.field_id.as_str().to_string())
            .collect(),
        scalar_artifact.sha256.as_str().to_string(),
    ));
    let mut artifacts = Vec::with_capacity(manifest.artifacts().len() - 1 + generated.len());
    for (index, artifact) in manifest.artifacts().iter().enumerate() {
        if index == scalar_index {
            artifacts.extend(generated.iter().cloned());
        } else {
            artifacts.push(artifact.clone());
        }
    }

    let created_at = OffsetDateTime::now_utc();
    let name = manifest.name().as_str().to_string();
    let materialized = manifest
        .reconstruct_for_derivation(name.clone(), created_at, artifacts)
        .context("reconstructing materialized manifest")?;
    let manifest_bytes = materialized
        .deterministic_json_bytes()
        .context("serializing deterministic materialized manifest")?;
    fs::write(staging.join("manifest.json"), manifest_bytes)
        .context("writing staged manifest.json")?;
    require_conformant(staging, "staged materialized")?;
    let staged_manifest =
        manifest::read(staging).context("re-reading validated staged materialized manifest")?;
    let derived_hash = staged_manifest
        .content_hash()
        .context("hashing validated staged materialized manifest")?;
    let record = DerivationRecord::new(
        base_hash,
        created_at,
        &derived_hash,
        name,
        Vec::new(),
        records,
    )?;
    let record_bytes = record.to_bytes()?;
    publish_staged(staging, out, record_path, &record_bytes, guard)?;
    Ok(record_bytes)
}

fn write_constant_tiff(path: &Path, grid: &Grid, epsg: u16, values: &[f64]) -> Result<()> {
    let pixel_count = usize::try_from(grid.nx)
        .context("converting materialized grid width")?
        .checked_mul(usize::try_from(grid.ny).context("converting materialized grid height")?)
        .ok_or_else(|| anyhow!("materialized pixel count overflows usize"))?;
    let sample_count = pixel_count
        .checked_mul(values.len())
        .ok_or_else(|| anyhow!("materialized sample count overflows usize"))?;
    let mut pixels = Vec::with_capacity(sample_count);
    for _ in 0..pixel_count {
        pixels.extend_from_slice(values);
    }
    let file = File::create(path)
        .with_context(|| format!("creating materialized TIFF {}", path.display()))?;
    let mut encoder = TiffEncoder::new(file)
        .with_context(|| format!("initializing TIFF encoder for {}", path.display()))?;
    let mut image = encoder
        .new_image::<MaterializedFloat>(grid.nx, grid.ny)
        .with_context(|| format!("creating TIFF image for {}", path.display()))?;
    if values.len() > 1 {
        image
            .extra_samples(&vec![ExtraSamples::Unspecified; values.len() - 1])
            .with_context(|| format!("configuring TIFF bands for {}", path.display()))?;
    }
    image
        .encoder()
        .write_tag(
            Tag::ModelPixelScaleTag,
            &[grid.cell_size, grid.cell_size, 0.0][..],
        )
        .with_context(|| format!("writing ModelPixelScaleTag for {}", path.display()))?;
    image
        .encoder()
        .write_tag(
            Tag::ModelTiepointTag,
            &[0.0, 0.0, 0.0, grid.extent.xmin, grid.extent.ymax, 0.0][..],
        )
        .with_context(|| format!("writing ModelTiepointTag for {}", path.display()))?;
    image
        .encoder()
        .write_tag(
            Tag::GeoKeyDirectoryTag,
            &[
                1_u16, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, epsg,
            ][..],
        )
        .with_context(|| format!("writing GeoKeyDirectoryTag for {}", path.display()))?;
    image
        .write_data(&pixels)
        .with_context(|| format!("writing and finishing TIFF image {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::BufReader;
    use std::path::{Path, PathBuf};

    use hmx_core::manifest::{self, Manifest};
    use hmx_core::readers::cog_reader::read_cog_metadata;
    use hmx_core::types::{Artifact, ArtifactFormat, Crs, FieldId};
    use jsonschema::Validator;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256 as Sha256Hasher};
    use tiff::decoder::{Decoder, DecodingResult};
    use tiff::tags::Tag;

    use crate::derivation::{TestDirectory, copy_declared};

    use super::{MaterializeRequest, execute, preflight_grid, require_supported_band_count};

    const SCALAR_ID: &str = "cell.snow_melt_threshold_c";
    const LAYER_ID: &str = "cell.soil_layer_capacity_mm";

    struct MaterializedPackage {
        _root: TestDirectory,
        base: PathBuf,
        out: PathBuf,
        record: PathBuf,
        bytes: Vec<u8>,
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/derive/base")
    }

    fn copy_base(root: &Path) -> PathBuf {
        let base = root.join("base");
        copy_declared(&fixture(), &base).expect("copy tracked derive base");
        base
    }

    fn materialized(label: &str) -> MaterializedPackage {
        let root = TestDirectory::new(label);
        let base = copy_base(root.path());
        let out = root.path().join("out");
        let record = root.path().join("record.json");
        let bytes = execute(MaterializeRequest::new(
            base.clone(),
            out.clone(),
            Some(record.clone()),
        ))
        .expect("materialize fixture through production path");
        MaterializedPackage {
            _root: root,
            base,
            out,
            record,
            bytes,
        }
    }

    fn identity(field_id: &str) -> (String, String) {
        let digest = format!("{:x}", Sha256Hasher::digest(field_id.as_bytes()));
        (
            format!("parameter.materialized.{digest}"),
            format!("parameter/materialized/{digest}.tif"),
        )
    }

    fn generated<'a>(manifest: &'a Manifest, field_id: &str) -> &'a Artifact {
        manifest
            .artifacts()
            .iter()
            .find(|artifact| artifact.variable.as_ref().map(|v| v.as_str()) == Some(field_id))
            .expect("generated exact variable exists")
    }

    fn record_value(package: &MaterializedPackage) -> Value {
        serde_json::from_slice(&package.bytes).expect("record is JSON")
    }

    fn decoder(path: &Path) -> Decoder<BufReader<File>> {
        Decoder::new(BufReader::new(File::open(path).expect("open TIFF"))).expect("decode TIFF")
    }

    fn schema_validator() -> Validator {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/derive.schema.json");
        let schema: Value = serde_json::from_slice(&fs::read(path).expect("read derive schema"))
            .expect("parse derive schema");
        jsonschema::validator_for(&schema).expect("compile derive schema")
    }

    #[test]
    fn successful_scalar_and_per_layer_conversion_preserves_package_facts() {
        let package = materialized("materialize-success");
        assert_eq!(
            package.bytes,
            fs::read(&package.record).expect("read record")
        );
        assert_eq!(package.bytes.last(), Some(&b'\n'));
        assert_ne!(
            package.bytes.get(package.bytes.len().saturating_sub(2)),
            Some(&b'\n')
        );
        let base = manifest::read(&package.base).expect("read base manifest");
        let out = manifest::read(&package.out).expect("read materialized manifest");
        let record = record_value(&package);
        assert_eq!(out.name().as_str(), "derive-base");
        assert!(out.created_at() > base.created_at());
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(package.out.join("manifest.json")).expect("read manifest JSON")
            )
            .expect("parse manifest JSON")["created_at"],
            record["created_at"]
        );
        assert!(!package.out.join("parameter/scalars.json").exists());
        assert!(
            out.artifacts()
                .iter()
                .all(|artifact| artifact.format != ArtifactFormat::ParameterScalarsV1)
        );
        let generated_artifacts = out
            .artifacts()
            .iter()
            .filter(|artifact| {
                artifact
                    .role
                    .as_str()
                    .starts_with("parameter.materialized.")
            })
            .collect::<Vec<_>>();
        assert_eq!(generated_artifacts.len(), 2);
        for id in [SCALAR_ID, LAYER_ID] {
            let artifact = generated(&out, id);
            let (role, path) = identity(id);
            assert_eq!(artifact.role.as_str(), role);
            assert_eq!(artifact.path.as_str(), path);
        }
        for artifact in base
            .artifacts()
            .iter()
            .filter(|artifact| artifact.format != ArtifactFormat::ParameterScalarsV1)
        {
            let retained = out
                .artifacts()
                .iter()
                .find(|candidate| candidate.role == artifact.role)
                .expect("retained artifact metadata");
            assert_eq!(retained, artifact);
            assert_eq!(
                fs::read(package.base.join(artifact.path.as_str())).expect("read base artifact"),
                fs::read(package.out.join(artifact.path.as_str())).expect("read retained artifact")
            );
        }
        assert!(
            hmx_core::validate::validate(&package.out)
                .expect("validate materialized package")
                .conformant()
        );
    }

    #[test]
    fn decoded_constants_preserve_f64_samples_and_layer_order() {
        let package = materialized("materialize-decode");
        let manifest = manifest::read(&package.out).expect("read output manifest");
        for (id, bands, expected) in [
            (SCALAR_ID, 1_u16, vec![0.5; 4]),
            (
                LAYER_ID,
                3,
                vec![
                    100.0, 150.0, 200.0, 100.0, 150.0, 200.0, 100.0, 150.0, 200.0, 100.0, 150.0,
                    200.0,
                ],
            ),
        ] {
            let path = package.out.join(generated(&manifest, id).path.as_str());
            let mut decoder = decoder(&path);
            assert_eq!(decoder.dimensions().expect("read dimensions"), (2, 2));
            assert_eq!(
                decoder
                    .find_tag_unsigned::<u16>(Tag::SamplesPerPixel)
                    .expect("read SamplesPerPixel")
                    .unwrap_or(1),
                bands
            );
            assert_eq!(
                decoder
                    .get_tag_u16_vec(Tag::BitsPerSample)
                    .expect("read bits"),
                vec![64; usize::from(bands)]
            );
            assert_eq!(
                decoder
                    .get_tag_u16_vec(Tag::SampleFormat)
                    .expect("read formats"),
                vec![3; usize::from(bands)]
            );
            match decoder.read_image().expect("read pixels") {
                DecodingResult::F64(values) => assert_eq!(values, expected),
                other => panic!("expected f64 decoding, got {other:?}"),
            }
        }
    }

    #[test]
    fn generated_tiffs_carry_exact_geospatial_tags() {
        let package = materialized("materialize-geotags");
        let manifest = manifest::read(&package.out).expect("read output manifest");
        for (id, bands) in [(SCALAR_ID, 1), (LAYER_ID, 3)] {
            let path = package.out.join(generated(&manifest, id).path.as_str());
            let metadata = read_cog_metadata(&path).expect("read HMX COG metadata");
            assert_eq!((metadata.width(), metadata.height()), (2, 2));
            assert_eq!(metadata.band_count(), bands);
            assert_eq!(metadata.dtype(), "f64");
            assert_eq!(metadata.crs_epsg(), Some(32645));
            assert_eq!(metadata.pixel_scale(), Some((250.0, 250.0)));
            let mut decoder = decoder(&path);
            assert_eq!(
                decoder
                    .get_tag_f64_vec(Tag::ModelPixelScaleTag)
                    .expect("read pixel scale"),
                vec![250.0, 250.0, 0.0]
            );
            assert_eq!(
                decoder
                    .get_tag_f64_vec(Tag::ModelTiepointTag)
                    .expect("read tiepoint"),
                vec![0.0, 0.0, 0.0, 0.0, 1000.0, 0.0]
            );
            assert_eq!(
                decoder
                    .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
                    .expect("read geokeys"),
                vec![1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, 32645]
            );
        }
    }

    #[test]
    fn manifest_record_digests_sizes_and_content_identity_match_disk() {
        let package = materialized("materialize-digests");
        let base = manifest::read(&package.base).expect("read base manifest");
        let out = manifest::read(&package.out).expect("read output manifest");
        let record = record_value(&package);
        for id in [SCALAR_ID, LAYER_ID] {
            let artifact = generated(&out, id);
            let path = package.out.join(artifact.path.as_str());
            let file_bytes = fs::read(&path).expect("independently read TIFF");
            let digest = format!("{:x}", Sha256Hasher::digest(&file_bytes));
            let size = fs::metadata(&path).expect("independently stat TIFF").len();
            assert_eq!(artifact.sha256.as_str(), digest);
            assert_eq!(artifact.size_bytes, Some(size));
            let addition = record["replaced"]
                .as_array()
                .expect("replacement array")
                .iter()
                .find(|item| item["artifact_role"] == artifact.role.as_str())
                .expect("addition record");
            assert_eq!(addition["new_sha256"], digest);
            assert!(addition["old_sha256"].is_null());
        }
        let scalar = base
            .artifacts()
            .iter()
            .find(|artifact| artifact.format == ArtifactFormat::ParameterScalarsV1)
            .expect("base scalar artifact");
        let removal = record["replaced"]
            .as_array()
            .expect("replacement array")
            .iter()
            .find(|item| item["artifact_role"] == scalar.role.as_str())
            .expect("removal record");
        assert_eq!(removal["old_sha256"], scalar.sha256.as_str());
        assert!(removal["new_sha256"].is_null());
        assert_eq!(
            record["derived_content_hash"]["value"],
            out.content_hash()
                .expect("hash completed manifest")
                .as_str()
        );
    }

    #[test]
    fn actual_record_and_digest_truth_table_match_schema() {
        let package = materialized("materialize-schema");
        let validator = schema_validator();
        let record = record_value(&package);
        assert!(validator.is_valid(&record));
        let replacement = |old: Option<Value>, new: Option<Value>| {
            let mut value = json!({
                "artifact_role": "parameter.test",
                "field_ids": ["cell.test"]
            });
            if let Some(old) = old {
                value["old_sha256"] = old;
            }
            if let Some(new) = new {
                value["new_sha256"] = new;
            }
            let mut candidate = record.clone();
            candidate["replaced"] = json!([value]);
            candidate
        };
        let digest = Value::String("a".repeat(64));
        assert!(validator.is_valid(&replacement(Some(digest.clone()), Some(Value::Null))));
        assert!(validator.is_valid(&replacement(Some(Value::Null), Some(digest.clone()))));
        assert!(!validator.is_valid(&replacement(Some(Value::Null), Some(Value::Null))));
        assert!(!validator.is_valid(&replacement(None, Some(digest.clone()))));
        assert!(!validator.is_valid(&replacement(Some(digest), None)));
        assert_eq!(record["derived_name"], "derive-base");
        assert_eq!(record["non_parameter_overrides"], json!([]));
        assert_eq!(record["tool_version"], env!("CARGO_PKG_VERSION"));
        let replaced = record["replaced"].as_array().expect("replacement array");
        assert!(replaced.windows(2).all(|pair| {
            pair[0]["artifact_role"].as_str().expect("role")
                < pair[1]["artifact_role"].as_str().expect("role")
        }));
        let removal = replaced
            .iter()
            .find(|item| item["artifact_role"] == "parameter.scalars")
            .expect("scalar removal");
        assert_eq!(removal["field_ids"], json!([SCALAR_ID, LAYER_ID]));
    }

    #[test]
    fn conformant_rasters_only_package_is_explicitly_refused_before_staging() {
        let first = materialized("materialize-rasters-only");
        assert!(
            hmx_core::validate::validate(&first.out)
                .expect("validate rasters-only base")
                .conformant()
        );
        let second_out = first._root.path().join("second-out");
        let second_record = first._root.path().join("second-record.json");
        let error = execute(MaterializeRequest::new(
            first.out.clone(),
            second_out.clone(),
            Some(second_record.clone()),
        ))
        .expect_err("rasters-only materialization must fail");
        assert!(format!("{error:#}").contains(
            "materialize requires exactly one hmx/parameter_scalars_v1 artifact; found 0"
        ));
        assert!(!second_out.exists());
        assert!(!second_record.exists());
        assert_no_temporary_paths(first._root.path());
    }

    #[test]
    fn publication_failure_cleans_staging_and_record_temporaries() {
        let root = TestDirectory::new("materialize-cleanup");
        let base = copy_base(root.path());
        let out = root.path().join("out");
        let record_parent = root.path().join("record-parent");
        fs::write(&record_parent, b"regular file").expect("create regular record parent");
        let record = record_parent.join("record.json");
        let error = execute(MaterializeRequest::new(
            base,
            out.clone(),
            Some(record.clone()),
        ))
        .expect_err("record publication must fail through real path");
        assert!(format!("{error:#}").contains("Not a directory"));
        assert!(!out.exists());
        assert!(!record.exists());
        assert_no_temporary_paths(root.path());
    }

    #[test]
    fn preflight_refuses_collisions_crs_and_tiff_short_overflow_without_paths() {
        let root = TestDirectory::new("materialize-preflight");
        let base = copy_base(root.path());
        let out = root.path().join("out");
        let record = root.path().join("record.json");
        let (colliding_role, _) = identity(SCALAR_ID);
        let manifest_path = base.join("manifest.json");
        let mut document: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("read copied manifest"))
                .expect("parse copied manifest");
        document["artifacts"][2]["role"] = Value::String(colliding_role);
        let mut bytes = serde_json::to_vec(&document).expect("serialize collision manifest");
        bytes.push(b'\n');
        fs::write(&manifest_path, bytes).expect("write collision manifest");
        assert!(
            hmx_core::validate::validate(&base)
                .expect("validate collision base")
                .conformant()
        );
        let error = execute(MaterializeRequest::new(
            base,
            out.clone(),
            Some(record.clone()),
        ))
        .expect_err("generated role collision must fail");
        assert!(format!("{error:#}").contains("generated artifact role collision"));
        assert!(!out.exists());
        assert!(!record.exists());
        assert_no_temporary_paths(root.path());

        let manifest = manifest::read(&fixture()).expect("read fixture manifest");
        let mut grid = manifest.grid().clone();
        grid.crs = Crs::new("urn:ogc:def:crs:EPSG::32645");
        assert!(
            format!(
                "{:#}",
                preflight_grid(&grid).expect_err("reject CRS spelling")
            )
            .contains("unsupported materialize grid CRS")
        );
        let overflow =
            require_supported_band_count(&FieldId::new(LAYER_ID), usize::from(u16::MAX) + 1)
                .expect_err("reject TIFF SHORT overflow");
        assert!(format!("{overflow:#}").contains("unsupported layer count 65536"));
    }

    fn assert_no_temporary_paths(root: &Path) {
        let names = fs::read_dir(root)
            .expect("scan shared test root")
            .map(|entry| entry.expect("read root entry").file_name())
            .collect::<Vec<_>>();
        assert!(names.iter().all(|name| {
            let name = name.to_string_lossy();
            !name.contains("hmx-staging") && !name.contains("hmx-record")
        }));
    }
}
