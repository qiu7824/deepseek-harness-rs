"""Build pinned browser assets exclusively from the supplied dependency tree."""
from __future__ import annotations
import argparse, base64, hashlib, json, os, pathlib, subprocess, tempfile


def browser_entry(package: pathlib.Path) -> pathlib.Path:
    metadata = json.loads((package / "package.json").read_text(encoding="utf8"))
    def selected(value):
        if isinstance(value, str):
            return value
        if isinstance(value, list):
            return next((result for item in value if (result := selected(item))), None)
        if isinstance(value, dict):
            for condition in (".", "browser", "import", "default"):
                if result := selected(value.get(condition)):
                    return result
        return None
    entry = selected(metadata.get("exports")) or metadata.get("module") or selected(metadata.get("browser")) or metadata.get("main") or "index.js"
    browser = metadata.get("browser")
    if isinstance(browser, dict) and isinstance(browser.get(entry), str):
        entry = browser[entry]
    path = (package / entry).resolve()
    if not path.is_file():
        path = next((candidate for candidate in [pathlib.Path(str(path) + suffix) for suffix in (".js", ".mjs", ".cjs")] + [path / "index.js"] if candidate.is_file()), path)
    if not path.is_relative_to(package.resolve()) or not path.is_file():
        raise ValueError(f"Package has no usable browser import entry: {package.name}")
    return path


def validate_inputs(metadata: dict, modules: pathlib.Path, sources: pathlib.Path, generated: pathlib.Path, cwd: pathlib.Path) -> None:
    roots = (modules.resolve(), sources.resolve(), generated.resolve())
    for name in metadata["inputs"]:
        if name.startswith("(disabled):") or name == "<runtime>":
            continue
        path = pathlib.Path(name)
        path = (cwd / path).resolve() if not path.is_absolute() else path.resolve()
        if not any(path.is_relative_to(root) for root in roots):
            raise ValueError(f"Bundler resolved an unpinned input outside the supplied dependency tree: {path}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--node-modules", type=pathlib.Path, required=True)
    args = parser.parse_args()
    modules = args.node_modules.resolve()
    root = pathlib.Path(__file__).resolve().parents[1]
    plugin = root / "release/plugins/dsh-sidebar-workbench-suite"
    sources = plugin / "vendor-src"
    spec = json.loads((sources / "package.json").read_text(encoding="utf8"))
    versions = {}
    for name, expected in spec["dependencies"].items():
        actual = json.loads((modules / name / "package.json").read_text(encoding="utf8"))["version"]
        if actual != expected:
            raise ValueError(f"{name}: expected {expected}, got {actual}")
        versions[name] = actual
    env = dict(os.environ, NODE_PATH=str(modules))
    # NODE_PATH is only a fallback: an ancestor node_modules can otherwise win.
    # Explicit aliases plus metafile validation bind all inputs to the lock tree.
    aliases = [f"--alias:{name}={browser_entry(modules / name)}" for name in sorted(versions) if name != "esbuild"]
    outputs = {}
    with tempfile.TemporaryDirectory(prefix="dsh-sidebar-build-", dir=os.environ.get("DSH_BUILD_TEMP")) as temporary:
        staging = pathlib.Path(temporary).resolve()
        def bundle(name, extra=()):
            output = staging / f"{name}.js"
            metadata = staging / f"{name}.meta.json"
            subprocess.run(["node", str(modules / "esbuild/bin/esbuild"), str(sources / f"{name}.js"),
                            "--bundle", "--minify", "--format=iife", "--platform=browser", "--target=es2022",
                            *aliases, *extra, f"--metafile={metadata}", f"--outfile={output}"],
                           check=True, env=env, cwd=modules.parent)
            validate_inputs(json.loads(metadata.read_text(encoding="utf8")), modules, sources, staging, modules.parent)
            outputs[f"lib/{name}.js"] = "sha256:" + hashlib.sha256(output.read_bytes()).hexdigest()
        for name in ("editor", "markdown", "mermaid", "docx"):
            bundle(name)
        pdf = modules / "pdfjs-dist"
        assets = {kind: {file.name: base64.b64encode(file.read_bytes()).decode("ascii")
                        for file in sorted((pdf / directory).iterdir())
                        if file.is_file() and file.suffix.lower() not in (".md", ".txt")}
                  for kind, directory in [("cMapUrl", "cmaps"), ("standardFontDataUrl", "standard_fonts"), ("wasmUrl", "wasm")]}
        asset_module = staging / "pdf-assets.js"
        asset_module.write_text("export const assets=" + json.dumps(assets) + ";\nexport const workerSource=" +
                               json.dumps((pdf / "build/pdf.worker.min.mjs").read_text(encoding="utf8")) +
                               ";\nexport const workerVersion=" + json.dumps(versions["pdfjs-dist"]) + ";\n", encoding="utf8", newline="\n")
        bundle("pdf", [f"--alias:dsh-pdf-assets={asset_module}"])
        # Publish only after every dependency graph has passed validation.
        for relative in outputs:
            (plugin / relative).write_bytes((staging / pathlib.Path(relative).name).read_bytes())
        # The core attachment dialog uses the same pinned document renderer.
        (root / "web/dist/plugins/docx-preview-runtime.js").write_bytes((plugin / "lib/docx.js").read_bytes())
    (plugin / "vendor-lock.json").write_text(json.dumps({"builder": "esbuild@" + versions.pop("esbuild"),
        "target": "es2022", "format": "iife", "resolution": "pinned-module-tree", "dependencies": versions,
        "outputs": outputs}, indent=2) + "\n", encoding="utf8", newline="\n")
    notices = ["# Third-party notices", "", "The browser assets include the following packages. License texts are reproduced from their distributed packages.", ""]
    packages = json.loads((sources / "package-lock.json").read_text(encoding="utf8"))["packages"]
    for relative in sorted(packages):
        if not relative.startswith("node_modules/"):
            continue
        directory = modules.parent / relative
        manifest = directory / "package.json"
        if not manifest.is_file():
            continue
        metadata = json.loads(manifest.read_text(encoding="utf8"))
        notices.extend([f"## {metadata.get('name', relative)} {metadata.get('version', '')}", "", "License: " + str(metadata.get("license", "See package distribution")), ""])
        for file in sorted(directory.iterdir()):
            if file.is_file() and file.name.lower().startswith(("license", "licence", "copying", "notice")):
                notices.extend(["### " + file.name, "", "```text", file.read_text(encoding="utf8", errors="replace").strip(), "```", ""])
    (plugin / "THIRD_PARTY_NOTICES.md").write_text("\n".join(notices), encoding="utf8", newline="\n")
    print("Built sidebar assets with verified pinned dependency resolution")


if __name__ == "__main__":
    main()
