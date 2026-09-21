"""Check durable context reuse and conservative provider-diagnostic recovery."""
from pathlib import Path
import os,sys,json,time,uuid,threading,hashlib,shutil,argparse
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
sys.dont_write_bytecode=True
sys.path.insert(0,str(Path(__file__).resolve().parent))
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import rpc,require_ok
parser=argparse.ArgumentParser();parser.add_argument('--binary',type=Path,required=True);parser.add_argument('--workdir',type=Path,required=True);options=parser.parse_args();binary=options.binary.resolve();b=options.workdir.resolve();b.mkdir(parents=True,exist_ok=True);runtime=binary.parent;run=b/('run-'+str(time.time_ns()));workspace=run/'workspace';workspace.mkdir(parents=True);home=run/'home';env=isolated_environment(run,home);env['DSH_NODE_COMMAND']=str(runtime/'runtime/node/node.exe');(workspace/'proof.txt').write_text('SENTINEL',encoding='utf-8')
class Model(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));messages=body['messages'];idx,marker=next((i,m['content']) for i,m in reversed(list(enumerate(messages))) if m.get('role')=='user' and m.get('content') in ['FIRST','SECOND','RESTORED']);out=[m for m in messages[idx+1:] if m.get('role')=='tool'];steps=[('read',{'file_path':str(workspace/'proof.txt')})]
  if marker=='FIRST':steps=[('task_execution',{'action':'create','contract':{'objective':'Check stable context','acceptanceChecks':[{'id':'content','description':'sentinel','checker':{'kind':'text','path':str(workspace/'proof.txt'),'required':['SENTINEL']}}]}}),*steps,*steps,*steps]
  if len(out)<len(steps):
   name,args=steps[len(out)];delta={'role':'assistant','tool_calls':[{'index':0,'id':'call_'+uuid.uuid4().hex,'type':'function','function':{'name':name,'arguments':json.dumps(args)}}]};finish='tool_calls'
  else:delta={'role':'assistant','content':'Fixture response'};finish='stop'
  data=('data: '+json.dumps({'id':'fixture','choices':[{'index':0,'delta':delta,'finish_reason':finish}]})+'\n\ndata: [DONE]\n\n').encode();self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
server=ThreadingHTTPServer(('127.0.0.1',0),Model);threading.Thread(target=server.serve_forever,daemon=True).start();(home/'settings.json').write_text(json.dumps({'llm-pi-ai':{'providers':{'fixture':{'keyless':True,'api':'openai-completions','baseURL':f'http://127.0.0.1:{server.server_port}/v1','models':[{'id':'fixture','contextWindow':131072,'maxTokens':4096}]}}},'agent-default-model':{'provider':'fixture','model':'fixture'}}),encoding='utf-8')
seed={'id':'fixture','workspaceKey':'','tool':'','source':'provider','category':'provider','code':'TIMEOUT','provider':'fixture','model':'fixture','providerKey':'','modelKey':'','routeKnown':True,'models':[],'occurrences':1,'firstSeen':1,'lastSeen':1,'lastRecovered':None,'status':'pending','verification':None,'ruleId':'provider-runtime','suggestion':'','message':'Fixture provider timeout','enabled':True,'revision':1,'applicationCount':0,'lastApplied':None,'lastApplicationOutcome':None}
normalized=str(workspace).replace('\\','/').rstrip('/');normalized=normalized.lower() if os.name=='nt' else normalized;key=hashlib.sha256(normalized.encode()).hexdigest();entries=[]
for id,code,model,last in [('recover-me','TIMEOUT','fixture',1),('unsupported','NATIVE_TOOL_UNSUPPORTED','fixture',1),('other-model','TIMEOUT','other',1),('newer-failure','TIMEOUT','fixture',int(time.time()*1000)+120000)]:
 e=dict(seed);e.update(id=id,workspaceKey=key,provider='fixture',model=model,providerKey=hashlib.sha256(b'fixture').hexdigest(),modelKey=hashlib.sha256(model.encode()).hexdigest(),routeKnown=True,firstSeen=last,lastSeen=last,occurrences=1,revision=1,code=code,enabled=True,status='pending',verification=None,models=[]);e.pop('review',None);e.pop('reviewStale',None);entries.append(e)
(home/'memory').mkdir(exist_ok=True);(home/'memory/learning.json').write_text(json.dumps({'version':1,'enabled':True,'revision':1,'configRevision':0,'entries':entries,'seen_events':[]}),encoding='utf-8')
def call(port,m,p):return require_ok(rpc(port,m,p,uuid.uuid4().hex),m)
def turn(port,sid,marker,count):
 call(port,'session.prompt',{'sessionId':sid,'mode':'queue','content':[{'type':'text','text':marker}]});deadline=time.monotonic()+90
 while time.monotonic()<deadline:
  events=[x['event'] for x in call(port,'session.history',{'sessionId':sid})['events']]
  if sum(e['type']=='turn/end' for e in events)>=count:break
  time.sleep(.2)
 else:raise AssertionError('turn timeout')
 (run/(marker+'.json')).write_text(json.dumps(events,ensure_ascii=False,indent=2),encoding='utf-8')
 assert not any(e['type']=='tool/result' and e['data'].get('error') for e in events),[e for e in events if e['type']=='tool/result' and e['data'].get('error')]
 snapshots=[e for e in events if e['type']=='user/message' and e['data'].get('source',{}).get('plugin')=='@deepseek-ai/dsh-system-prompt'];print(marker,'runtime snapshots',len(snapshots),flush=True);return snapshots
try:
 with running_fixture_host(binary,run,env,None,'first') as port:
  sid=call(port,'session.create',{'cwd':str(workspace),'agentPreset':'standard'})['sessionId'];call(port,'session.rename',{'sessionId':sid,'title':'Context stability acceptance'});first=turn(port,sid,'FIRST',1);assert len(first)==3,[(e['seq'],e['data']['source']) for e in first]
  second=turn(port,sid,'SECOND',2);assert len(second)==len(first),'unchanged second turn reinjected context'
  deadline=time.monotonic()+5
  while time.monotonic()<deadline:
   ledger=call(port,'memory.learningList',{'limit':1000});ids={e['id'] for e in ledger['items']}
   if 'recover-me' not in ids:break
   time.sleep(.1)
  assert ids=={'unsupported','other-model','newer-failure'},ids
 with running_fixture_host(binary,run,env,None,'restored') as port:
  restored=turn(port,sid,'RESTORED',3);assert len(restored)==len(first),'restart reinjected unchanged context'
 (b/'context-e2e-result.json').write_text(json.dumps({'runtimeSnapshots':[len(first),len(second),len(restored)],'resolvedProviderRemoved':True,'capabilityOtherRouteAndNewerErrorsPreserved':True,'home':str(home)},indent=2),encoding='utf-8');print('PASS multiple tools, next user turn, cold restart, and route/freshness-fenced automatic recovery',flush=True)
finally:server.shutdown();server.server_close()

