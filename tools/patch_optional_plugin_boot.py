"""Apply the readable optional-plugin startup adapter to the pinned Web shell."""
from pathlib import Path
import hashlib,re
ROOT=Path(__file__).resolve().parents[1]
assets=ROOT/'web/dist/assets'
bundles=list(assets.glob('index-*.js'))
if len(bundles)!=1: raise RuntimeError('expected one pinned Web shell bundle')
bundle=bundles[0]
source=bundle.read_text(encoding='utf-8')
marker='__dshBootPluginEntries'
if marker not in source:
    begin=source.index('async runPluginBoot(')
    end=source.index('assertEntriesActive(){',begin)
    old=source[begin:end]
    expected='const u=[Ol,...this.manifest.plugins.map(c=>c.id).filter(c=>c!==Ol),Fl];'
    if expected not in old or 'await Promise.all(u.map(' not in old: raise RuntimeError('Web shell startup contract changed; review the adapter')
    replacement='async runPluginBoot(r){const i=this.ctx;await i.plugin(x6);const s=i.loader;s.internal=this.modules,i.on("internal/status",c=>{const h=c.entry;h===void 0||h.fiber===void 0||this.status.set(h.options.name,u3[h.fiber.state])}),await r;'+expected+'await __dshBootPluginEntries(s,u,this.status,__dshOptionalPluginIds(globalThis.__DSH_BOOT__),()=>this.assertEntriesActive())}'
    source=source[:begin]+replacement+source[end:]
    source='import{bootPluginEntries as __dshBootPluginEntries,optionalClientPluginIds as __dshOptionalPluginIds}from"./optional-plugin-boot.js";\n'+source
    # The vendored source map no longer maps the adapted boot method.
    source=source.replace('//# sourceMappingURL='+bundle.name+'.map','')
    bundle.write_text(source,encoding='utf-8')
helper=(ROOT/'web/src/optional-plugin-boot.js').read_bytes()
(assets/'optional-plugin-boot.js').write_bytes(helper)
revision=hashlib.sha256(helper).hexdigest()[:16]
source=re.sub(r'\./optional-plugin-boot\.js(?:\?rev=[a-f0-9]+)?',f'./optional-plugin-boot.js?rev={revision}',source)
bundle.write_text(source,encoding='utf-8')
index=ROOT/'web/dist/index.html'
html=index.read_text(encoding='utf-8')
revision=hashlib.sha256(bundle.read_bytes()).hexdigest()[:16]
html=re.sub(re.escape('/assets/'+bundle.name)+r'(?:\?rev=[a-f0-9]+)?','/assets/'+bundle.name+'?rev='+revision,html)
index.write_text(html,encoding='utf-8')
print('patched optional client plugin startup; core readiness remains strict')
