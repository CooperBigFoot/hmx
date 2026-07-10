//! Typed reader for `hmx/parameter_scalars_v1` JSON artifacts.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use tracing::{debug, instrument};

use crate::CoreError;
use crate::registry::FieldRegistry;
use crate::types::{Extent, FieldId, SemanticRole};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ParameterScalarValueDto {
    Scalar(f64),
    PerLayer(Vec<f64>),
}

/// A parameter value typed according to its registry extent.
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterScalarValue {
    Scalar { value: f64 },
    PerLayer { values: Vec<f64> },
}

impl ParameterScalarValue {
    pub fn as_scalar(&self) -> Option<f64> {
        match self {
            Self::Scalar { value } => Some(*value),
            Self::PerLayer { .. } => None,
        }
    }

    pub fn as_per_layer(&self) -> Option<&[f64]> {
        match self {
            Self::Scalar { .. } => None,
            Self::PerLayer { values } => Some(values),
        }
    }
}

/// Parameter scalars keyed by exact registry field ID.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterScalars {
    values: BTreeMap<FieldId, ParameterScalarValue>,
}

impl ParameterScalars {
    pub fn get(&self, id: &FieldId) -> Option<&ParameterScalarValue> {
        self.values.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&FieldId, &ParameterScalarValue)> {
        self.values.iter()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Reads and types a parameter-scalars artifact using registry metadata.
///
/// # Errors
///
/// Returns [`CoreError`] when the file cannot be read, its local JSON shape is
/// invalid, or a value conflicts with its exact registry field declaration.
#[instrument(skip(registry), fields(path = %path.as_ref().display()))]
pub fn read_parameter_scalars(
    path: impl AsRef<Path>,
    registry: &FieldRegistry,
) -> Result<ParameterScalars, CoreError> {
    let path = path.as_ref();
    let json = std::fs::read_to_string(path).map_err(|error| CoreError::ArtifactUnreadable {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    let raw: BTreeMap<String, ParameterScalarValueDto> =
        serde_json::from_str(&json).map_err(|error| CoreError::InvalidParameterScalarsJson {
            detail: error.to_string(),
        })?;
    if raw.is_empty() {
        return Err(CoreError::InvalidParameterScalarsJson {
            detail: "parameter-scalars object must not be empty".to_string(),
        });
    }

    let values = raw
        .into_iter()
        .map(|(raw_id, value)| parse_value(raw_id, value, registry))
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    debug!(field_count = values.len(), "parsed parameter scalars");
    Ok(ParameterScalars { values })
}

fn parse_value(
    raw_id: String,
    value: ParameterScalarValueDto,
    registry: &FieldRegistry,
) -> Result<(FieldId, ParameterScalarValue), CoreError> {
    let id = FieldId::new(raw_id);
    let field = registry.require(&id)?;
    if field.role() != SemanticRole::Parameter {
        return Err(CoreError::ParameterScalarRole {
            id: id.as_str().to_string(),
            actual: field.role(),
        });
    }

    let typed = match (field.extent(), value) {
        (Extent::Scalar, ParameterScalarValueDto::Scalar(value)) => {
            require_finite(&id, value)?;
            ParameterScalarValue::Scalar { value }
        }
        (Extent::PerLayer, ParameterScalarValueDto::PerLayer(values)) => {
            let expected = field
                .layer_count()
                .ok_or_else(|| CoreError::InvalidLayerCount {
                    id: id.as_str().to_string(),
                    detail: "parsed per_layer field is missing layer_count".to_string(),
                })?
                .get();
            if values.len() != expected {
                return Err(CoreError::ParameterScalarLayerCount {
                    id: id.as_str().to_string(),
                    expected,
                    actual: values.len(),
                });
            }
            for value in &values {
                require_finite(&id, *value)?;
            }
            ParameterScalarValue::PerLayer { values }
        }
        (expected, ParameterScalarValueDto::Scalar(_)) => {
            return Err(CoreError::ParameterScalarExtent {
                id: id.as_str().to_string(),
                expected,
                observed: "number",
            });
        }
        (expected, ParameterScalarValueDto::PerLayer(_)) => {
            return Err(CoreError::ParameterScalarExtent {
                id: id.as_str().to_string(),
                expected,
                observed: "array",
            });
        }
    };

    Ok((id, typed))
}

fn require_finite(id: &FieldId, value: f64) -> Result<(), CoreError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(CoreError::NonFiniteParameterScalar {
            id: id.as_str().to_string(),
        })
    }
}
