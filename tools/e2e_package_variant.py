"""Verify package UI boundaries, including a previously shared skin profile."""
from __future__ import annotations
import argparse,json,pathlib,re,time,urllib.request,urllib.error
from e2e_model_management import isolated_environment,running_fixture_host

def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=pathlib.Path,required=True);p.add_argument('--workdir',type=pathlib.Path,required=True);p.add_argument('--variant',choices=['core','skin','free'],required=True);args=p.parse_args()
 run=args.workdir.resolve()/str(time.time_ns());run.mkdir(parents=True);home=run/'home';env=isolated_environment(run,home)
 profile=home/'profiles/web';package=profile/'node_modules/dsh-skin-center';(package/'lib').mkdir(parents=True)
 (profile/'package.json').write_text(json.dumps({'name':'dsh-profile-web','private':True,'dependencies':{'dsh-skin-center':'bundled'},'dsh':{'profile':{'bundles':['@deepseek-ai/dsh-base','@deepseek-ai/dsh-web-app']}}}))
 (profile/'plugins.json').write_text(json.dumps([{'id':'dsh-skin-center','name':'dsh-skin-center','disabled':False}]))
 (package/'package.json').write_text(json.dumps({'name':'dsh-skin-center','exports':{'./client':'./lib/client.js'},'dsh':{'client':{'platform':'web','inject':[]}}}))
 (package/'lib/client.js').write_text('window.__ModuleLoader__.load({id:"dsh-skin-center",factory:()=>({apply(){},inject:[]})});')
 with running_fixture_host(args.binary.resolve(),run,env,None,'variant') as port:
  with urllib.request.urlopen(f'http://127.0.0.1:{port}/') as response:html=response.read().decode()
  boot=json.loads(re.search(r'window\.__DSH_BOOT__=(.*?);</script>',html).group(1))
  assert boot['variant']==args.variant,boot['variant']
  entries=boot['entries']+boot.get('availableEntries',[])
  if args.variant!='skin':assert all(entry['id']!='dsh-skin-center' for entry in entries)
  if args.variant!='free':
   try:
    with urllib.request.urlopen(f'http://127.0.0.1:{port}/__dsh-free/models') as response:
     assert 'application/json' not in response.headers.get('Content-Type',''),'free API exposed outside free package'
   except urllib.error.HTTPError as error:assert error.code==404,error.code
  sidebar=next(entry for entry in boot['entries'] if entry['id']=='dsh-better-sidebar')
  asset_url=f'http://127.0.0.1:{port}'+sidebar['url'].removesuffix('.js')+'/git-terminal.js'
  with urllib.request.urlopen(asset_url) as response:
   assert response.status==200 and b'data-dsh-git-terminal-style' in response.read()
  asset=profile/'node_modules/dsh-better-sidebar/lib/git-terminal.js'
  asset.write_bytes(asset.read_bytes()+b'\n// changed fixture revision\n')
  try:urllib.request.urlopen(asset_url)
  except urllib.error.HTTPError as error:assert error.code==409,error.code
  else:raise AssertionError('changed lazy asset was served under an old content identity')
 print('PASS package variant and legacy profile isolation:',args.variant)

if __name__=='__main__':main()
