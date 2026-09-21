import argparse,json,os,pathlib,sys,threading,time,uuid,urllib.request
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import rpc,require_ok

entered=threading.Event();release=threading.Event();requests=[]
class Model(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages=body.get('messages',[]);main=any(m.get('role')=='user' and m.get('content')=='PORTABLE_SEARCH' for m in messages)
        stopping=any(m.get('role')=='user' and m.get('content')=='CANCEL_STALLED_STREAM' for m in messages)
        requests.append({'main':main,'stopping':stopping,'at':time.monotonic()})
        if stopping:
            self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers();self.wfile.flush();entered.set();release.wait(40);return
        outputs=[m for m in messages if m.get('role')=='tool']
        steps=[('glob',{'pattern':'*.txt'}),('grep',{'pattern':'PORTABLE_SENTINEL','path':str(workspace)})]
        writes=[i for i,m in enumerate(messages) if m.get('role')=='user' and m.get('content')=='ENV_WRITE']
        if writes:
            main=True;outputs=[m for m in messages[writes[-1]+1:] if m.get('role')=='tool'];steps=[('write',{'file_path':str(workspace/'after.txt'),'content':'MIGRATION_OK'})]
        if any(m.get('role')=='user' and m.get('content')=='CANCEL_HANGING_TOOL' for m in messages):
            main=True;steps=[('tool_describe',{'names':['computer_use_js']}),('computer_use_js',{'code':'while (true) {}'})]
        if main and len(outputs)<len(steps):
            name,args=steps[len(outputs)];delta={'role':'assistant','tool_calls':[{'index':0,'id':uuid.uuid4().hex,'type':'function','function':{'name':name,'arguments':json.dumps(args)}}]};finish='tool_calls'
        else:delta={'role':'assistant','content':'SEARCH_OK' if main else 'fixture'};finish='stop'
        payload=('data: '+json.dumps({'id':'portable','choices':[{'index':0,'delta':delta,'finish_reason':finish}]})+'\n\ndata: [DONE]\n\n').encode()
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(payload)));self.end_headers();self.wfile.write(payload)

p=argparse.ArgumentParser();p.add_argument('--binary',type=pathlib.Path,required=True);p.add_argument('--workdir',type=pathlib.Path,required=True);args=p.parse_args()
root=args.workdir.resolve()/str(time.time_ns());workspace=root/'workspace';workspace.mkdir(parents=True);home=root/'home';env=isolated_environment(root,home)
env['PATH']=str(pathlib.Path(os.environ['SystemRoot'])/'System32')+os.pathsep+str(pathlib.Path(os.environ['SystemRoot'])/'System32/WindowsPowerShell/v1.0')
for key in list(env):
    if key.startswith('DSH_') and key!='DSH_HOME':env.pop(key)
(workspace/'sample.txt').write_text('PORTABLE_SENTINEL\n',encoding='utf-8')
server=ThreadingHTTPServer(('127.0.0.1',0),Model);server.daemon_threads=True;threading.Thread(target=server.serve_forever,daemon=True).start()
settings={'computer-use':{'enabled':True,'adapter':'command','command':sys.executable},'llm-pi-ai':{'providers':{'portable':{'keyless':True,'api':'openai-completions','baseURL':f'http://127.0.0.1:{server.server_port}/v1','models':[{'id':'portable','contextWindow':131072,'maxTokens':4096}]}}},'agent-default-model':{'provider':'portable','model':'portable'}}
(home/'settings.json').write_text(json.dumps(settings),encoding='utf-8')
try:
    with running_fixture_host(args.binary.resolve(),root,env,None,'portable') as port:
        def call(method,payload):return require_ok(rpc(port,method,payload,uuid.uuid4().hex),method)
        def history(sid):return [r['event'] for r in call('session.history',{'sessionId':sid})['events']]
        def prompt(sid,text):return call('session.prompt',{'sessionId':sid,'mode':'queue','content':[{'type':'text','text':text}]})
        def task(sid,action,**extra):
            data={'sessionId':sid,'action':action,'idempotencyKey':uuid.uuid4().hex,**extra};base=f'http://127.0.0.1:{port}'
            request=urllib.request.Request(base+'/__dsh-task-execution',data=json.dumps(data).encode(),headers={'Content-Type':'application/json','Origin':base})
            with urllib.request.urlopen(request,timeout=30) as response:return json.load(response)
        def wait_turn(sid,count):
            end=time.monotonic()+60
            while time.monotonic()<end:
                events=history(sid)
                if len([e for e in events if e['type']=='turn/end'])>=count:return events
                time.sleep(.1)
            raise AssertionError('turn did not finish')
        sid=call('session.create',{'cwd':str(workspace)})['sessionId'];prompt(sid,'PORTABLE_SEARCH')
        deadline=time.monotonic()+60
        while time.monotonic()<deadline:
            events=history(sid)
            if any(e['type']=='turn/end' for e in events):break
            time.sleep(.2)
        else:raise AssertionError('search did not finish')
        calls={e['data']['callId']:e['data']['name'] for e in events if e['type']=='tool/call'}
        results=[p for e in events if e['type']=='tool/result' for p in e['data']['message']['content'] if p['type']=='tool-result' and calls.get(p['toolCallId']) in ('grep','glob')]
        assert len(results)==2,results
        assert all(not r.get('isError') for r in results),results
        assert all('sample.txt' in json.dumps(r) for r in results),results
        sid3=call('session.create',{'cwd':str(workspace)})['sessionId']
        task(sid3,'create',taskId='environment-regression',contract={'objective':'Verify migration retains content acceptance','acceptanceChecks':[{'id':'content','description':'Written content is correct','checker':{'kind':'text','path':str(workspace/'after.txt'),'required':['MIGRATION_OK']}}]})
        call('commands.execute',{'args':{'agentId':sid3,'line':'/permission danger-full-access'}})
        prompt(sid3,'ENV_WRITE');events=wait_turn(sid3,1)
        assert not (workspace/'after.txt').exists(),'permission change bypassed migration'
        assert any('TASK_ENVIRONMENT_CHANGED' in json.dumps(e) for e in events if e['type']=='tool/result')
        current=task(sid3,'get',taskId='environment-regression')['task']
        task(sid3,'migrate_environment',taskId=current['taskId'],revision=current['revision'])
        prompt(sid3,'ENV_WRITE');wait_turn(sid3,2)
        assert (workspace/'after.txt').read_text()=='MIGRATION_OK'
        task(sid3,'validate',taskId='environment-regression')
        completed=task(sid3,'complete',taskId='environment-regression')
        assert completed['task']['state']=='completed',completed
        sid2=call('session.create',{'cwd':str(workspace)})['sessionId'];prompt(sid2,'CANCEL_STALLED_STREAM');assert entered.wait(30)
        prompt(sid2,'QUEUED_BEFORE_STOP')
        started=time.monotonic();call('session.cancel',{'sessionId':sid2});latency=time.monotonic()-started
        assert latency<5,latency
        snapshot=len([r for r in requests if r['stopping']]);time.sleep(2)
        assert len([r for r in requests if r['stopping']])==snapshot,'stop restarted pending input'
        items=call('session.list',{})['items'];assert not next(r for r in items if r['sessionId']==sid2)['running']
        events=history(sid2);assert len([e for e in events if e['type']=='turn/start'])==1,events[-5:]
        sid4=call('session.create',{'cwd':str(workspace)})['sessionId'];prompt(sid4,'CANCEL_HANGING_TOOL')
        end=time.monotonic()+30
        while time.monotonic()<end:
            events=history(sid4)
            if any(e['type']=='tool/call' and e['data']['name']=='computer_use_js' for e in events):break
            time.sleep(.1)
        else:raise AssertionError('hanging tool never started')
        time.sleep(.5);prompt(sid4,'QUEUED_TOOL_STOP');started=time.monotonic();call('session.cancel',{'sessionId':sid4});tool_latency=time.monotonic()-started
        assert tool_latency<5,tool_latency
        time.sleep(1);events=history(sid4);assert len([e for e in events if e['type']=='turn/start'])==1
        result={'cleanPath':env['PATH'],'grep':True,'glob':True,'permissionMigrationAndRevalidation':True,'cancelStalledStreamSeconds':latency,'cancelInfiniteJsSeconds':tool_latency,'queuedInputDoesNotRestart':True}
        (root/'result.json').write_text(json.dumps(result,indent=2),encoding='utf-8');print(json.dumps(result))
finally:release.set();server.shutdown();server.server_close()
