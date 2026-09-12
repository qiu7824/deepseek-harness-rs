"""Exercise the native Devin adapter and local tool round trip without real accounts."""
from __future__ import annotations
import argparse, gzip, json, pathlib, struct, subprocess, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from e2e_model_management import isolated_environment

def integer(value):
    out=bytearray()
    while value>=128:out.append((value&127)|128);value>>=7
    out.append(value);return bytes(out)

def field(number,value):
    if isinstance(value,int):return integer(number<<3)+integer(value)
    if isinstance(value,str):value=value.encode('utf-8')
    return integer(number<<3|2)+integer(len(value))+value

def message(data):
    offset=0;result={}
    def read_int():
        nonlocal offset
        value=0
        for shift in range(0,70,7):
            byte=data[offset];offset+=1;value|=(byte&127)<<shift
            if not byte&128:return value
        raise AssertionError('invalid protobuf integer')
    while offset<len(data):
        key=read_int();kind=key&7
        if kind==0:value=read_int()
        elif kind==2:
            size=read_int();value=data[offset:offset+size];offset+=size
        elif kind in (1,5):size=8 if kind==1 else 4;value=data[offset:offset+size];offset+=size
        else:raise AssertionError('unsupported wire kind')
        result.setdefault(key>>3,[]).append(value)
    return result

def text(fields,key):return fields.get(key,[b''])[-1].decode('utf-8')
def frame(flags,data):return bytes([flags])+struct.pack('>I',len(data))+data

class Model(BaseHTTPRequestHandler):
    records=[]
    paths=[]
    evidence=None
    def log_message(self,*args):pass
    def send_payload(self,data,kind):
        self.send_response(200);self.send_header('Content-Type',kind);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def do_POST(self):
        Model.paths.append(self.path)
        if Model.evidence:Model.evidence.write_text(json.dumps({'paths':Model.paths,'records':Model.records}),encoding='utf-8')
        size=int(self.headers.get('Content-Length','0'))
        if size>8*1024*1024:self.send_error(413);return
        raw=self.rfile.read(size)
        if self.path.endswith('/GetUserJwt'):
            metadata=message(message(raw)[1][0]);assert text(metadata,3)=='devin-session-token$fixture-only-token'
            self.send_payload(field(1,'fixture-user-jwt'),'application/proto');return
        if not self.path.endswith('/GetChatMessage'):self.send_error(404);return
        assert len(raw)>=5 and int.from_bytes(raw[1:5],'big')==len(raw)-5
        fields=message(gzip.decompress(raw[5:]) if raw[0]&1 else raw[5:])
        assert text(fields,21)=='swe-2-fixture'
        prompts=[message(data) for data in fields.get(3,[])]
        has_result=any(prompt.get(2)==[4] and 'DEVIN_FILE_MARKER' in text(prompt,3) for prompt in prompts)
        tools=[text(message(value),1) for value in fields.get(10,[])]
        Model.records.append({'cascade':text(fields,16),'tools':tools,'hasToolResult':has_result,
            'nativeSignatureReplayed':any(text(prompt,12)=='fixture-signature' for prompt in prompts)})
        if Model.evidence:Model.evidence.write_text(json.dumps({'paths':Model.paths,'records':Model.records}),encoding='utf-8')
        reply=field(1,'response-'+str(len(Model.records)))
        if not tools:reply+=field(3,'Helper')+field(5,2)
        elif has_result:reply+=field(3,'DEVIN_FIXTURE_DONE')+field(5,2)
        elif any(prompt.get(2)==[4] for prompt in prompts):
            failure=next(text(prompt,3) for prompt in prompts if prompt.get(2)==[4])
            reply+=field(3,'FIXTURE_TOOL_FAILED: '+failure)+field(5,2)
        else:
            assert 'read' in tools
            definition=next(message(value) for value in fields[10] if text(message(value),1)=='read')
            assert 'file_path' in json.loads(text(definition,3))['properties']
            reply+=field(9,'Read the supplied test file.')+field(10,'fixture-signature')
            reply+=field(6,field(1,'read-file')+field(2,'read')+field(3,json.dumps({'file_path':'probe.txt'})))+field(5,10)
        reply+=field(7,field(2,10)+field(3,5)+field(5,20))
        self.send_payload(frame(1,gzip.compress(reply))+frame(2,b'{}'),'application/connect+proto')

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--workdir',type=pathlib.Path,required=True);args=parser.parse_args()
    work=args.workdir.resolve()/('run-'+str(time.time_ns()));work.mkdir(parents=True)
    home=work/'home';env=isolated_environment(work,home);env['DEVIN_FIXTURE_KEY']='fixture-only-token'
    (work/'probe.txt').write_text('DEVIN_FILE_MARKER',encoding='utf-8')
    server=ThreadingHTTPServer(('127.0.0.1',0),Model);threading.Thread(target=server.serve_forever,daemon=True).start()
    Model.evidence=work/'requests.json'
    settings={'llm-pi-ai':{'providers':{'devin-fixture':{'api':'devin-agent','baseURL':f'http://127.0.0.1:{server.server_port}',
        'apiKeyEnv':'DEVIN_FIXTURE_KEY','models':[{'id':'swe-2-fixture','contextWindow':65536,'maxTokens':2048}]}}},
        'agent-default-model':{'provider':'devin-fixture','model':'swe-2-fixture'}}
    (home/'settings.json').write_text(json.dumps(settings),encoding='utf-8')
    try:
        try:
            result=subprocess.run([str(args.binary.resolve()),'--profile','headless','Read probe.txt and report the marker.'],cwd=work,env=env,capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=90,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
        except subprocess.TimeoutExpired as error:
            def decoded(value):return value.decode('utf-8',errors='replace') if isinstance(value,bytes) else value or ''
            (work/'host-timeout.log').write_text(decoded(error.stdout)+decoded(error.stderr),encoding='utf-8')
            raise
        (work/'host.log').write_text(result.stdout+result.stderr,encoding='utf-8')
        assert result.returncode==0,result.stderr[-2000:]
        assert 'DEVIN_FIXTURE_DONE' in result.stdout,result.stdout[-2000:]
        assert 'fixture-only-token' not in result.stdout+result.stderr
        task=[record for record in Model.records if record['tools']]
        assert len(task)==2,Model.records
        assert task[0]['cascade']==task[1]['cascade'] and task[1]['hasToolResult'] and task[1]['nativeSignatureReplayed'],task
        report={'passed':True,'realProviderCalls':0,'nativeRequests':len(task),'toolRoundTrip':True,'signedReplay':True}
        (work/'evidence.json').write_text(json.dumps(report),encoding='utf-8');print(json.dumps(report))
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
