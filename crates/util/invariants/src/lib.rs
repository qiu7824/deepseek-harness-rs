//! Configurable registry for package-owned runtime invariant contributions.
//! Rust port of `@deepseek-ai/dsh-invariants`.

use std::collections::HashSet;
use std::sync::Arc;

use cordis::{ArcValue, BoxFuture, Context, InjectSpec, Plugin, PluginError, Service, arc};
use parking_lot::Mutex;
use regex::Regex;

/// Runtime invariant selection configured on the service plugin.
#[derive(Debug, Clone, Default)]
pub struct InvariantConfig {
    /// Global switch; defaults to `true`.
    pub enabled: bool,
    /// Case-sensitive regex sources that admit package names; empty admits
    /// all.
    pub package_allowlist: Vec<String>,
    /// Case-sensitive regex sources that exclude package names after
    /// allowlist matching.
    pub package_blocklist: Vec<String>,
}

/// Thrown when a package-owned runtime invariant is violated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantError {
    /// Stable machine-readable invariant failure code.
    pub code: &'static str,
    /// Full package name that owns the violated invariant.
    pub package_name: String,
    /// Violated contract, without the standard error prefix.
    pub message: String,
}

impl InvariantError {
    pub fn new(package_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: "INVARIANT",
            package_name: package_name.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for InvariantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invariant violated by \"{}\": {}",
            self.package_name, self.message
        )
    }
}

impl std::error::Error for InvariantError {}

/// One package's invariant installer (TS `InvariantInstaller`).
pub struct InvariantInstaller {
    /// Runs the package contribution in a child context; `fail` reports a
    /// violation bound to the registering package name. The failure channel
    /// is owned so installers may move it into spawned listeners.
    pub install: Arc<
        dyn Fn(&Context, Arc<dyn Fn(&str) + Send + Sync>) -> BoxFuture<'static, ()> + Send + Sync,
    >,
    /// Services the child installer fiber may access.
    pub inject: Option<InjectSpec>,
}

/// Compile and validate one package-filter list (TS `compilePatterns`).
fn compile_patterns(field: &str, values: &[String]) -> Vec<Regex> {
    let mut seen = HashSet::new();
    values
        .iter()
        .map(|value| {
            if value.is_empty() || value.trim() != value {
                panic!("invariants: {field} entries must be non-blank and have no surrounding whitespace");
            }
            if !seen.insert(value.clone()) {
                panic!("invariants: {field} contains duplicate regex {value:?}");
            }
            Regex::new(value).unwrap_or_else(|cause| {
                panic!("invariants: {field} contains invalid regex {value:?}: {cause}")
            })
        })
        .collect()
}

/// Package-owned invariant registry with global and regex-based selection
/// (TS `InvariantRegistry`).
pub struct InvariantRegistry {
    enabled: bool,
    owner_ctx: Context,
    package_allowlist: Vec<Regex>,
    package_blocklist: Vec<Regex>,
    registrations: Arc<Mutex<HashSet<String>>>,
}

impl InvariantRegistry {
    /// Create the registry service (register it under `invariants`).
    pub fn new(ctx: &Context, config: InvariantConfig) -> Arc<Self> {
        let service = Arc::new(Self {
            enabled: config.enabled,
            owner_ctx: ctx.clone(),
            package_allowlist: compile_patterns("package_allowlist", &config.package_allowlist),
            package_blocklist: compile_patterns("package_blocklist", &config.package_blocklist),
            registrations: Arc::new(Mutex::new(HashSet::new())),
        });
        ctx.register_service(service.clone());
        service
    }

    /// Return whether one full package name passes the configured filters.
    fn selected(&self, package_name: &str) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.package_allowlist.is_empty()
            && !self
                .package_allowlist
                .iter()
                .any(|pattern| pattern.is_match(package_name))
        {
            return false;
        }
        !self
            .package_blocklist
            .iter()
            .any(|pattern| pattern.is_match(package_name))
    }

    /// Register one package's invariant installer (TS `register`; the
    /// caller context is explicit — the TS Proxy rebinds `this.ctx.effect`
    /// and the child plugin's parent to the caller's fiber).
    ///
    /// The package name is reserved even when filtering disables its checks.
    /// Enabled installers run in a child fiber; failure disposes that fiber
    /// and releases the reservation. Unloading the caller's fiber releases
    /// the reservation and disposes the child.
    pub fn register(
        &self,
        caller: &Context,
        package_name: &str,
        installer: InvariantInstaller,
    ) -> cordis::Disposer {
        if package_name.is_empty()
            || package_name.trim() != package_name
            || package_name.chars().any(|ch| ch.is_whitespace())
        {
            panic!("invariants: packageName must be non-blank and contain no whitespace");
        }
        {
            let mut registrations = self.registrations.lock();
            if registrations.contains(package_name) {
                panic!("invariants: package \"{package_name}\" is already registered");
            }
            registrations.insert(package_name.to_string());
        }

        if !self.selected(package_name) {
            let registrations = self.registrations.clone();
            let package_name = package_name.to_string();
            let cleanup = cordis::make_disposer(move || {
                let registrations = registrations.clone();
                let package_name = package_name.clone();
                Box::pin(async move {
                    registrations.lock().remove(&package_name);
                })
            });
            // The caller fiber owns the reservation release.
            let caller = caller.clone();
            let effect_cleanup = cleanup.clone();
            let _ = caller.effect(
                "invariants.register()",
                Box::pin(async move { Some(effect_cleanup) }),
            );
            return cleanup;
        }

        // Enabled: start the installer in a child fiber owned by the
        // CALLER's context (TS `ctx.plugin` with the rebound caller ctx).
        let owner = caller.clone();
        let package_name_owned = package_name.to_string();
        let install = installer.install;
        let inject = installer.inject;
        let registrations = self.registrations.clone();
        let child = {
            let install = install.clone();
            let package_name = package_name_owned.clone();
            owner.plugin(
                Arc::new(InstallerPlugin {
                    install,
                    package_name: package_name.clone(),
                    inject: inject.clone(),
                }),
                arc(()),
            )
        };

        // Settle the child; failure disposes it and releases the reservation
        // (TS `await child` catch path).
        {
            let child_for_settle = child.clone();
            let registrations_for_fail = registrations.clone();
            let package_name_for_fail = package_name_owned.clone();
            tokio::spawn(async move {
                if let Err(error) = child_for_settle.settle().await {
                    child_for_settle.dispose().await;
                    registrations_for_fail.lock().remove(&package_name_for_fail);
                    tracing::error!(
                        "invariant installer for {package_name_for_fail} failed: {}",
                        error.message()
                    );
                }
            });
        }

        let cleanup = cordis::make_disposer(move || {
            let child = child.clone();
            let registrations = registrations.clone();
            let package_name = package_name_owned.clone();
            Box::pin(async move {
                child.dispose().await;
                registrations.lock().remove(&package_name);
            })
        });
        // The caller fiber owns the reservation release (TS `ctx.effect`).
        let caller = caller.clone();
        let effect_cleanup = cleanup.clone();
        let _ = caller.effect(
            "invariants.register()",
            Box::pin(async move { Some(effect_cleanup) }),
        );
        cleanup
    }
}

impl Service for InvariantRegistry {
    fn service_name(&self) -> &'static str {
        "invariants"
    }
}

struct InstallerPlugin {
    install:
        Arc<dyn Fn(&Context, Arc<dyn Fn(&str) + Send + Sync>) -> BoxFuture<'static, ()> + Send + Sync>,
    package_name: String,
    inject: Option<InjectSpec>,
}

#[async_trait::async_trait]
impl Plugin for InstallerPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("invariant-installer")
    }

    fn inject(&self) -> InjectSpec {
        self.inject.clone().unwrap_or_default()
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let package_name = self.package_name.clone();
        let fail: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |message: &str| {
            panic!("{}", InvariantError::new(package_name.clone(), message.to_string()));
        });
        (self.install)(ctx, fail).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn error_format() {
        let error = InvariantError::new("@deepseek-ai/x", "contract broken");
        assert_eq!(error.code, "INVARIANT");
        assert_eq!(error.to_string(), "invariant violated by \"@deepseek-ai/x\": contract broken");
    }

    #[test]
    fn pattern_validation() {
        assert!(compile_patterns("x", &["^ok$".to_string()]).len() == 1);
        assert!(std::panic::catch_unwind(|| {
            compile_patterns("x", &[" has spaces ".to_string()])
        })
        .is_err());
        assert!(std::panic::catch_unwind(|| {
            compile_patterns("x", &["(".to_string()])
        })
        .is_err());
    }

    #[tokio::test]
    async fn register_runs_and_filters_installers() {
        let ctx = Context::root();
        let registry = InvariantRegistry::new(&ctx, InvariantConfig {
            enabled: true,
            package_allowlist: vec!["^@ok/".to_string()],
            package_blocklist: vec![],
        });

        let runs = Arc::new(AtomicU32::new(0));
        let runs2 = runs.clone();
        let installer = InvariantInstaller {
            install: Arc::new(move |_ctx, _fail| {
                let runs = runs2.clone();
                Box::pin(async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                })
            }),
            inject: None,
        };
        let dispose = registry.register(&ctx, "@ok/pkg", installer);
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // allowlist excludes this package: reservation only
        let skipped = Arc::new(AtomicU32::new(0));
        let skipped2 = skipped.clone();
        let installer2 = InvariantInstaller {
            install: Arc::new(move |_ctx, _fail| {
                let skipped = skipped2.clone();
                Box::pin(async move {
                    skipped.fetch_add(1, Ordering::SeqCst);
                })
            }),
            inject: None,
        };
        let dispose2 = registry.register(&ctx, "@other/pkg", installer2);
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(skipped.load(Ordering::SeqCst), 0);

        // duplicate registration panics
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registry.register(&ctx, "@ok/pkg", InvariantInstaller {
                install: Arc::new(|_ctx, _fail| Box::pin(async {})),
                inject: None,
            });
        }))
        .is_err());

        dispose().await;
        dispose2().await;
    }
}
