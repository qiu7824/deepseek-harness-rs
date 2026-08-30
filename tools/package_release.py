from __future__ import annotations
import argparse, json, os, pathlib, shutil, stat, subprocess, tarfile, zipfile

ROOT=pathlib.Path(__file__).resolve().parents[1]

def copy_tree(src: pathlib.Path,dst:pathlib.Path):
    if src.exists(): shutil.copytree(src,dst,dirs_exist_ok=True)

def main():
    p=argparse.ArgumentParser();p.add_argument('--platform',choices=['windows','linux','macos'],required=True);p.add_argument('--arch',required=True);p.add_argument('--variant',choices=['full','no-skin'],default='full');p.add_argument('--version',required=True);a=p.parse_args()
    suffix=f"deepseek-harness-rs-v{a.version}-{a.platform}-{a.arch}-{a.variant}"
    stage=ROOT/'dist'/suffix
    if stage.exists(): shutil.rmtree(stage)
    stage.mkdir(parents=True)
    source=ROOT/'target'/'release'/('dsh.exe' if a.platform=='windows' else 'dsh')
    output='deepseek harness-rs.exe' if a.platform=='windows' else 'deepseek-harness-rs'
    shutil.copy2(source,stage/output)
    if a.platform!='windows': (stage/output).chmod((stage/output).stat().st_mode|stat.S_IXUSR|stat.S_IXGRP|stat.S_IXOTH)
    copy_tree(ROOT/'release'/'plugins',stage/'plugins')
    copy_tree(ROOT/'web'/'dist',stage/'web'/'dist')
    if a.variant=='no-skin': shutil.rmtree(stage/'web'/'dist'/'skins',ignore_errors=True)
    copy_tree(ROOT/'config'/'agent-presets',stage/'config'/'agent-presets')
    for name in ['README.md','README.zh.md','LICENSE','THIRD_PARTY_NOTICES.md']:
        if (ROOT/name).exists(): shutil.copy2(ROOT/name,stage/name)
    shutil.copy2(ROOT/'release'/'PLUGIN_SECURITY.md',stage/'PLUGIN_SECURITY.md')
    if a.platform=='windows':
        shutil.copy2(ROOT/'release'/'windows'/'DshServiceManager.ps1',stage/'DshServiceManager.ps1')
        shutil.copy2(ROOT/'release'/'windows'/'启动DeepSeek Harness-rs.cmd',stage/'启动DeepSeek Harness-rs.cmd')
    else:
        launcher=stage/'deepseek-harness-rs-web'
        launcher.write_text('#!/bin/sh\nexec "$(dirname "$0")/deepseek-harness-rs" web "$@"\n',encoding='utf-8')
        launcher.chmod(0o755)
    if a.variant=='no-skin': (stage/'NO_SKIN').write_text('no-skin build\n',encoding='ascii')
    manifest={'name':suffix,'version':a.version,'platform':a.platform,'arch':a.arch,'variant':a.variant,'entry':output}
    (stage/'PACKAGE.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2),encoding='utf-8')
    if a.platform=='windows':
        out=ROOT/'dist'/f'{suffix}-portable.zip'
        with zipfile.ZipFile(out,'w',zipfile.ZIP_DEFLATED) as z:
            for f in stage.rglob('*'):
                if f.is_file(): z.write(f,pathlib.Path(suffix)/f.relative_to(stage))
    else:
        out=ROOT/'dist'/f'{suffix}-portable.tar.gz'
        with tarfile.open(out,'w:gz') as t:t.add(stage,arcname=suffix)
    print(stage);print(out)
if __name__=='__main__':main()
