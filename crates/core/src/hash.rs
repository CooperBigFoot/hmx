//! Package content-hash API (spec §9, D1).

use std::fmt;
use std::fmt::Write;

use sha2::{Digest, Sha256};

use crate::CoreError;
use crate::canonical;
use crate::manifest::Manifest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn hash_algo(&self) -> &'static str {
        "sha256"
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn content_hash(manifest: &Manifest) -> Result<ContentHash, CoreError> {
    let bytes = canonical::canonical_bytes(manifest)?;
    let digest = Sha256::digest(&bytes);
    Ok(ContentHash(to_lower_hex(&digest)))
}

fn to_lower_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::canonical;
    use crate::hash::content_hash;
    use crate::manifest::{Manifest, read};

    const VALID: &str = r#"{
  "format_version": "0.1",
  "name": "synthetic-glacier-mini",
  "created_at": "2026-06-29T00:00:00Z",
  "producer": "hmx-core-a7-test",
  "producer_version": "0.1.3",
  "package_kind": "input",
  "crs": "EPSG:32645",
  "grid": {
    "crs": "EPSG:32645",
    "extent": { "xmin": 0.0, "ymin": 0.0, "xmax": 1000.0, "ymax": 1000.0 },
    "cell_size": 250.0,
    "nx": 4,
    "ny": 4,
    "origin": "upper_left"
  },
  "domains": [
    { "id": "cell", "entity_count": 16, "index_base": "dense_zero_based" },
    { "id": "glacier", "entity_count": 3, "index_base": "dense_zero_based", "external_ids": [1, 2, 2001] }
  ],
  "mappings": [
    { "purpose": "cell_to_glacier", "source_domain": "cell", "target_domain": "glacier", "artifact_role": "mapping.cell_to_glacier" }
  ],
  "artifacts": [
    { "role": "registry.fields", "path": "registry/fields.json", "format": "hmx/field_registry_v1", "sha256": "0000000000000000000000000000000000000000000000000000000000000000", "size_bytes": 512 }
  ]
}"#;

    #[test]
    fn content_hash_is_deterministic_lowercase_sha256() {
        let manifest_a = Manifest::from_json(VALID).unwrap();
        let manifest_b = Manifest::from_json(VALID).unwrap();

        let hash_a = manifest_a.content_hash().unwrap();
        let hash_b = manifest_b.content_hash().unwrap();
        let hash_a_again = content_hash(&manifest_a).unwrap();

        assert_eq!(hash_a, hash_b);
        assert_eq!(hash_a, hash_a_again);
        assert_eq!(hash_a.as_str().len(), 64);
        assert!(hash_a.as_str().chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(hash_a.hash_algo(), "sha256");
    }

    #[test]
    fn content_hash_is_path_independent() {
        let root = std::env::temp_dir();
        let suffix = unique_suffix();
        let dir_a = root.join(format!("hmx-a7-a-{suffix}"));
        let dir_b = root.join(format!("hmx-a7-b-{suffix}"));

        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        fs::write(dir_a.join("manifest.json"), VALID).unwrap();
        fs::write(dir_b.join("manifest.json"), VALID).unwrap();

        let hash_a = read(&dir_a).unwrap().content_hash().unwrap();
        let hash_b = read(&dir_b).unwrap().content_hash().unwrap();

        fs::remove_dir_all(&dir_a).unwrap();
        fs::remove_dir_all(&dir_b).unwrap();

        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn content_hash_changes_when_artifact_metadata_changes() {
        let base = Manifest::from_json(VALID).unwrap().content_hash().unwrap();
        let changed_sha = VALID.replace(
            r#""sha256": "0000000000000000000000000000000000000000000000000000000000000000""#,
            r#""sha256": "1111111111111111111111111111111111111111111111111111111111111111""#,
        );
        let changed_size = VALID.replace(r#""size_bytes": 512"#, r#""size_bytes": 1024"#);

        assert_ne!(
            base,
            Manifest::from_json(&changed_sha)
                .unwrap()
                .content_hash()
                .unwrap()
        );
        assert_ne!(
            base,
            Manifest::from_json(&changed_size)
                .unwrap()
                .content_hash()
                .unwrap()
        );
    }

    #[test]
    fn canonical_bytes_are_compact_and_sort_object_keys() {
        let manifest = Manifest::from_json(VALID).unwrap();
        let canonical = String::from_utf8(canonical::canonical_bytes(&manifest).unwrap()).unwrap();

        assert!(!canonical.contains(": "));
        assert!(!canonical.contains('\n'));
        assert!(index_of(&canonical, r#""artifacts""#) < index_of(&canonical, r#""created_at""#));
        assert!(index_of(&canonical, r#""created_at""#) < index_of(&canonical, r#""crs""#));
        assert!(index_of(&canonical, r#""crs""#) < index_of(&canonical, r#""domains""#));
        assert!(
            index_of(&canonical, r#""artifact_role""#) < index_of(&canonical, r#""purpose""#)
        );
    }

    fn unique_suffix() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{}-{nanos}", std::process::id())
    }

    fn index_of(haystack: &str, needle: &str) -> usize {
        haystack.find(needle).unwrap()
    }
}
