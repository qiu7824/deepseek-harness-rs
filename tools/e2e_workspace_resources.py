"""Verify real Host artifact mutations, recovery and native temporary environment."""
from __future__ import annotations
import argparse,json,pathlib,time,urllib.request,urllib.error,sys,os
sys.dont_write_bytecode=True
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import require_ok,rpc
from e2e_sidebar_git_terminal import request as preview

def request(port,operation,args,status=200):
    request=urllib.request.Request(f"http://127.0.0.1:{port}/__dsh-artifacts/{operation}",data=json.dumps(args).encode(),headers={"Origin":f"http://127.0.0.1:{port}","Sec-Fetch-Site":"same-origin","Content-Type":"application/json"})
    try: response=urllib.request.urlopen(request,timeout=60)
    except urllib.error.HTTPError as error:
        assert error.code==status,(operation,error.code,error.read())
        return json.loads(error.read())
    with response:
        assert response.status==status
        return json.loads(response.read())

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--workdir',type=pathlib.Path,required=True);args=parser.parse_args()
    run=args.workdir.resolve()/f"run-{int(time.time()*1000)}";project=run/'project';project.mkdir(parents=True);home=run/'home';env=isolated_environment(run,home)
    (project/'existing.txt').write_text('before',encoding='utf8')
    sequence=0
    with running_fixture_host(args.binary.resolve(),run,env,None,'workspace-resources') as port:
        def call(method,payload):
            nonlocal sequence
            sequence+=1;return require_ok(rpc(port,method,payload,sequence),method)
        workspace=call('workspace.create',{'path':str(project)})['workspace'];owner=call('session.create',{'workspaceId':workspace['workspaceId']})['sessionId']
        request(port,'list',{'sessionId':owner})
        (project/'existing.txt').write_text('updated',encoding='utf8');(project/'report.txt').write_text('deliverable',encoding='utf8')
        result=request(port,'list',{'sessionId':owner});entries={entry['path']:entry for entry in result['entries']}
        assert entries['existing.txt']['change']=='modified',result;assert entries['report.txt']['change']=='created',result
        report=entries['report.txt'];(project/'report.txt').write_text('concurrent edit',encoding='utf8')
        failure=request(port,'file-action',{'sessionId':owner,'action':'trash','path':report['path'],'etag':report['etag']},400)
        assert '修改' in failure['message'];assert (project/'report.txt').read_text()=='concurrent edit'
        report=next(entry for entry in request(port,'list',{'sessionId':owner})['entries'] if entry['path']=='report.txt')
        removed=request(port,'file-action',{'sessionId':owner,'action':'trash','path':report['path'],'etag':report['etag']})
        assert not (project/'report.txt').exists();resources=request(port,'resources',{'sessionId':owner})['entries'];item=next(item for item in resources if item['id']==removed['id']);assert item['kind']=='trash'
        request(port,'file-action',{'sessionId':owner,'action':'restore','id':removed['id']});assert (project/'report.txt').read_text()=='concurrent edit'
        request(port,'file-action',{'sessionId':owner,'action':'restore','id':removed['id']},400)
        current=next(entry for entry in request(port,'list',{'sessionId':owner})['entries'] if entry['path']=='report.txt')
        request(port,'file-action',{'sessionId':owner,'action':'rename','path':'report.txt','etag':current['etag'],'newPath':'renamed.txt'});assert (project/'renamed.txt').exists()
        request(port,'file-action',{'sessionId':owner,'action':'rename','path':'../outside.txt','etag':current['etag'],'newPath':'escape.txt'},400)
        opened=preview(port,'terminal-action',body={'sessionId':owner,'action':'open','name':'scratch environment'});terminal=opened['id']
        command='echo DSH_SCRATCH_IS=%DSH_SCRATCH_DIR% & echo proof> "%TEMP%\\resource-proof.txt"\r' if os.name=='nt' else "printf 'DSH_TEMP_IS=%s\\n' \"$TMPDIR\"; printf proof > \"$TMPDIR/resource-proof.txt\"\r"
        preview(port,'terminal-action',body={'sessionId':owner,'terminalId':terminal,'action':'input','text':command})
        deadline=time.monotonic()+25
        while time.monotonic()<deadline:
            output=preview(port,'terminal-read',query={'sessionId':owner,'terminalId':terminal,'count':2000})['text']
            if 'scratch' in output and 'content' in output:break
            time.sleep(.1)
        else:raise AssertionError(output)
        resources=request(port,'resources',{'sessionId':owner})['entries'];assert any(item['busy'] for item in resources),resources
        assert any((pathlib.Path(item['path'])/'resource-proof.txt').is_file() for item in resources if item['busy']),resources
        preview(port,'terminal-action',body={'sessionId':owner,'terminalId':terminal,'action':'close'})
        evidence={'artifactChanges':entries,'removedId':removed['id'],'terminalEnvironment':output,'resourceCount':len(resources)}
        (run/'evidence.json').write_text(json.dumps(evidence,ensure_ascii=False,indent=2),encoding='utf8')
    print(f'PASS artifact changes, version conflict, trash, restore, rename, traversal and native TEMP: {run}')
    return 0
if __name__=='__main__':raise SystemExit(main())
