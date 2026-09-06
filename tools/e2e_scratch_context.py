"""Check execution-copy confinement, bounded model context and versioned delivery."""
from __future__ import annotations
import argparse,json,pathlib,time,threading,uuid,sys
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
sys.dont_write_bytecode=True
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import require_ok,rpc
from e2e_workspace_resources import request

class Model(BaseHTTPRequestHandler):
    project=None
    requests=[]
    def log_message(self,*args):pass
    def do_POST(self):
        import re
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));self.requests.append(body)
        outputs=[item['content'] for item in body.get('messages',[]) if item.get('role')=='tool']
        def parsed(index):return json.loads(outputs[index])
        step=len(outputs);tool='workspace_scratch';args=None
        if step==0:args={'action':'prepare_copy'}
        elif step==1:
            copy=parsed(0);target=str(self.project/'input.txt').replace("'","''")
            tool='pwsh';args={'description':'Check managed execution output and source protection','workdir':copy['workdir'],'command':"Write-Output ('BEGIN_'+('x'*22000)+'_END'); Set-Content -LiteralPath (Join-Path $pwd 'result.txt') -Value 'DELIVERED_FIXTURE' -Encoding UTF8 -NoNewline; try {Set-Content -LiteralPath '"+target+"' -Value 'INVALID' -ErrorAction Stop; Write-Output 'SOURCE_WRITE_ALLOWED'} catch {Write-Output 'SOURCE_WRITE_BLOCKED'}"}
        elif step==2:
            assert len(outputs[1].encode('utf8'))<=12000, len(outputs[1]);assert 'SOURCE_WRITE_BLOCKED' in outputs[1],outputs[1][-1800:]
            match=re.search(r'scratch:([a-f0-9-]+)',outputs[1]);assert match,outputs[1][-1000:]
            args={'action':'read','id':match[1],'path':'output.txt','offset':0,'limit':32000}
        elif step==3:
            assert len(parsed(2)['text'])>22000,'explicit detail read must not be spilled again'
            args={'action':'inspect','target':'final.txt'}
        elif step==4:args={'action':'promote','id':parsed(0)['id'],'path':'worktree/result.txt','target':'final.txt','expectedSha256':parsed(3)['sha256']}
        elif step==5:args={'action':'release','id':parsed(0)['id']}
        delta={'role':'assistant','content':'SCRATCH_E2E_DONE'};finish='stop'
        if args is not None:delta={'role':'assistant','tool_calls':[{'index':0,'id':uuid.uuid4().hex,'type':'function','function':{'name':tool,'arguments':json.dumps(args)}}]};finish='tool_calls'
        wire='data: '+json.dumps({'choices':[{'index':0,'delta':delta,'finish_reason':None}]})+'\n\ndata: '+json.dumps({'choices':[{'index':0,'delta':{},'finish_reason':finish}],'usage':{'prompt_tokens':100,'completion_tokens':50,'total_tokens':150}})+'\n\ndata: [DONE]\n\n'
        data=wire.encode();self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--workdir',type=pathlib.Path,required=True);args=parser.parse_args()
    run=args.workdir.resolve()/('run-'+str(int(time.time()*1000)));project=run/'project';project.mkdir(parents=True);(project/'input.txt').write_text('original input',encoding='utf8');home=run/'home';env=isolated_environment(run,home)
    Model.project=project;Model.requests=[];server=ThreadingHTTPServer(('127.0.0.1',0),Model);threading.Thread(target=server.serve_forever,daemon=True).start()
    settings={'llm-pi-ai':{'providers':{'scratch-fixture':{'keyless':True,'api':'openai-completions','baseURL':f'http://127.0.0.1:{server.server_port}/v1','models':[{'id':'scratch-fixture','contextWindow':131072,'maxTokens':4096}]}}},'agent-default-model':{'provider':'scratch-fixture','model':'scratch-fixture'}}
    (home/'settings.json').write_text(json.dumps(settings),encoding='utf8')
    try:
        with running_fixture_host(args.binary.resolve(),run,env,None,'scratch-context') as port:
            seq=0
            def call(method,payload):
                nonlocal seq
                seq+=1;return require_ok(rpc(port,method,payload,seq),method)
            workspace=call('workspace.create',{'path':str(project)})['workspace'];owner=call('session.create',{'workspaceId':workspace['workspaceId']})['sessionId']
            call('session.prompt',{'sessionId':owner,'content':[{'type':'text','text':'scratch context fixture'}],'mode':'queue'})
            deadline=time.monotonic()+110
            while time.monotonic()<deadline:
                history=call('session.history',{'sessionId':owner});events=[item['event'] for item in history['events']]
                if any(event['type']=='turn/end' for event in events):break
                time.sleep(.2)
            else:raise AssertionError('scratch turn did not finish')
            assert any('SCRATCH_E2E_DONE' in json.dumps(event) for event in events),events[-3:]
            assert (project/'input.txt').read_text()=='original input';assert (project/'final.txt').read_text(encoding='utf-8-sig')=='DELIVERED_FIXTURE'
            assert sorted(path.name for path in project.iterdir())==['final.txt','input.txt']
            assert any(row['path']=='final.txt' for row in request(port,'list',{'sessionId':owner})['entries'])
            (run/'evidence.json').write_text(json.dumps({'requestCount':len(Model.requests),'files':[path.name for path in project.iterdir()],'sourceProtected':True,'boundedContext':True,'explicitRead':True,'delivery':True},indent=2),encoding='utf8')
        print('PASS managed execution copy, source protection, bounded model context, detail recovery and delivery:',run)
    finally:server.shutdown();server.server_close();(run/'model-requests.json').write_text(json.dumps(Model.requests,ensure_ascii=False,indent=2),encoding='utf8')
    return 0
if __name__=='__main__':raise SystemExit(main())
