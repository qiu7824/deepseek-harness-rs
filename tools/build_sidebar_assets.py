"""Build pinned browser assets from dependencies installed outside the checkout."""
from __future__ import annotations
import argparse,base64,hashlib,json,os,pathlib,subprocess,tempfile

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--node-modules',type=pathlib.Path,required=True);args=parser.parse_args()
    modules=args.node_modules.resolve();root=pathlib.Path(__file__).resolve().parents[1];plugin=root/'release/plugins/dsh-sidebar-workbench-suite';spec=json.loads((plugin/'vendor-src/package.json').read_text(encoding='utf8'))
    versions={}
    for name,version in spec['dependencies'].items():
        actual=json.loads((modules/name/'package.json').read_text(encoding='utf8'))['version']
        if actual!=version:raise ValueError(f'{name}: expected {version}, got {actual}')
        versions[name]=actual
    env=dict(os.environ);env['NODE_PATH']=str(modules)
    outputs={}
    for name in ('editor','markdown','mermaid'):
        output=plugin/'lib'/f'{name}.js'
        subprocess.run(['node',str(modules/'esbuild/bin/esbuild'),str(plugin/'vendor-src'/f'{name}.js'),'--bundle','--minify','--format=iife','--platform=browser','--target=es2022',f'--outfile={output}'],check=True,env=env)
        outputs[f'lib/{name}.js']='sha256:'+hashlib.sha256(output.read_bytes()).hexdigest()
    pdf=modules/'pdfjs-dist'
    assets={kind:{file.name:base64.b64encode(file.read_bytes()).decode('ascii') for file in sorted((pdf/directory).iterdir()) if file.is_file() and file.suffix.lower() not in ('.md','.txt')} for kind,directory in [('cMapUrl','cmaps'),('standardFontDataUrl','standard_fonts'),('wasmUrl','wasm')]}
    with tempfile.TemporaryDirectory(prefix='dsh-pdf-build-',dir=os.environ.get('DSH_BUILD_TEMP')) as temporary:
        asset_module=pathlib.Path(temporary)/'pdf-assets.js'
        asset_module.write_text('export const assets='+json.dumps(assets)+';\nexport const workerSource='+json.dumps((pdf/'build/pdf.worker.min.mjs').read_text(encoding='utf8'))+';\n',encoding='utf8')
        output=plugin/'lib/pdf.js'
        subprocess.run(['node',str(modules/'esbuild/bin/esbuild'),str(plugin/'vendor-src/pdf.js'),'--bundle','--minify','--format=iife','--platform=browser','--target=es2022',f'--alias:dsh-pdf-assets={asset_module}',f'--outfile={output}'],check=True,env=env)
        outputs['lib/pdf.js']='sha256:'+hashlib.sha256(output.read_bytes()).hexdigest()
    (plugin/'vendor-lock.json').write_text(json.dumps({'builder':'esbuild@'+versions.pop('esbuild'),'target':'es2022','format':'iife','dependencies':versions,'outputs':outputs},indent=2)+'\n',encoding='utf8')
    notices=['# Third-party notices','', 'The browser assets include the following packages. License texts are reproduced from their distributed packages.','']
    packages=json.loads((plugin/'vendor-src/package-lock.json').read_text(encoding='utf8'))['packages']
    for relative in sorted(packages):
        if not relative.startswith('node_modules/'):continue
        directory=modules.parent/relative
        manifest=directory/'package.json'
        if not manifest.is_file():continue
        metadata=json.loads(manifest.read_text(encoding='utf8'));name=metadata.get('name',relative);version=metadata.get('version','')
        licenses=[file for file in directory.iterdir() if file.is_file() and file.name.lower().startswith(('license','licence','copying','notice'))]
        notices.extend([f'## {name} {version}', '', 'License: '+str(metadata.get('license','See package distribution')), ''])
        for file in licenses:notices.extend(['### '+file.name,'','```text',file.read_text(encoding='utf8',errors='replace').strip(),'```',''])
    (plugin/'THIRD_PARTY_NOTICES.md').write_text('\n'.join(notices),encoding='utf8')
    print('Built and pinned sidebar browser assets')
if __name__=='__main__':main()
