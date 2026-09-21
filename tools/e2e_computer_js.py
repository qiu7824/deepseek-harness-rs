"""Exercise the packaged JS kernel through a real Host and deterministic model."""
from __future__ import annotations
import argparse,json,os,pathlib,threading,time,uuid,urllib.request
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import require_ok,rpc

TASK='computer-js-kernel-regression'
class Model(BaseHTTPRequestHandler):
    requests=[]
    def log_message(self,*args):pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.requests.append(body)
        outputs=[m for m in body.get('messages',[]) if m.get('role')=='tool']
        steps=[
            ('tool_describe',{'names':['computer_use_js','computer_use_js_reset']}),
            ('computer_use_js',{'code':"throw new Error('EXPECTED_KERNEL_ERROR')"}),
            ('computer_use_js',{'code':'let regressionValue = 41; nodeRepl.write(regressionValue + 1)'}),
            ('computer_use_js',{'code':'nodeRepl.write(regressionValue + 2)'}),
            ('computer_use_js_reset',{}),
            ('computer_use_js',{'code':'nodeRepl.write(typeof regressionValue); nodeRepl.write(typeof process); nodeRepl.write(typeof require)'})]
        if os.name=='nt':
            protected=str(self.protected).replace("'","''")
            steps.append(('pwsh',{'command':"Set-Content -LiteralPath (Join-Path $pwd 'scope-proof.txt') -Value 'scoped' -Encoding UTF8 -NoNewline; try { Set-Content -LiteralPath '"+protected+"' -Value 'bad' -ErrorAction Stop } catch { Write-Output 'BOUNDARY_PRESERVED' }",'description':'Verify selected workspace remains confined','workdir':str(self.workspace),'timeout_ms':10000}))
        main=any(m.get('role')=='user' and m.get('content')==TASK for m in body.get('messages',[]))
        if main and len(outputs)<len(steps):
            name,args=steps[len(outputs)]
            delta={'role':'assistant','tool_calls':[{'index':0,'id':uuid.uuid4().hex,'type':'function','function':{'name':name,'arguments':json.dumps(args)}}]};finish='tool_calls'
        else:delta={'role':'assistant','content':'COMPUTER_JS_READY' if main else 'JS fixture'};finish='stop'
        payload=('data: '+json.dumps({'id':'js-fixture','choices':[{'index':0,'delta':delta,'finish_reason':finish}]})+'\n\ndata: [DONE]\n\n').encode()
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(payload)));self.end_headers();self.wfile.write(payload)

def main():
    import sys
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--workdir',type=pathlib.Path,required=True);parser.add_argument('--node',type=pathlib.Path);args=parser.parse_args()
    binary=args.binary.resolve();node=args.node or binary.parent/'runtime/node'/('node.exe' if os.name=='nt' else 'node')
    assert node.is_file(),'Packaged Node runtime is missing'
    run=args.workdir.resolve()/str(time.time_ns());workspace=run/'workspace';workspace.mkdir(parents=True);home=run/'home';env=isolated_environment(run,home)
    env['DSH_NODE_COMMAND']=str(node.resolve())
    Model.workspace=workspace;Model.protected=run/'protected.txt';Model.protected.write_text('preserve',encoding='utf-8')
    if os.name=='nt':
        (home/'windows-sandbox.json').write_text(json.dumps({'version':1,'backend':'windows-native','runner':str(run/'missing-native.exe'),'stateDirectory':str(run/'private-native-state'),'workspaces':[str(run/'different-workspace')],'sha256':'0'*64,'commandRunnerSha256':'0'*64,'setupSha256':'0'*64}),encoding='utf-8')
    server=ThreadingHTTPServer(('127.0.0.1',0),Model);threading.Thread(target=server.serve_forever,daemon=True).start()
    settings={'computer-use':{'enabled':True,'adapter':'command','command':sys.executable},'llm-pi-ai':{'providers':{'js-fixture':{'keyless':True,'api':'openai-completions','baseURL':f'http://127.0.0.1:{server.server_port}/v1','models':[{'id':'js-fixture','contextWindow':131072,'maxTokens':4096}]}}},'agent-default-model':{'provider':'js-fixture','model':'js-fixture'}}
    (home/'settings.json').write_text(json.dumps(settings),encoding='utf-8')
    try:
        with running_fixture_host(binary,run,env,None,'computer-js') as port:
            def call(method,payload):return require_ok(rpc(port,method,payload,uuid.uuid4().hex),method)
            sid=call('session.create',{'cwd':str(workspace)})['sessionId']
            contract={'action':'create','sessionId':sid,'taskId':'js-regression','idempotencyKey':'create-js-regression','contract':{'objective':'Verify persistent isolated JavaScript','acceptanceChecks':[{'id':'kernel-result','description':'Kernel has no process global','checker':{'kind':'tool_result','step_id':'tool:computer_use_js','assertions':{'/logs/0/value':'undefined'}}}]}}
            request=urllib.request.Request(f'http://127.0.0.1:{port}/__dsh-task-execution',data=json.dumps(contract).encode(),headers={'Content-Type':'application/json','Origin':f'http://127.0.0.1:{port}'})
            with urllib.request.urlopen(request,timeout=30) as response:json.load(response)
            call('session.prompt',{'sessionId':sid,'mode':'queue','content':[{'type':'text','text':TASK}]})
            deadline=time.monotonic()+90
            while time.monotonic()<deadline:
                history=call('session.history',{'sessionId':sid});events=[row['event'] for row in history['events']]
                if any(e['type']=='turn/end' for e in events):break
                time.sleep(.1)
            else:raise AssertionError('JS regression turn did not finish')
            calls={e['data']['callId']:e['data']['name'] for e in events if e['type']=='tool/call'}
            results=[p for e in events if e['type']=='tool/result' for p in e['data']['message']['content'] if p['type']=='tool-result' and calls.get(p['toolCallId'])=='computer_use_js']
            assert len(results)==4,results
            assert results[0].get('isError') and 'EXPECTED_KERNEL_ERROR' in json.dumps(results[0]),results[0]
            assert all(not p.get('isError') for p in results[1:]),results[1:]
            values=[json.loads(next(c['text'] for c in p['content'] if c['type']=='text')) for p in results[1:]]
            assert values[0]['logs'][0]['value']==42 and values[1]['logs'][0]['value']==43,values
            assert [row['value'] for row in values[2]['logs']]==['undefined']*3,values
            request=urllib.request.Request(f'http://127.0.0.1:{port}/__dsh-task-execution',data=json.dumps({'action':'list','sessionId':sid}).encode(),headers={'Content-Type':'application/json','Origin':f'http://127.0.0.1:{port}'})
            with urllib.request.urlopen(request,timeout=30) as response:task=json.load(response)['tasks'][0]
            assert not any(step['state']=='unknown' for step in task['steps']),task
            assert any(step['state']=='not_dispatched' for step in task['steps']),task
            if os.name=='nt':
                assert (workspace/'scope-proof.txt').read_text(encoding='utf-8-sig')=='scoped'
                assert Model.protected.read_text(encoding='utf-8')=='preserve'
            (run/'evidence.json').write_text(json.dumps({'persistentValues':[42,43],'reset':True,'noProcessOrRequire':True,'failedComputationHasNoUnknownEffects':True}),encoding='utf-8')
            print('PASS actual Host JS kernel: canonical Windows paths, persistence, reset, isolated globals and effect receipts')
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
