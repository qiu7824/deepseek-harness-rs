use super::*;

pub struct ExecutionResources {
    pub environment: Vec<(String, String)>,
    run: Lease,
    cache: Option<Lease>,
}
impl ExecutionResources {
    pub fn spill_directory(&self) -> PathBuf {
        self.run.path()
    }
    pub fn attach_process(&mut self, pid: u32) -> Result<()> {
        self.run.attach_process(pid)
    }
    pub fn finish(&mut self, success: bool) {
        let _ = self.run.finish(success);
        if let Some(cache) = self.cache.as_mut() {
            let _ = cache.finish(success);
        }
    }
    pub fn protect(&mut self) {
        let _ = self.run.store.retain(self.run.id(), true, false);
        if let Some(cache) = self.cache.as_ref() {
            let _ = cache.store.retain(cache.id(), true, false);
        }
    }
}

impl Store {
    pub fn prepare_execution(
        self: &Arc<Self>,
        cwd: &str,
        argv: &[String],
        explicit: &[(String, Option<String>)],
    ) -> Result<ExecutionResources> {
        let lookup = |key: &str| {
            explicit
                .iter()
                .rev()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .and_then(|(_, value)| value.clone())
        };
        let owner = lookup("DSH_SESSION_ID").unwrap_or_else(|| "host".into());
        let project = fs::canonicalize(lookup("DSH_PROJECT_ROOT").as_deref().unwrap_or(cwd))
            .map_err(|e| format!("执行目录不可访问：{e}"))?
            .to_string_lossy()
            .into_owned();
        let run = self.allocate(&owner, &project, "run", "执行临时文件与输出")?;
        let mut environment = Vec::new();
        for key in ["TEMP", "TMP", "TMPDIR"] {
            if !explicit
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(key))
            {
                environment.push((key.into(), run.path().to_string_lossy().into_owned()));
            }
        }
        environment.push((
            "DSH_SCRATCH_DIR".into(),
            run.path().to_string_lossy().into_owned(),
        ));
        environment.push(("DSH_SCRATCH_ID".into(), run.id().into()));
        let cargo = Path::new(cwd).join("Cargo.toml").is_file()
            || argv
                .iter()
                .any(|arg| arg == "cargo" || arg.starts_with("cargo "));
        let cache = if cargo
            && !explicit
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("CARGO_TARGET_DIR"))
        {
            let mut key = format!(
                "cargo\0{}\0{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            for name in [
                "RUSTUP_TOOLCHAIN",
                "CARGO_BUILD_TARGET",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "RUSTC",
                "RUSTC_WRAPPER",
            ] {
                key.push('\0');
                key.push_str(name);
                key.push('=');
                key.push_str(
                    &lookup(name)
                        .or_else(|| std::env::var(name).ok())
                        .unwrap_or_default(),
                );
            }
            let cache = self.cache(&project, &key)?;
            environment.push((
                "CARGO_TARGET_DIR".into(),
                cache.path().to_string_lossy().into_owned(),
            ));
            Some(cache)
        } else {
            None
        };
        Ok(ExecutionResources {
            environment,
            run,
            cache,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_paths_remain_and_cargo_cache_is_shared_between_runs() {
        let fixture = crate::tests::Fixture::new();
        let root = fixture.0.join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        let store = Store::open(fixture.0.join("managed")).unwrap();
        let cwd = root.to_string_lossy();
        let argv = vec!["cargo".into(), "test".into()];
        let mut first = store.prepare_execution(&cwd, &argv, &[]).unwrap();
        let mut second = store.prepare_execution(&cwd, &argv, &[]).unwrap();
        let get = |execution: &ExecutionResources, key: &str| {
            execution
                .environment
                .iter()
                .find(|(name, _)| name == key)
                .unwrap()
                .1
                .clone()
        };
        assert_ne!(get(&first, "TEMP"), get(&second, "TEMP"));
        assert_eq!(
            get(&first, "CARGO_TARGET_DIR"),
            get(&second, "CARGO_TARGET_DIR")
        );
        let mut explicit = store
            .prepare_execution(
                &cwd,
                &argv,
                &[
                    ("TEMP".into(), Some("user-temp".into())),
                    ("CARGO_TARGET_DIR".into(), Some("user-target".into())),
                ],
            )
            .unwrap();
        assert!(
            explicit
                .environment
                .iter()
                .all(|(name, _)| name != "TEMP" && name != "CARGO_TARGET_DIR")
        );
        first.finish(true);
        second.finish(true);
        explicit.finish(true);
    }
}
