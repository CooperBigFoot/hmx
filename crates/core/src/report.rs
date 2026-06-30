//! Validation report vocabulary and verb error types.

use serde::Serialize;

use crate::CoreError;

/// A single HMX 0.1 conformance check id from spec §11.1 plus carried nits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckId {
    /// Manifest parsed and `format_version == "0.1"` (spec §0/§3.3).
    M1,
    /// Manifest has the required closed shape and `package_kind == "input"` (spec §3).
    M2,
    /// Package CRS is present and non-empty (spec §4).
    M3,
    /// Artifact paths obey the full package-relative rule (spec §2.2).
    P1,
    /// Exactly one readable field registry parses as `hmx/field_registry_v1` (spec §6.1-§6.4).
    R1,
    /// Every domain-attribute field id is declared in the registry (spec §6.5).
    R2,
    /// Domain cardinality is single-source and cross-checked where possible (spec §5).
    D1,
    /// Mapping artifact roles resolve to declared mapping encodings (spec §8.3).
    MAP1,
    /// Declared non-registry artifacts open and carry required columns (spec §7).
    F1,
}

impl CheckId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::M1 => "M1",
            Self::M2 => "M2",
            Self::M3 => "M3",
            Self::P1 => "P1",
            Self::R1 => "R1",
            Self::R2 => "R2",
            Self::D1 => "D1",
            Self::MAP1 => "MAP1",
            Self::F1 => "F1",
        }
    }
}

/// Full ordered HMX 0.1 checklist in spec order.
pub const ALL_CHECK_IDS: [CheckId; 9] = [
    CheckId::M1,
    CheckId::M2,
    CheckId::M3,
    CheckId::P1,
    CheckId::R1,
    CheckId::R2,
    CheckId::D1,
    CheckId::MAP1,
    CheckId::F1,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ran,
    Skipped,
}

impl CheckStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ran => "ran",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckResult {
    Pass,
    Fail,
}

impl CheckResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthClass {
    MetadataDeep,
    ByteDeep,
}

impl DepthClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MetadataDeep => "metadata_deep",
            Self::ByteDeep => "byte_deep",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    id: CheckId,
    status: CheckStatus,
    result: Option<CheckResult>,
    depth: DepthClass,
    detail: Option<String>,
}

impl CheckOutcome {
    pub fn ran_pass(id: CheckId, depth: DepthClass) -> Self {
        Self {
            id,
            status: CheckStatus::Ran,
            result: Some(CheckResult::Pass),
            depth,
            detail: None,
        }
    }

    pub fn ran_fail(id: CheckId, depth: DepthClass, detail: impl Into<String>) -> Self {
        Self {
            id,
            status: CheckStatus::Ran,
            result: Some(CheckResult::Fail),
            depth,
            detail: Some(detail.into()),
        }
    }

    pub fn skipped(id: CheckId, depth: DepthClass, reason: impl Into<String>) -> Self {
        Self {
            id,
            status: CheckStatus::Skipped,
            result: None,
            depth,
            detail: Some(reason.into()),
        }
    }

    pub fn id(&self) -> CheckId {
        self.id
    }

    pub fn status(&self) -> CheckStatus {
        self.status
    }

    pub fn result(&self) -> Option<CheckResult> {
        self.result
    }

    pub fn depth(&self) -> DepthClass {
        self.depth
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    checks: Vec<CheckOutcome>,
    conformant: bool,
}

impl ValidationReport {
    pub fn from_outcomes(checks: Vec<CheckOutcome>) -> Self {
        let conformant = !checks
            .iter()
            .any(|check| check.result() == Some(CheckResult::Fail));
        Self { checks, conformant }
    }

    pub fn checks(&self) -> &[CheckOutcome] {
        &self.checks
    }

    pub fn conformant(&self) -> bool {
        self.conformant
    }

    pub fn find(&self, id: CheckId) -> Option<&CheckOutcome> {
        self.checks.iter().find(|check| check.id == id)
    }

    pub fn to_dto(&self) -> ValidationReportDto<'_> {
        ValidationReportDto {
            checks: self.checks.iter().map(CheckOutcomeDto::from).collect(),
            conformant: self.conformant,
        }
    }

    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.to_dto())
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.to_dto())
    }
}

#[derive(Serialize)]
pub struct ValidationReportDto<'a> {
    checks: Vec<CheckOutcomeDto<'a>>,
    conformant: bool,
}

#[derive(Serialize)]
struct CheckOutcomeDto<'a> {
    id: &'a str,
    status: &'a str,
    result: Option<&'a str>,
    depth: &'a str,
    detail: Option<&'a str>,
}

impl<'a> From<&'a CheckOutcome> for CheckOutcomeDto<'a> {
    fn from(check: &'a CheckOutcome) -> Self {
        Self {
            id: check.id.as_str(),
            status: check.status.as_str(),
            result: check.result.map(|result| result.as_str()),
            depth: check.depth.as_str(),
            detail: check.detail.as_deref(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValidateError {
    /// Fires when `<root>/manifest.json` is absent or unreadable.
    #[error("could not read manifest at {path}: {detail}")]
    ManifestUnreadable { path: String, detail: String },
    /// Fires when manifest parsing, the version hard cut, or manifest-local typing fails.
    #[error(transparent)]
    Manifest(#[from] CoreError),
    /// Fires when the assembled validation report cannot be serialized.
    #[error("could not serialize validation report: {detail}")]
    Serialize { detail: String },
}

#[derive(Debug, thiserror::Error)]
pub enum DescribeError {
    /// Fires when `<root>/manifest.json` is absent or unreadable.
    #[error("could not read manifest at {path}: {detail}")]
    ManifestUnreadable { path: String, detail: String },
    /// Fires when manifest parsing, the version hard cut, or manifest-local typing fails.
    #[error(transparent)]
    Manifest(#[from] CoreError),
    /// Fires when a declared field registry artifact is unreadable or malformed.
    #[error("could not read field registry at {path}: {detail}")]
    Registry { path: String, detail: String },
    /// Fires when the assembled description cannot be serialized.
    #[error("could not serialize description: {detail}")]
    Serialize { detail: String },
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::report::{
        CheckId, CheckOutcome, CheckResult, DepthClass, ValidationReport,
    };

    #[test]
    fn from_outcomes_fails_closed_on_any_ran_fail() {
        let report = ValidationReport::from_outcomes(vec![
            CheckOutcome::ran_pass(CheckId::M1, DepthClass::MetadataDeep),
            CheckOutcome::ran_fail(CheckId::P1, DepthClass::MetadataDeep, "bad path"),
        ]);
        assert!(!report.conformant());
    }

    #[test]
    fn from_outcomes_stays_true_for_passes_and_skips() {
        let report = ValidationReport::from_outcomes(vec![
            CheckOutcome::ran_pass(CheckId::M1, DepthClass::MetadataDeep),
            CheckOutcome::skipped(CheckId::R2, DepthClass::MetadataDeep, "no registry"),
        ]);
        assert!(report.conformant());
    }

    #[test]
    fn skipped_has_no_result_and_non_null_detail() {
        let outcome = CheckOutcome::skipped(CheckId::R2, DepthClass::MetadataDeep, "no registry");
        assert_eq!(outcome.result(), None);
        assert_eq!(outcome.detail(), Some("no registry"));
    }

    #[test]
    fn dto_serializes_skipped_result_as_null() {
        let report = ValidationReport::from_outcomes(vec![CheckOutcome::skipped(
            CheckId::R2,
            DepthClass::MetadataDeep,
            "no registry",
        )]);
        let json = report.to_json_string().expect("report serializes");
        let value: Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(value["checks"][0]["result"], Value::Null);
        assert_eq!(report.checks()[0].result(), None);
        assert_ne!(report.checks()[0].result(), Some(CheckResult::Fail));
    }
}
