"""Verify the real Headless profile, configured model route and default native tools."""
import argparse,json,pathlib,subprocess,threading
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from e2e_model_management import isolated_environment
TASK = 'Return the completion token.'

def is_task_request(body):
    for message in body.get('messages', []):
        content = message.get('content')
        if isinstance(content, list):
            content = ''.join(part.get('text', '') for part in content if part.get('type') == 'text')
        if message.get('role') == 'user' and content == TASK:
            return True
    return False

class Model(BaseHTTPRequestHandler):
    observed=[]
    def log_message(self,*args):pass
    def do_POST(self):
        length=int(self.headers.get('Content-Length','0'))
        if length>2*1024*1024:self.send_error(413);return
        body=json.loads(self.rfile.read(length))
        Model.observed.append({'model':body.get('model'),'kind':'task' if is_task_request(body) else 'auxiliary','tools':[tool.get('function',{}).get('name') for tool in body.get('tools',[])]})
        event={'id':'headless-fixture','choices':[{'index':0,'delta':{'role':'assistant','content':'HEADLESS_TOOL_SNAPSHOT_OK'},'finish_reason':'stop'}]}
        payload=('data: '+json.dumps(event)+'\n\ndata: [DONE]\n\n').encode()
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(payload)));self.end_headers();self.wfile.write(payload)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--workdir',type=pathlib.Path,required=True);args=parser.parse_args()
    work=args.workdir.resolve()
    Model.observed=[]
    if work.exists():raise RuntimeError('use a fresh fixture workdir')
    work.mkdir(parents=True);home=work/'home';env=isolated_environment(work,home);env.pop('DSH_DEEPSEEK_MODEL',None)
    server=ThreadingHTTPServer(('127.0.0.1',0),Model);threading.Thread(target=server.serve_forever,daemon=True).start()
    settings={'llm-pi-ai':{'providers':{'headless-fixture':{'keyless':True,'api':'openai-completions','baseURL':f'http://127.0.0.1:{server.server_port}/v1','models':[{'id':'fixture-model','contextWindow':65536,'maxTokens':1024}]}}},'agent-default-model':{'provider':'headless-fixture','model':'fixture-model'}}
    (home/'settings.json').write_text(json.dumps(settings),encoding='utf-8')
    try:
        result=subprocess.run([str(args.binary.resolve()),'--profile','headless',TASK],cwd=work,env=env,capture_output=True,text=True,encoding='utf-8',timeout=60,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
        (work/'headless.log').write_text(result.stdout+result.stderr,encoding='utf-8')
        assert result.returncode==0,result.stderr[-1000:]
        assert 'HEADLESS_TOOL_SNAPSHOT_OK' in result.stdout
        assert Model.observed and all(row['model']=='fixture-model' for row in Model.observed)
        task_requests=[row for row in Model.observed if row['kind']=='task']
        assert task_requests,Model.observed
        for request in task_requests:
            assert {'read','write','edit'}.issubset(set(request['tools'])),request
        report={'passed':True,'profile':'headless','configuredModel':True,'defaultNativeTools':['read','write','edit'],'modelRequests':Model.observed,'externalProviderRequests':0}
        (work/'evidence.json').write_text(json.dumps(report),encoding='utf-8');print(json.dumps(report))
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
