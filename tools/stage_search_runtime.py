"""Stage pinned ripgrep executables without requiring a developer PATH."""
from __future__ import annotations
import hashlib,json,pathlib,shutil,stat,tarfile,urllib.request,zipfile

ROOT=pathlib.Path(__file__).resolve().parents[1]
ARCHIVES={
    ('windows','x86_64'):('x86_64-pc-windows-msvc.zip','71b2fef860abe467217a538ff31de02f5258807c0129f771846f87bd029aafc5'),
    ('linux','x86_64'):('x86_64-unknown-linux-musl.tar.gz','33e15bcf1624b25cdd2a55813a47a2f95dbe126268203e76aa6a585d1e7b149c'),
    ('macos','x86_64'):('x86_64-apple-darwin.tar.gz','af7825fcc69a2afc7a7aea55fc9af90e26421d8f20fe59df32e233c0b8a231c1'),
    ('macos','aarch64'):('aarch64-apple-darwin.tar.gz','3750b2e93f37e0c692657da574d7019a101c0084da05a790c83fd335bad973e4'),
}
VERSION='15.2.0'

def stage_search_runtime(stage:pathlib.Path,platform:str,arch:str,cache:pathlib.Path|None=None)->None:
    suffix,expected=ARCHIVES[(platform,arch)]
    name=f'ripgrep-{VERSION}-{suffix}'
    cache=cache or ROOT/'target/search-runtime-downloads';cache.mkdir(parents=True,exist_ok=True)
    archive=cache/name
    if not archive.exists():
        temporary=archive.with_suffix(archive.suffix+'.part')
        with urllib.request.urlopen(f'https://github.com/BurntSushi/ripgrep/releases/download/{VERSION}/{name}',timeout=90) as response,temporary.open('wb') as out:shutil.copyfileobj(response,out)
        if hashlib.sha256(temporary.read_bytes()).hexdigest()!=expected:raise ValueError('ripgrep archive checksum mismatch')
        temporary.replace(archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest()!=expected:raise ValueError('Cached ripgrep archive checksum mismatch')
    stem=name.removesuffix('.zip').removesuffix('.tar.gz')
    binary='rg.exe' if platform=='windows' else 'rg'
    output=stage/'runtime/search';output.mkdir(parents=True,exist_ok=True)
    wanted=[binary,'COPYING','LICENSE-MIT','UNLICENSE']
    if platform=='windows':
        with zipfile.ZipFile(archive) as package:
            for item in wanted:(output/item).write_bytes(package.read(stem+'/'+item))
    else:
        with tarfile.open(archive,'r:gz') as package:
            for item in wanted:
                entry=package.getmember(stem+'/'+item)
                if not entry.isfile():raise ValueError('ripgrep member must be a regular file')
                with package.extractfile(entry) as data:(output/item).write_bytes(data.read())
        (output/binary).chmod((output/binary).stat().st_mode|stat.S_IXUSR|stat.S_IXGRP|stat.S_IXOTH)
    (output/'IDENTITY.json').write_text(json.dumps({'version':VERSION,'archive':name,'archiveSha256':expected,'binarySha256':hashlib.sha256((output/binary).read_bytes()).hexdigest()},indent=2)+'\n',encoding='utf-8')
