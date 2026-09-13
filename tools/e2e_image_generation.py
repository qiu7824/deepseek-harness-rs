"""Native image generation, edit, vision and durable attachment fixtures; no paid calls."""
from __future__ import annotations
import argparse,base64,hashlib,http.client,io,json,pathlib,struct,subprocess,threading,time,urllib.parse,uuid,zipfile,zlib
from http.server import BaseHTTPRequestHandler
from e2e_http import ThreadingHTTPServer
from email.parser import BytesParser
from email.policy import default
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import rpc,require_ok

def png(color):
    def part(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data)&0xffffffff)
    return b'\x89PNG\r\n\x1a\n'+part(b'IHDR',struct.pack('>IIBBBBB',64,64,8,6,0,0,0))+part(b'IDAT',zlib.compress((b'\0'+bytes(color)*64)*64))+part(b'IEND',b'')

class Provider(BaseHTTPRequestHandler):
    calls=[]
    image_delay=0
    def log_message(self,*args):pass
    def reply(self,data,kind='application/json'):
        data=data if isinstance(data,bytes) else json.dumps(data).encode()
        self.send_response(200);self.send_header('Content-Type',kind);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def do_GET(self):self.reply({'data':[]})
    def do_POST(self):
        data=self.rfile.read(int(self.headers.get('Content-Length',0)))
        if self.path.endswith('/images/generations'):
            time.sleep(Provider.image_delay)
            body=json.loads(data);assert body['model']=='gpt-image-2.5-sunburst';assert self.headers.get('Authorization')=='Bearer fixture-image-key'
            assert body['n']==2
            Provider.calls.append({'kind':'generate','model':body['model']});self.reply({'data':[{'b64_json':base64.b64encode(png(color)).decode()} for color in [(255,0,0,255),(0,255,0,255)]]});return
        if self.path.endswith('/images/edits'):
            time.sleep(Provider.image_delay)
            assert self.headers.get('Authorization')=='Bearer fixture-image-key'
            message=BytesParser(policy=default).parsebytes(('Content-Type: '+self.headers['Content-Type']+'\r\nMIME-Version: 1.0\r\n\r\n').encode()+data)
            parts={p.get_param('name',header='content-disposition'):p.get_payload(decode=True) for p in message.iter_parts()}
            assert parts['model']==b'gpt-image-2.5-sunburst';assert parts['image[]'].startswith(b'\x89PNG')
            assert sum(p.get_param('name',header='content-disposition')=='image[]' for p in message.iter_parts())==2
            assert parts['mask'].startswith(b'\x89PNG')
            Provider.calls.append({'kind':'edit','referenceBytes':len(parts['image[]'])});self.reply({'data':[{'b64_json':base64.b64encode(png((0,0,255,255))).decode()}]});return
        if not self.path.endswith('/chat/completions'):
            self.send_error(404, 'Fixture endpoint not supported');return
        body=json.loads(data);messages=body['messages'];model=body['model']
        for tool in body.get('tools',[]):
            if tool.get('function',{}).get('name')=='present':
                limits=tool['function']['parameters']['properties']['files'];assert limits['minItems']==1 and limits['maxItems']==8
        if model=='vision-fixture':
            assert any(isinstance(m.get('content'),list) and any(c.get('type')=='image_url' for c in m['content']) for m in messages)
            Provider.calls.append({'kind':'vision','model':model});delta={'content':'VISION_FIXTURE_OK'};finish='stop'
        else:
            # Image rendering metadata must not send unsupported images to a text-only main model.
            assert not any(isinstance(m.get('content'),list) and any(c.get('type')=='image_url' for c in m['content']) for m in messages)
            user=max(i for i,m in enumerate(messages) if m['role']=='user')
            prompt=str(messages[user]['content']);results=[m for m in messages[user+1:] if m['role']=='tool']
            if not body.get('tools'):delta={'content':'Fixture title'};finish='stop'
            elif results:delta={'content':'IMAGE_FIXTURE_DONE'};finish='stop'
            else:
                previous=[]
                for m in messages:
                    if m['role']=='tool':
                        try:
                            v=json.loads(m['content']);previous.extend(v.get('images',[]))
                        except (ValueError,TypeError):pass
                name='generate_image';args={'prompt':'fixture scene','n':2}
                if 'vision-fixture' in prompt:name='consult_model';args={'task':'vision','prompt':'inspect the generated image','reference_images':[previous[-1]['attachment']['attachmentId']]}
                elif previous:args={'prompt':prompt,'reference_images':[v['attachment']['name'] for v in previous[-2:]],'mask':previous[0]['attachment']['attachmentId']}
                delta={'tool_calls':[{'index':0,'id':'image-'+uuid.uuid4().hex,'type':'function','function':{'name':name,'arguments':json.dumps(args)}}]};finish='tool_calls'
        events=[{'choices':[{'index':0,'delta':delta,'finish_reason':None}]},{'choices':[{'index':0,'delta':{},'finish_reason':finish}],'usage':{'prompt_tokens':10,'completion_tokens':5}}]
        self.reply((''.join('data: '+json.dumps(v)+'\n\n' for v in events)+'data: [DONE]\n\n').encode(),'text/event-stream')

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--workdir',type=pathlib.Path,required=True);parser.add_argument('--serve',action='store_true');parser.add_argument('--assignment',choices=['full','none','provider','model'],default='full');args=parser.parse_args()
    Provider.image_delay=5 if args.serve else 0
    work=args.workdir.resolve()/('image-run-'+str(time.time_ns()));work.mkdir(parents=True)
    home=work/'home';env=isolated_environment(work,home);env['IMAGE_FIXTURE_KEY']='fixture-image-key'
    server=ThreadingHTTPServer(('127.0.0.1',0),Provider);threading.Thread(target=server.serve_forever,daemon=True).start()
    base=f'http://127.0.0.1:{server.server_port}/v1'
    settings={'llm-pi-ai':{'providers':{'text-fixture':{'keyless':True,'api':'openai-completions','baseURL':base,'models':[{'id':'text-fixture','contextWindow':131072,'maxTokens':4096}]},'image-fixture':{'api':'openai-completions','apiKeyEnv':'IMAGE_FIXTURE_KEY','baseURL':base,'models':[{'id':'gpt-image-2.5-sunburst'}]},'vision-fixture':{'keyless':True,'api':'openai-completions','baseURL':base,'models':[{'id':'vision-fixture','imageInput':True,'contextWindow':131072,'maxTokens':8192}]}}},'agent-default-model':{'provider':'text-fixture','model':'text-fixture'},'task-models':{'image':{'provider':'image-fixture','model':'gpt-image-2.5-sunburst'},'vision':{'provider':'vision-fixture','model':'vision-fixture'}}}
    if args.assignment in ['none','model']:
        settings['llm-pi-ai']['providers']['text-fixture'].pop('keyless')
        settings['llm-pi-ai']['providers']['text-fixture']['apiKeyEnv']='IMAGE_FIXTURE_KEY'
    assignments={'none':{},'provider':{'provider':'image-fixture'},'model':{'model':'gpt-image-2.5-sunburst'}}
    if args.assignment!='full':settings['task-models']['image']=assignments[args.assignment]
    (home/'settings.json').write_text(json.dumps(settings),encoding='utf-8')
    try:
        with running_fixture_host(args.binary.resolve(),work,env,None,'image-generation') as port:
            count=0
            def call(method,payload):
                nonlocal count;count+=1;return require_ok(rpc(port,method,payload,count),method)
            if args.assignment!='full':
                connection=http.client.HTTPConnection('127.0.0.1',port,timeout=20)
                headers={'Origin':f'http://127.0.0.1:{port}','Sec-Fetch-Site':'same-origin','Content-Type':'application/json'}
                try:
                    connection.request('POST','/task-models/describe','{}',headers);response=connection.getresponse();config=json.loads(response.read());assert response.status==200
                    connection.request('POST','/task-models/save',json.dumps({'routes':settings['task-models'],'revision':config['revision']}),headers);response=connection.getresponse();saved=json.loads(response.read());assert response.status==200,saved
                finally:connection.close()
            workspace=call('workspace.create',{'path':str(work)})['workspace']['workspaceId'];sid=call('session.create',{'workspaceId':workspace})['sessionId']
            history=[]
            for turn,prompt in enumerate(['image-generation-fixture','image-edit-fixture: make it blue','vision-fixture'],1):
                call('session.prompt',{'sessionId':sid,'content':[{'type':'text','text':prompt}],'mode':'queue','requestId':str(uuid.uuid4())})
                deadline=time.monotonic()+70
                while time.monotonic()<deadline:
                    history=[v['event'] for v in call('session.history',{'sessionId':sid})['events']]
                    ends=[e for e in history if e['type']=='turn/end']
                    if len(ends)>=turn:assert ends[-1]['data']['reason']['kind']=='completed',ends[-1];break
                    time.sleep(.1)
                else:raise AssertionError('image fixture did not complete')
            generated=[e for e in history if e['type']=='tool/result' and e['data'].get('meta',{}).get('kind')=='image-generation']
            assert len(generated)==2,[e['data'] for e in history if e['type']=='tool/result']
            ids=[v['attachment']['attachmentId'] for e in generated for v in e['data']['meta']['images']];assert len(set(ids))==3
            for identifier in ids:
                image=call('session.attachment',{'sessionId':sid,'attachmentId':identifier})
                raw=image['data'];raw=base64.b64decode(raw) if isinstance(raw,str) else bytes(raw);assert raw.startswith(b'\x89PNG')
            other=call('session.create',{'workspaceId':workspace})['sessionId'];count+=1
            denied=rpc(port,'session.attachment',{'sessionId':other,'attachmentId':ids[0]},count);assert denied['result']['ok'] is False
            assert [v['kind'] for v in Provider.calls]==['generate','edit','vision'],Provider.calls
            connection=http.client.HTTPConnection('127.0.0.1',port,timeout=20)
            try:
                connection.request('GET','/api/session.export?'+urllib.parse.urlencode({'sessionId':sid,'includeDescendants':'false'}),headers={'Origin':f'http://127.0.0.1:{port}','Sec-Fetch-Site':'same-origin'})
                response=connection.getresponse();archive=response.read(2*1024*1024+1);assert response.status==200 and len(archive)<=2*1024*1024
            finally:connection.close()
            with zipfile.ZipFile(io.BytesIO(archive)) as exported:
                media={hashlib.sha256(exported.read(name)).hexdigest():exported.read(name) for name in exported.namelist() if name.startswith('media/')}
                assert {identifier.removeprefix('sha256:') for identifier in ids}<=media.keys(),exported.namelist()
            zip_path=work/'images.zip';zip_path.write_bytes(archive);destination=work/'imported-home'
            imported=subprocess.run([str(args.binary.resolve()),'history','import',str(zip_path),'--to',str(destination)],env=env,capture_output=True,text=True,encoding='utf-8',timeout=40,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
            assert imported.returncode==0,imported.stderr
            for digest,data in media.items():assert (destination/'attachments/v1/objects'/digest[:2]/digest).read_bytes()==data
            report={'passed':True,'realProviderCalls':0,'generation':True,'editing':True,'multipleImagesAndReferences':True,'maskEdit':True,'visionRoute':True,'mainRemainsTextOnly':True,'sessionAttachmentIsolation':True,'exportImportImages':True,'sessionId':sid,'port':port,'home':str(home),'work':str(work)}
            (work/'evidence.json').write_text(json.dumps(report,indent=2),encoding='utf-8');print(json.dumps(report),flush=True)
            if args.serve:
                (args.workdir.resolve()/'ui-ready.json').write_text(json.dumps(report),encoding='utf-8')
                while not (work/'stop').exists():time.sleep(.5)
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
