"""Exercise Git/Cloud adoption through a real Host and a local Git fixture."""
from __future__ import annotations
import argparse,json,pathlib,subprocess,uuid
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import rpc,require_ok

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--binary',type=pathlib.Path,required=True)
    parser.add_argument('--workdir',type=pathlib.Path,required=True)
    args=parser.parse_args()
    work=args.workdir.resolve()/('workspace-sources-'+uuid.uuid4().hex)
    work.mkdir(parents=True)
    source=work/'source';source.mkdir()
    def git(*arguments):
        return subprocess.check_output(['git','-C',str(source),*arguments],stderr=subprocess.STDOUT,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
    git('init','--initial-branch=main')
    (source/'marker.txt').write_text('main fixture',encoding='utf-8')
    git('add','marker.txt');git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-m','fixture')
    git('checkout','-b','feature-test')
    (source/'marker.txt').write_text('branch fixture',encoding='utf-8')
    git('add','marker.txt');git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-m','branch fixture')
    git('checkout','main')
    env=isolated_environment(work,work/'home')
    with running_fixture_host(args.binary.resolve(),work,env,None,'workspace-sources') as port:
        for sequence,kind in enumerate(['git','cloud'],1):
            destination=work/(kind+' destination')
            payload={'kind':kind,'source':str(source),'path':str(destination),'branch':'feature-test'}
            value=require_ok(rpc(port,'workspace.create',payload,sequence),'clone')
            assert value['created'] is True and value['workspace']['workspaceId']
            assert (destination/'.git').is_dir()
            assert (destination/'marker.txt').read_text(encoding='utf-8')=='branch fixture'
            repeated=rpc(port,'workspace.create',payload,sequence+10)['result']
            assert repeated['ok'] is False
            assert (destination/'marker.txt').read_text(encoding='utf-8')=='branch fixture'
        for sequence,source_value in enumerate(['--upload-pack=bad','ext::bad','https://secret@example.test/repo'],30):
            target=work/str(sequence)
            value=rpc(port,'workspace.create',{'kind':'git','source':source_value,'path':str(target)},sequence)['result']
            assert value['ok'] is False and not target.exists()
    evidence={'passed':True,'gitClone':True,'cloudClone':True,'branchSelection':True,'existingDirectoryPreserved':True,'invalidSourceRejected':True,'work':str(work)}
    (work/'result.json').write_text(json.dumps(evidence,indent=2),encoding='utf-8')
    print(json.dumps(evidence))

if __name__=='__main__':main()
