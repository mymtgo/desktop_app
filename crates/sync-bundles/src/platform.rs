//! Which game a bundle is from (cross-device sync spec section 5.1).
//!
//! Match, deck and league bundles carry a top-level `platform` key for
//! Arena and Endstep. An MTGO bundle never has it, so its canonical bytes
//! and hash stay what they were; a bundle without the key is MTGO.

use serde_json::Value;

use crate::deck::BundleError;

/// The platform a bundle was recorded on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BundlePlatform {
    #[default]
    Mtgo,
    Arena,
    Endstep,
}

impl BundlePlatform {
    /// The `platform` key's value, `None` for MTGO (the key is left out).
    pub fn key(&self) -> Option<&'static str> {
        match self {
            BundlePlatform::Mtgo => None,
            BundlePlatform::Arena => Some("arena"),
            BundlePlatform::Endstep => Some("endstep"),
        }
    }

    /// The platform of a bundle: no `platform` key (or null) is MTGO, a
    /// string that names no platform is an error.
    pub fn of_bundle(bundle: &Value) -> Result<BundlePlatform, BundleError> {
        match bundle.get("platform") {
            None | Some(Value::Null) => Ok(BundlePlatform::Mtgo),
            Some(Value::String(key)) => match key.as_str() {
                "arena" => Ok(BundlePlatform::Arena),
                "endstep" => Ok(BundlePlatform::Endstep),
                _ => Err(BundleError::Invalid("platform".to_string())),
            },
            Some(_) => Err(BundleError::Invalid("platform".to_string())),
        }
    }
}

/// Puts the `platform` key on a bundle when the platform has one.
pub fn add_platform(bundle: &mut Value, platform: BundlePlatform) {
    if let (Some(key), Some(fields)) = (platform.key(), bundle.as_object_mut()) {
        fields.insert("platform".to_string(), Value::String(key.to_string()));
    }
}

/// Copies a bundle's `platform` to its sidecar when the bundle has one.
pub fn copy_platform(bundle: &Value, sidecar: &mut Value) {
    let (Some(platform), Some(fields)) = (bundle.get("platform"), sidecar.as_object_mut()) else {
        return;
    };
    if !platform.is_null() {
        fields.insert("platform".to_string(), platform.clone());
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_the_platform_of_a_bundle() {
        assert_eq!(
            BundlePlatform::of_bundle(&json!({})),
            Ok(BundlePlatform::Mtgo)
        );
        assert_eq!(
            BundlePlatform::of_bundle(&json!({"platform": "arena"})),
            Ok(BundlePlatform::Arena)
        );
        assert_eq!(
            BundlePlatform::of_bundle(&json!({"platform": "endstep"})),
            Ok(BundlePlatform::Endstep)
        );
        assert!(BundlePlatform::of_bundle(&json!({"platform": "mtgo"})).is_err());
        assert!(BundlePlatform::of_bundle(&json!({"platform": 3})).is_err());
    }
}
