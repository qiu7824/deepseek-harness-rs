//! Immutable launch-time environment snapshot that records which layer
//! supplied each value. Rust port of
//! `packages/util/launch-environment/src/index.ts`. Harness consumers resolve
//! through it instead of a flattened `std::env`; launchers may still
//! materialize accepted values for config expressions and third-party
//! libraries.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::Context;

/// Which layer supplied a value, from most to least trusted: the environment
/// this process inherited, the invoking directory's `.env`, the Harness
/// home's `.env` (TS `LaunchEnvironmentSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaunchEnvironmentSource {
    Process,
    ProjectEnv,
    UserEnv,
}

impl LaunchEnvironmentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            LaunchEnvironmentSource::Process => "process",
            LaunchEnvironmentSource::ProjectEnv => "project-env",
            LaunchEnvironmentSource::UserEnv => "user-env",
        }
    }
}

/// Layer order, most trusted first.
const SOURCE_ORDER: [LaunchEnvironmentSource; 3] = [
    LaunchEnvironmentSource::Process,
    LaunchEnvironmentSource::ProjectEnv,
    LaunchEnvironmentSource::UserEnv,
];

/// One resolved variable and the layer it came from (TS
/// `LaunchEnvironmentEntry`).
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchEnvironmentEntry {
    /// The value as the layer supplied it; may be empty, which each owner
    /// judges for itself.
    pub value: String,
    /// The layer that supplied it.
    pub source: LaunchEnvironmentSource,
    /// Absolute path of the file that supplied it; absent for `process`.
    pub path: Option<String>,
}

/// The frozen environment of one launch. Construct through
/// [`create_launch_environment_snapshot`]; nothing mutates it afterwards, so
/// a later `chdir`, workspace switch, or resumed session observes the same
/// values a consumer resolved at boot.
#[derive(Debug, Clone)]
pub struct LaunchEnvironmentSnapshot {
    by_source: HashMap<LaunchEnvironmentSource, Layer>,
}

#[derive(Debug, Clone)]
struct Layer {
    path: Option<String>,
    values: HashMap<String, String>,
}

impl LaunchEnvironmentSnapshot {
    /// Resolve one name across every layer, most trusted first.
    pub fn get(&self, name: &str) -> Option<LaunchEnvironmentEntry> {
        self.get_from(name, &SOURCE_ORDER)
    }

    /// Resolve one name only from `sources`, retaining canonical trust
    /// order; omitted layers are unreachable.
    pub fn get_from(
        &self,
        name: &str,
        sources: &[LaunchEnvironmentSource],
    ) -> Option<LaunchEnvironmentEntry> {
        let key = lookup_key(name);
        for source in SOURCE_ORDER {
            if !sources.contains(&source) {
                continue;
            }
            let Some(layer) = self.by_source.get(&source) else {
                continue;
            };
            let Some(value) = layer.values.get(&key) else {
                continue;
            };
            return Some(LaunchEnvironmentEntry {
                value: value.clone(),
                source,
                path: layer.path.clone(),
            });
        }
        None
    }
}

/// The map key one variable name resolves under. Windows treats environment
/// names case-insensitively; every other platform does not.
fn lookup_key(name: &str) -> String {
    #[cfg(windows)]
    {
        name.to_uppercase()
    }
    #[cfg(not(windows))]
    {
        name.to_string()
    }
}

/// One layer's raw contents, as [`create_launch_environment_snapshot`]
/// receives them (TS `LaunchEnvironmentLayerInput`).
#[derive(Debug, Clone)]
pub struct LaunchEnvironmentLayerInput {
    pub source: LaunchEnvironmentSource,
    /// Absolute path of the file behind this layer; omit for `process`.
    pub path: Option<String>,
    pub values: Vec<(String, String)>,
}

/// Build the snapshot from each layer's contents.
///
/// The layers may arrive in any order; the result searches them by canonical
/// trust order. Every layer is copied so later mutations cannot change the
/// snapshot; names fold on Windows so case variants cannot split precedence.
pub fn create_launch_environment_snapshot(
    layers: &[LaunchEnvironmentLayerInput],
) -> Arc<LaunchEnvironmentSnapshot> {
    let mut by_source = HashMap::new();
    for layer in layers {
        let values = layer
            .values
            .iter()
            .map(|(name, value)| (lookup_key(name), value.clone()))
            .collect();
        by_source.insert(
            layer.source,
            Layer {
                path: layer.path.clone(),
                values,
            },
        );
    }
    Arc::new(LaunchEnvironmentSnapshot { by_source })
}

/// Context slot the launcher fills with this run's snapshot before any
/// config entry mounts.
pub const DSH_LAUNCH_ENVIRONMENT_KEY: &str = "launchEnvironment";

/// Return the launcher's snapshot, or the inherited environment as the sole
/// layer when the host provided none.
pub fn launch_environment_of(ctx: &Context) -> Arc<LaunchEnvironmentSnapshot> {
    let provided = ctx
        .get_typed::<Arc<LaunchEnvironmentSnapshot>>(DSH_LAUNCH_ENVIRONMENT_KEY, false)
        .map(|slot| (*slot).clone());
    provided.unwrap_or_else(|| {
        create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::Process,
            path: None,
            values: std::env::vars().collect(),
        }])
    })
}
