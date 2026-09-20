//! Windows launch resolution. Package-manager arguments never pass through a shell.
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub fn locate_in(program: &str, paths: &[PathBuf], pathext: &str) -> Option<PathBuf> {
    let direct = Path::new(program);
    if direct.is_absolute() { return direct.is_file().then(|| direct.to_path_buf()); }
    if program.contains(['/', '\\']) { return None; }
    let extensions: Vec<_> = if direct.extension().is_some() { vec![String::new()] } else {
        pathext.split(';').filter(|s| s.starts_with('.') && !s.contains(['/', '\\']))
            .map(str::to_owned).collect()
    };
    paths.iter().filter(|p|p.is_absolute()).flat_map(|p|extensions.iter().map(move |ext|p.join(format!("{program}{ext}"))))
        .find(|p|p.is_file())
}

pub fn locate(program: &str) -> Option<PathBuf> {
    locate_in(program, &std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>(),
        &std::env::var("PATHEXT").unwrap_or_else(|_|".COM;.EXE;.BAT;.CMD".into()))
}

pub fn adapt(command: &mut Vec<String>, reads: &mut Vec<PathBuf>) -> Result<()> {
    let Some(program) = command.first() else { return Ok(()); };
    let Some(exe) = locate(program) else { bail!("PROGRAM_NOT_FOUND: {program}"); };
    let name = exe.file_stem().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase();
    let extension = exe.extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(name.as_str(),"npm"|"npx") && matches!(extension.as_str(),""|"cmd"|"bat"|"ps1") {
        let root = exe.parent().ok_or_else(||anyhow::anyhow!("Package-manager path has no parent"))?;
        let entry = root.join(format!("node_modules/npm/bin/{name}-cli.js"));
        if !entry.is_file() { bail!("PACKAGE_MANAGER_ENTRY_MISSING: {}", entry.display()); }
        let node = root.join("node.exe");
        let node = if node.is_file() { node } else { locate("node.exe").ok_or_else(||anyhow::anyhow!("Node executable not found"))? };
        reads.push(root.to_path_buf());
        if let Some(parent) = node.parent() { reads.push(parent.to_path_buf()); }
        command[0] = node.to_string_lossy().into_owned();
        command.insert(1,entry.to_string_lossy().into_owned());
    } else {
        if !matches!(extension.as_str(),"exe"|"com") {
            bail!("SCRIPT_REQUIRES_ADAPTER: use the matching shell/interpreter explicitly for {}",exe.display());
        }
        if let Some(parent) = exe.parent() { reads.push(parent.to_path_buf()); }
        command[0] = exe.to_string_lossy().into_owned();
    }
    Ok(())
}

#[cfg(all(test,windows))]
mod tests {
    use super::*;
    #[test]
    fn windows_lookup_ignores_extensionless_shadow_and_honors_extension_order() {
        let root=std::env::temp_dir().join(format!("dsh-launcher-{}",std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for name in ["npm","npm.cmd","npm.exe"] {std::fs::write(root.join(name),b"test").unwrap();}
        assert_eq!(locate_in("npm",&[root.clone()],".CMD;.EXE"),Some(root.join("npm.CMD")));
        assert_eq!(locate_in("npm",&[root.clone()],".EXE;.CMD"),Some(root.join("npm.EXE")));
        assert!(locate_in("./npm",&[root.clone()],".CMD").is_none());
        for name in ["npm","npm.cmd","npm.exe"] {std::fs::remove_file(root.join(name)).unwrap();}
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn npm_adapter_preserves_literal_arguments_without_shell_evaluation() {
        let root=std::env::temp_dir().join(format!("dsh-npm-{}",std::process::id()));
        let bin=root.join("node_modules/npm/bin");std::fs::create_dir_all(&bin).unwrap();
        for name in ["npm.cmd","node.exe"] { std::fs::write(root.join(name),b"fixture").unwrap(); }
        std::fs::write(bin.join("npm-cli.js"),b"fixture").unwrap();
        let args=vec!["".to_string(),"& echo injected".into(),"%PATH%".into(),"quoted \" text".into(),"中文 空格".into()];
        let mut command=vec![root.join("npm.cmd").to_string_lossy().into_owned()];command.extend(args.clone());
        adapt(&mut command,&mut Vec::new()).unwrap();
        assert_eq!(command[0],root.join("node.exe").to_string_lossy());
        assert_eq!(Path::new(&command[1]).canonicalize().unwrap(),bin.join("npm-cli.js").canonicalize().unwrap());
        assert_eq!(&command[2..],args.as_slice());
        for file in [root.join("npm.cmd"),root.join("node.exe"),bin.join("npm-cli.js")] {std::fs::remove_file(file).unwrap();}
        for folder in [bin,root.join("node_modules/npm"),root.join("node_modules"),root] {std::fs::remove_dir(folder).unwrap();}
    }
}
