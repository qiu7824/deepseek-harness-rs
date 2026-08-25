from pathlib import Path
import shutil

root = Path(__file__).resolve().parents[1]
source = root / "release" / "plugins"
target = root / "target" / "release" / "plugins"
if not source.is_dir():
    raise SystemExit(f"missing bundled plugin source: {source}")
target.mkdir(parents=True, exist_ok=True)
source_names = {entry.name for entry in source.iterdir() if entry.is_dir()}
for entry in list(target.iterdir()):
    if entry.is_dir() and entry.name not in source_names:
        shutil.rmtree(entry)
for entry in source.iterdir():
    if not entry.is_dir():
        continue
    destination = target / entry.name
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(entry, destination)
required = {"dsh-context-jump", "dsh-web-preview-rs", "dsh-better-sidebar"}
missing = sorted(name for name in required if not (target / name / "package.json").is_file())
if missing:
    raise SystemExit(f"staged release is missing bundled plugins: {', '.join(missing)}")
print(f"staged {len(source_names)} bundled plugins into {target}")
