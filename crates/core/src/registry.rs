//! Typed field-registry boundary parse for the JSON `hmx/field_registry_v1` artifact.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::Deserialize;
use tracing::{debug, instrument, warn};

use crate::CoreError;
use crate::types::{
    ConservationClass, DomainId, Extent, FieldId, FieldTimeMeaning, Quantity, SemanticRole, Units,
    ValueType,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldRegistryDto {
    registry_version: String,
    fields: Vec<FieldDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldDto {
    id: String,
    domain: String,
    quantity: String,
    units: String,
    value_type: String,
    time_meaning: String,
    role: String,
    conservation_class: String,
    extent: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryVersion {
    V1,
}

impl RegistryVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V1 => "1",
        }
    }
}

impl FromStr for RegistryVersion {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "1" => Ok(Self::V1),
            other => Err(CoreError::UnknownRegistryVersion {
                found: other.to_string(),
            }),
        }
    }
}

/// One field's typed registry spec (spec §6.2): every attribute parsed into a
/// domain type at the boundary. Constructed only by the registry parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    id: FieldId,
    domain: DomainId,
    quantity: Quantity,
    units: Units,
    value_type: ValueType,
    time_meaning: FieldTimeMeaning,
    role: SemanticRole,
    conservation_class: ConservationClass,
    extent: Extent,
}

impl FieldSpec {
    pub fn id(&self) -> &FieldId {
        &self.id
    }

    pub fn domain(&self) -> &DomainId {
        &self.domain
    }

    pub fn quantity(&self) -> &Quantity {
        &self.quantity
    }

    pub fn units(&self) -> &Units {
        &self.units
    }

    pub fn value_type(&self) -> ValueType {
        self.value_type
    }

    pub fn time_meaning(&self) -> FieldTimeMeaning {
        self.time_meaning
    }

    pub fn role(&self) -> SemanticRole {
        self.role
    }

    pub fn conservation_class(&self) -> ConservationClass {
        self.conservation_class
    }

    pub fn extent(&self) -> Extent {
        self.extent
    }
}

/// The typed field registry (spec §6): a `FieldId`-keyed map of `FieldSpec`,
/// parsed from the `hmx/field_registry_v1` JSON artifact. Duplicate ids are
/// rejected at construction. The map is `BTreeMap` for deterministic iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRegistry {
    registry_version: RegistryVersion,
    fields: BTreeMap<FieldId, FieldSpec>,
}

impl FieldRegistry {
    /// Parses a raw field-registry JSON string into typed domain values.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when JSON structure is invalid, the registry
    /// version hard cut fails, a field value is not representable, or duplicate
    /// field ids are declared.
    #[instrument(skip(json))]
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        let dto: FieldRegistryDto = serde_json::from_str(json).map_err(map_serde_error)?;
        let registry_version: RegistryVersion = dto.registry_version.parse()?;
        let fields = dto
            .fields
            .into_iter()
            .map(parse_field)
            .collect::<Result<Vec<_>, _>>()?;
        let registry = build_registry(registry_version, fields)?;

        debug!(field_count = registry.len(), "parsed field registry");
        Ok(registry)
    }

    pub fn registry_version(&self) -> RegistryVersion {
        self.registry_version
    }

    pub fn get(&self, id: &FieldId) -> Option<&FieldSpec> {
        self.fields.get(id)
    }

    /// Requires a declared field id, rejecting undeclared model-consumed fields
    /// per spec §6.5 (F8/F19 input-completeness gate).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::UndeclaredField`] when `id` is absent.
    pub fn require(&self, id: &FieldId) -> Result<&FieldSpec, CoreError> {
        self.get(id).ok_or_else(|| CoreError::UndeclaredField {
            id: id.as_str().to_string(),
        })
    }

    pub fn contains(&self, id: &FieldId) -> bool {
        self.fields.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FieldSpec> {
        self.fields.values()
    }
}

fn parse_field(dto: FieldDto) -> Result<FieldSpec, CoreError> {
    Ok(FieldSpec {
        id: FieldId::new(require_non_empty(dto.id, "field.id")?),
        domain: DomainId::new(require_non_empty(dto.domain, "field.domain")?),
        quantity: Quantity::new(require_non_empty(dto.quantity, "field.quantity")?),
        units: Units::new(require_non_empty(dto.units, "field.units")?),
        value_type: dto.value_type.parse::<ValueType>()?,
        time_meaning: dto.time_meaning.parse::<FieldTimeMeaning>()?,
        role: dto.role.parse::<SemanticRole>()?,
        conservation_class: dto.conservation_class.parse::<ConservationClass>()?,
        extent: dto.extent.parse::<Extent>()?,
    })
}

fn require_non_empty(value: String, field: &'static str) -> Result<String, CoreError> {
    if value.is_empty() {
        warn!(field, "rejecting empty required registry field");
        return Err(CoreError::EmptyField { field });
    }
    Ok(value)
}

fn build_registry(
    registry_version: RegistryVersion,
    fields: Vec<FieldSpec>,
) -> Result<FieldRegistry, CoreError> {
    let mut by_id = BTreeMap::new();

    for field in fields {
        let id = field.id().clone();
        if by_id.insert(id.clone(), field).is_some() {
            return Err(CoreError::DuplicateFieldId {
                id: id.as_str().to_string(),
            });
        }
    }

    Ok(FieldRegistry {
        registry_version,
        fields: by_id,
    })
}

fn map_serde_error(err: serde_json::Error) -> CoreError {
    let message = err.to_string();

    if let Some(field) = extract_backticked(&message, "missing field") {
        warn!(field = %field, "rejecting registry with a missing required field");
        return CoreError::MissingRegistryField { field };
    }
    if let Some(field) = extract_backticked(&message, "unknown field") {
        warn!(field = %field, "rejecting registry with an unexpected field");
        return CoreError::ExtraRegistryField { field };
    }

    warn!(error = %message, "rejecting unparsable registry JSON");
    CoreError::InvalidRegistryJson { detail: message }
}

fn extract_backticked(message: &str, prefix: &str) -> Option<String> {
    if !message.starts_with(prefix) {
        return None;
    }
    let after = message.find('`')? + 1;
    let len = message[after..].find('`')?;
    Some(message[after..after + len].to_string())
}

#[cfg(test)]
mod tests {
    use crate::CoreError;
    use crate::registry::{FieldRegistry, RegistryVersion};
    use crate::types::{
        ConservationClass, Extent, FieldId, FieldTimeMeaning, SemanticRole, ValueType,
    };

    const VALID_REGISTRY: &str =
        include_str!("../../../schemas/examples/field_registry.valid.json");
    const INVALID_UNKNOWN_ROLE: &str =
        include_str!("../../../schemas/examples/field_registry.invalid-unknown-role.json");
    const INVALID_MISSING_CONSERVATION_CLASS: &str = include_str!(
        "../../../schemas/examples/field_registry.invalid-missing-conservation-class.json"
    );

    #[test]
    fn valid_json_round_trips() {
        let registry = parse_valid();

        assert_eq!(registry.registry_version(), RegistryVersion::V1);
        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
        assert!(registry.contains(&FieldId::new("cells.flow_dir")));
        assert_eq!(registry.iter().count(), 3);

        let glacier = registry
            .get(&FieldId::new("glacier.ice_volume_m3"))
            .unwrap_or_else(|| panic!("expected glacier.ice_volume_m3"));
        assert_eq!(glacier.id().as_str(), "glacier.ice_volume_m3");
        assert_eq!(glacier.domain().as_str(), "glacier");
        assert_eq!(glacier.quantity().as_str(), "volume");
        assert_eq!(glacier.units().as_str(), "m3");
        assert_eq!(glacier.role(), SemanticRole::DifferentialState);
        assert_eq!(
            glacier.conservation_class(),
            ConservationClass::WaterVolume
        );
        assert_eq!(glacier.value_type(), ValueType::F64);
        assert_eq!(glacier.time_meaning(), FieldTimeMeaning::Instant);
        assert_eq!(glacier.extent(), Extent::Scalar);

        let flow_dir = registry
            .get(&FieldId::new("cells.flow_dir"))
            .unwrap_or_else(|| panic!("expected cells.flow_dir"));
        assert_eq!(flow_dir.role(), SemanticRole::Parameter);
        assert_eq!(flow_dir.conservation_class(), ConservationClass::None);
        assert_eq!(flow_dir.value_type(), ValueType::I32);
    }

    #[test]
    fn unknown_role_rejected() {
        match FieldRegistry::from_json(INVALID_UNKNOWN_ROLE).unwrap_err() {
            CoreError::InvalidEnumValue { field, found } => {
                assert_eq!(field, "role");
                assert_eq!(found, "wizard");
            }
            other => panic!("expected InvalidEnumValue, got {other:?}"),
        }
    }

    #[test]
    fn missing_conservation_class_rejected() {
        match FieldRegistry::from_json(INVALID_MISSING_CONSERVATION_CLASS).unwrap_err() {
            CoreError::MissingRegistryField { field } => assert_eq!(field, "conservation_class"),
            other => panic!("expected MissingRegistryField, got {other:?}"),
        }
    }

    #[test]
    fn empty_conservation_class_rejected() {
        match parse_err(replace_once(
            r#""conservation_class": "water_volume""#,
            r#""conservation_class": """#,
        )) {
            CoreError::InvalidEnumValue { field, found } => {
                assert_eq!(field, "conservation_class");
                assert_eq!(found, "");
            }
            other => panic!("expected InvalidEnumValue, got {other:?}"),
        }
    }

    #[test]
    fn unknown_value_type_rejected() {
        match parse_err(replace_once(r#""value_type": "f64""#, r#""value_type": "f128""#)) {
            CoreError::InvalidEnumValue { field, found } => {
                assert_eq!(field, "value_type");
                assert_eq!(found, "f128");
            }
            other => panic!("expected InvalidEnumValue, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_field_id_rejected() {
        let json = replace_once(
            r#""id": "cells.flow_dir""#,
            r#""id": "cells.snow_water_equivalent_m3""#,
        );

        match parse_err(json) {
            CoreError::DuplicateFieldId { id } => {
                assert_eq!(id, "cells.snow_water_equivalent_m3");
            }
            other => panic!("expected DuplicateFieldId, got {other:?}"),
        }
    }

    #[test]
    fn unknown_registry_version_rejected() {
        match parse_err(replace_once(
            r#""registry_version": "1""#,
            r#""registry_version": "2""#,
        )) {
            CoreError::UnknownRegistryVersion { found } => assert_eq!(found, "2"),
            other => panic!("expected UnknownRegistryVersion, got {other:?}"),
        }
    }

    #[test]
    fn registry_version_cut_wins_over_field_error() {
        let json = replace_once(r#""registry_version": "1""#, r#""registry_version": "9""#);
        let json = json.replacen(r#""role": "parameter""#, r#""role": "wizard""#, 1);

        match parse_err(json) {
            CoreError::UnknownRegistryVersion { found } => assert_eq!(found, "9"),
            other => panic!("expected UnknownRegistryVersion, got {other:?}"),
        }
    }

    #[test]
    fn extra_field_key_rejected() {
        match parse_err(replace_once(
            r#""extent": "scalar" }"#,
            r#""extent": "scalar", "bogus": 1 }"#,
        )) {
            CoreError::ExtraRegistryField { field } => assert_eq!(field, "bogus"),
            other => panic!("expected ExtraRegistryField, got {other:?}"),
        }
    }

    #[test]
    fn empty_field_id_rejected() {
        match parse_err(replace_once(
            r#""id": "cells.snow_water_equivalent_m3""#,
            r#""id": """#,
        )) {
            CoreError::EmptyField { field } => assert_eq!(field, "field.id"),
            other => panic!("expected EmptyField, got {other:?}"),
        }
    }

    #[test]
    fn undeclared_field_required_rejects() {
        let registry = parse_valid();
        assert!(
            registry
                .require(&FieldId::new("cells.snow_water_equivalent_m3"))
                .is_ok()
        );

        match registry
            .require(&FieldId::new("cells.does_not_exist"))
            .unwrap_err()
        {
            CoreError::UndeclaredField { id } => assert_eq!(id, "cells.does_not_exist"),
            other => panic!("expected UndeclaredField, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_rejects_without_panic() {
        assert!(FieldRegistry::from_json("{ not json }").is_err());
    }

    fn parse_valid() -> FieldRegistry {
        FieldRegistry::from_json(VALID_REGISTRY).unwrap_or_else(|err| {
            panic!("expected valid registry to parse, got {err:?}");
        })
    }

    fn parse_err(json: String) -> CoreError {
        FieldRegistry::from_json(&json).unwrap_err()
    }

    fn replace_once(from: &str, to: &str) -> String {
        VALID_REGISTRY.replacen(from, to, 1)
    }
}
