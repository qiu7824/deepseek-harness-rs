"""Retired project-task boundaries, opt-in memory sync and menu persistence."""
from __future__ import annotations
import argparse,http.client,json,pathlib,threading,time,uuid
from http.server import BaseHTTPRequestHandler
from e2e_http import ThreadingHTTPServer
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import rpc,require_ok

def memory_tool_results(messages):
    return [message for message in messages if message.get('role') == 'tool' and message.get('tool_call_id', '').startswith('memory-call-')]

class Provider(BaseHTTPRequestHandler):
    seen_context=False
    seen_tool=False
    def log_message(self,*args):pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        content=json.dumps(body.get('messages',[]),ensure_ascii=False)
        results=memory_tool_results(body.get('messages',[]))
        tools={tool.get('function',{}).get('name') for tool in body.get('tools',[])}
        if tools:
            assert 'project_tasks' not in tools,'retired project tool was advertised'
            assert 'TASK_FIXTURE' not in content,'retired project task file entered model context'
        if not tools:delta={'content':'Fixture memory'};finish='stop'
        elif results:
            assert 'USER_EDIT_MEMORY_FIXTURE' in json.dumps(results),'memory tool result was not returned'
            Provider.seen_tool=True;delta={'content':'PRODUCTIVITY_FIXTURE_DONE'};finish='stop'
        else:
            assert 'USER_EDIT_MEMORY_FIXTURE' in content,'imported local memory was not included in context'
            Provider.seen_context=True
            if 'memory' in tools:
                name,arguments,prefix='memory',{'action':'list','scope':'default'},'memory-call-'
            else:
                assert 'tool_describe' in tools,'memory tool must be reachable through discovery'
                assert not any(message.get('role')=='tool' and message.get('tool_call_id','').startswith('memory-discovery-') for message in body.get('messages',[])),'discovery did not expose the requested memory tool'
                name,arguments,prefix='tool_describe',{'names':['memory']},'memory-discovery-'
            delta={'tool_calls':[{'index':0,'id':prefix+uuid.uuid4().hex,'type':'function','function':{'name':name,'arguments':json.dumps(arguments)}}]};finish='tool_calls'
        events=[{'choices':[{'index':0,'delta':delta,'finish_reason':None}]},{'choices':[{'index':0,'delta':{},'finish_reason':finish}]}]
        data=(''.join('data: '+json.dumps(v)+'\n\n' for v in events)+'data: [DONE]\n\n').encode()
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',required=True,type=pathlib.Path);parser.add_argument('--workdir',required=True,type=pathlib.Path);parser.add_argument('--check-auto',action='store_true');parser.add_argument('--network',action='store_true');parser.add_argument('--serve',action='store_true');args=parser.parse_args()
    work=args.workdir.resolve()/('run-'+str(time.time_ns()));work.mkdir(parents=True);home=work/'home';env=isolated_environment(work,home)
    task_file=work/'PROJECT_TASKS.md'
    original_tasks=b'# Personal project notes\r\n\r\n<!-- dsh-project-tasks:start -->\r\n- [ ] [P1] [todo] TASK_FIXTURE <!-- task:keep -->\r\n<!-- dsh-project-tasks:end -->\r\n'
    task_file.write_bytes(original_tasks)
    user=pathlib.Path(env['USERPROFILE']);codex=user/'.codex/memories';codex.mkdir(parents=True);memory=codex/'MEMORY.md'
    memory.write_text('# Preference\nPrefer bounded tests. IMPORTED_PREFERENCE_FIXTURE\n# Credentials\napi_key=sk-private-fixture-1234567890123456\n',encoding='utf-8');(codex/'raw_memories.md').write_text('RAW_TRANSCRIPT_MUST_NOT_IMPORT',encoding='utf-8')
    hermes=user/'.hermes/memories';hermes.mkdir(parents=True);hermes_file=hermes/'USER.md';hermes_file.write_text('Prefers deterministic tests. AUTO_INITIAL',encoding='utf-8')
    server=ThreadingHTTPServer(('127.0.0.1',0),Provider);threading.Thread(target=server.serve_forever,daemon=True).start()
    settings={'memory':{'enabled':True,'memoryBudget':2200},'agent-default-model':{'provider':'product-fixture','model':'fixture'},'llm-pi-ai':{'providers':{'product-fixture':{'keyless':True,'api':'openai-completions','baseURL':f'http://127.0.0.1:{server.server_port}/v1','models':[{'id':'fixture','contextWindow':131072,'maxTokens':4096}]}}}}
    (home/'settings.json').write_text(json.dumps(settings),encoding='utf-8')
    try:
        with running_fixture_host(args.binary.resolve(),work,env,None,'productivity') as port:
            counter=0
            def call(method,payload):
                nonlocal counter;counter+=1;return require_ok(rpc(port,method,payload,counter),method)
            def post(action,body={},expected=200,origin=None):
                c=http.client.HTTPConnection('127.0.0.1',port,timeout=30)
                try:
                    c.request('POST','/__dsh-productivity/'+action,json.dumps(body),{'Content-Type':'application/json','Origin':origin or f'http://127.0.0.1:{port}','Sec-Fetch-Site':'same-origin'});response=c.getresponse();data=json.loads(response.read());assert response.status==expected,(action,response.status,data);return data
                finally:c.close()
            workspace=call('workspace.create',{'path':str(work)})['workspace']['workspaceId'];sid=call('session.create',{'workspaceId':workspace})['sessionId']
            assert task_file.read_bytes()==original_tasks,'Host startup must preserve legacy project notes'
            for file_exists in (True,False):
                if not file_exists:task_file.unlink()
                for action in ('list','save'):
                    retired=post('tasks/'+action,{'sessionId':sid,'revision':'legacy','tasks':[]},410)
                    assert retired['code']=='feature-retired' and retired['feature']=='project-tasks',retired
                    assert task_file.exists()==file_exists,'retired task API must not create or remove project files'
                    if file_exists:assert task_file.read_bytes()==original_tasks,'retired task API must preserve original bytes'
                post('tasks/save',{'sessionId':sid,'tasks':[]},403,'https://foreign.invalid')
                assert task_file.exists()==file_exists
                if file_exists:assert task_file.read_bytes()==original_tasks
            task_file.write_bytes(original_tasks)
            sources=post('memory/discover')['sources'];source=next(s for s in sources if s['name']=='Codex');hsource=next(s for s in sources if s['name']=='Hermes')
            preview=post('memory/preview',{'sourceId':source['id']});assert 'sk-private-fixture' not in json.dumps(preview) and 'RAW_TRANSCRIPT' not in json.dumps(preview)
            imported=post('memory/import',{'sourceId':source['id'],'revision':preview['revision'],'auto':True});assert imported['imported']>=1
            repeated=post('memory/sync',{'sourceId':source['id']});assert repeated['imported']==0
            entries=call('memory.list',{'scope':'default'})['entries'];entry=next(e for e in entries if 'IMPORTED_PREFERENCE' in e['content']);assert entry['category']=='user-preference'
            entry['content']='USER_EDIT_MEMORY_FIXTURE: prefer bounded tests';call('memory.upsert',{'entry':entry,'expectedRevision':entry['revision']})
            memory.write_text('# Preference\nPrefer new behavior. EXTERNAL_CHANGED\n',encoding='utf-8');post('memory/import',{'sourceId':source['id'],'revision':preview['revision']},400)
            conflict=post('memory/sync',{'sourceId':source['id']});assert conflict['conflicts']==1
            assert any('USER_EDIT_MEMORY_FIXTURE' in e['content'] for e in call('memory.list',{'scope':'default'})['entries'])
            hp=post('memory/preview',{'sourceId':hsource['id']});post('memory/import',{'sourceId':hsource['id'],'revision':hp['revision'],'auto':True})
            hermes_file.write_text('Prefers deterministic tests. AUTO_UPDATED',encoding='utf-8')
            if args.check_auto:
                deadline=time.monotonic()+70
                while time.monotonic()<deadline:
                    if any('AUTO_UPDATED' in e['content'] for e in call('memory.list',{'scope':'default'})['entries']):break
                    time.sleep(1)
                else:raise AssertionError('background memory synchronization did not update the approved source')
            else:post('memory/sync',{'sourceId':hsource['id']})
            call('session.prompt',{'sessionId':sid,'content':[{'type':'text','text':'Review the imported local memory'}],'mode':'queue','requestId':str(uuid.uuid4())})
            deadline=time.monotonic()+30
            while time.monotonic()<deadline:
                history=[v['event'] for v in call('session.history',{'sessionId':sid})['events']]
                ends=[e for e in history if e['type']=='turn/end']
                if ends:assert ends[-1]['data']['reason']['kind']=='completed',ends[-1];break
                time.sleep(.1)
            else:raise AssertionError('memory tool did not finish')
            assert Provider.seen_context and Provider.seen_tool
            assert task_file.read_bytes()==original_tasks,'ordinary model execution must preserve legacy project notes'
            ns=next(n for n in call('settings.describe',{})['namespaces'] if n['ns']=='mini-menu')
            call('settings.mutate',{'ns':'mini-menu','ops':[{'op':'set','path':['artifacts'],'value':False}],'expectedRevision':ns['revision']});assert json.loads((home/'settings.json').read_text(encoding='utf-8'))['mini-menu']['artifacts'] is False
            report={'passed':True,'retiredTaskApis':True,'legacyTaskFilesPreserved':True,'noTaskFileCreated':True,'retiredTaskContextExcluded':True,'staleWriteRejected':True,'foreignOriginRejected':True,'memoryImported':True,'secretsFiltered':True,'manualEditsPreserved':True,'automaticSync':args.check_auto,'memoryContextAndTool':True,'menuSaved':True,'port':port,'sessionId':sid,'work':str(work),'home':str(home)}
            if args.network:report['network']=post('network')
            (work/'evidence.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8');print(json.dumps(report,ensure_ascii=False),flush=True)
            if args.serve:
                (args.workdir.resolve()/'ui-ready.json').write_text(json.dumps(report),encoding='utf-8')
                while not (work/'stop').exists():time.sleep(.5)
    finally:server.shutdown();server.server_close()
if __name__=='__main__':main()
