//! Rust port of `packages/host/plugin-inventory/tests/invariant.spec.ts`:
//! the package-owned empty installer registers cleanly and can re-register
//! after teardown.

use dsh_host_plugin_inventory::invariant::apply;
use dsh_invariants::{InvariantConfig, InvariantRegistry};

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

#[test]
fn registers_the_package_owned_empty_installer() {
    run(async {
        let ctx = cordis::Context::root();
        InvariantRegistry::new(
            &ctx,
            InvariantConfig {
                enabled: true,
                package_allowlist: Vec::new(),
                package_blocklist: Vec::new(),
            },
        );

        let disposer = apply(&ctx);
        disposer().await;
        // A second registration after teardown must succeed too.
        let disposer = apply(&ctx);
        disposer().await;
    });
}
