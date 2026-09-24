"""Bundle a checksum-pinned Node runtime and its license for Computer Use JS."""
from __future__ import annotations
import hashlib,json,pathlib,shutil,stat,tarfile,urllib.request,zipfile

ROOT=pathlib.Path(__file__).resolve().parents[1]

def stage_node_runtime(stage:pathlib.Path,platform:str,arch:str,cache:pathlib.Path|None=None)->None:
    lock=json.loads((ROOT/'tools/node_runtime_lock.json').read_text(encoding='utf-8'))
    cpu={'x86_64':'x64','aarch64':'arm64'}[arch]
    os_name={'windows':'win','linux':'linux','macos':'darwin'}[platform]
    stem=f"node-v{lock['version']}-{os_name}-{cpu}"
    archive_name=stem+('.zip' if platform=='windows' else '.tar.xz')
    expected=lock['archives'][archive_name]
    cache=cache or ROOT/'target/node-runtime-downloads';cache.mkdir(parents=True,exist_ok=True)
    archive=cache/archive_name
    if not archive.exists():
        temporary=archive.with_suffix(archive.suffix+'.part')
        with urllib.request.urlopen(lock['baseUrl']+archive_name,timeout=90) as response,temporary.open('wb') as out:shutil.copyfileobj(response,out)
        if hashlib.sha256(temporary.read_bytes()).hexdigest()!=expected:raise ValueError('Node archive checksum mismatch')
        temporary.replace(archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest()!=expected:raise ValueError('Cached Node archive checksum mismatch')
    binary='node.exe' if platform=='windows' else 'node'
    wanted={binary:stem+('/node.exe' if platform=='windows' else '/bin/node'),'LICENSE':stem+'/LICENSE'}
    output=stage/'runtime/node';output.mkdir(parents=True,exist_ok=True)
    if platform=='windows':
        with zipfile.ZipFile(archive) as package:
            for name,member in wanted.items():(output/name).write_bytes(package.read(member))
    else:
        with tarfile.open(archive,'r:xz') as package:
            for name,member in wanted.items():
                entry=package.getmember(member)
                if not entry.isfile():raise ValueError('Node runtime archive member is not a regular file')
                with package.extractfile(entry) as data:(output/name).write_bytes(data.read())
        (output/binary).chmod((output/binary).stat().st_mode|stat.S_IXUSR|stat.S_IXGRP|stat.S_IXOTH)
    identity={'version':lock['version'],'archive':archive_name,'archiveSha256':expected,'binarySha256':hashlib.sha256((output/binary).read_bytes()).hexdigest()}
    (output/'IDENTITY.json').write_text(json.dumps(identity,indent=2)+'\n',encoding='utf-8')


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('--stage', type=pathlib.Path, required=True)
    parser.add_argument('--platform', choices=['windows', 'linux', 'macos'], required=True)
    parser.add_argument('--arch', choices=['x86_64', 'aarch64'], required=True)
    parser.add_argument('--cache', type=pathlib.Path, required=True)
    args = parser.parse_args()
    stage_node_runtime(args.stage, args.platform, args.arch, args.cache)
