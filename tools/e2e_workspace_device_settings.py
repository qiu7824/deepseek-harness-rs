"""Check workspace scratch routing and selectable native presets in an isolated Host."""
from __future__ import annotations
import argparse,json,pathlib,time
from e2e_model_management import isolated_environment,running_fixture_host
from e2e_settings_model_preserves_data import rpc,require_ok
from e2e_workspace_insights import PreviewClient

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--workdir',type=pathlib.Path,required=True);args=parser.parse_args()
    run=args.workdir.resolve()/str(time.time_ns());project=run/'workspace';project.mkdir(parents=True);scratch=run/'project-scratch';home=run/'home';env=isolated_environment(run,home)
    with running_fixture_host(args.binary.resolve(),run,env,None,'workspace-devices') as port:
        def call(method,payload):return require_ok(rpc(port,method,payload,1),method)
        workspace=call('workspace.create',{'path':str(project)})['workspace']['workspaceId']
        session=call('session.create',{'workspaceId':workspace})['sessionId'];client=PreviewClient(port,session)
        path='/__dsh-artifacts/workspace-settings'
        status,value=client.raw(path,{'path':str(project),'location':str(scratch)});assert status==200,value
        status,value=client.raw(path,{'path':str(project)});assert status==200 and pathlib.Path(value['effectiveLocation']).samefile(scratch),value
        other=run/'other';other.mkdir();status,value=client.raw(path,{'path':str(other)});assert status==200 and not pathlib.Path(value['effectiveLocation']).samefile(scratch),value
        foreign=run/'foreign';foreign.mkdir();(foreign/'input.txt').write_text('preserve');status,value=client.raw(path,{'path':str(project),'location':str(foreign)});assert status==400,value;assert (foreign/'input.txt').read_text()=='preserve'
        presets=call('agentPreset.list',{})['presets'];assert {'minimal','cordis','standard','code'}<={p['id'] for p in presets}
        for preset in ['minimal','cordis']:
            created=call('session.create',{'cwd':str(project),'agentPreset':preset});assert created['agentPreset']==preset,created
        opened=client.ok('terminal-action',body={'sessionId':session,'action':'open','name':'workspace routing'});terminal=opened['id']
        import os
        command='echo route-proof> "%TEMP%\\routing-proof.txt"\r' if os.name=='nt' else 'printf route-proof > "$TMPDIR/routing-proof.txt"\r'
        client.ok('terminal-action',body={'sessionId':session,'terminalId':terminal,'action':'input','text':command})
        deadline=time.monotonic()+20
        while time.monotonic()<deadline and not list(scratch.rglob('routing-proof.txt')):time.sleep(.1)
        assert list(scratch.rglob('routing-proof.txt')),'terminal ignored workspace scratch setting'
        client.ok('terminal-action',body={'sessionId':session,'terminalId':terminal,'action':'close'})
        status,devices=client.raw('/__dsh-devices/status',{});assert status==200,devices;assert isinstance(devices['installed'],bool)
        if devices.get('signedIn') and devices.get('devices'):
            identity=devices['devices'][0]['id']
            status,value=client.raw('/__dsh-devices/bind',{'deviceId':identity});assert status==200,value
            status,value=client.raw('/__dsh-devices/status',{});assert value['boundDeviceId']==identity
            status,value=client.raw('/__dsh-devices/unbind',{});assert status==200,value
        status,value=client.raw('/__dsh-devices/connect',{'deviceId':'not-an-owned-device'});assert status==400,value
        (run/'evidence.json').write_text(json.dumps({'workspaceRouting':True,'nativePresets':['minimal','cordis'],'deviceInstalled':devices['installed'],'deviceListCount':len(devices.get('devices',[])),'remoteConnectionsStarted':0}),encoding='utf-8')
    print('PASS workspace routing, foreign directory protection, native presets and UU device binding:',run)

if __name__=='__main__':main()
