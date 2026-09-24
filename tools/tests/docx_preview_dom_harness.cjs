const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=path.resolve(process.argv[2]),vendor=path.resolve(process.argv[3]);
const {JSDOM}=require(path.join(modules,'jsdom')),JSZip=require(path.join(vendor,'jszip'));
const dom=new JSDOM('<main id="root"></main>',{url:'http://127.0.0.1',pretendToBeVisual:true,runScripts:'outside-only'});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
Object.assign(dom.window,{TextDecoder,TextEncoder,setImmediate,clearImmediate});
dom.window.HTMLElement.prototype.scrollIntoView=function(){this.dataset.scrolled='true'};
const React=require(path.join(modules,'react')),Client=require(path.join(modules,'react-dom/client'));
const plugin=path.resolve(__dirname,'../../release/plugins/dsh-sidebar-workbench-suite');
dom.window.eval(fs.readFileSync(path.join(plugin,'lib/docx.js'),'utf8'));
const runtime=dom.window.__DSH_SIDEBAR_DOCX__,source=fs.readFileSync(path.join(plugin,'lib/client.js'),'utf8');
assert.ok(runtime);
async function fixture(title){
  const zip=new JSZip();
  zip.file('[Content_Types].xml','<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>');
  zip.file('_rels/.rels','<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rDoc" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>');
  zip.file('word/document.xml',`<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body><w:p><w:r><w:t>${title}</w:t></w:r></w:p><w:tbl><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="8"/><w:bottom w:val="single" w:sz="8"/><w:insideH w:val="single" w:sz="8"/><w:insideV w:val="single" w:sz="8"/></w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>中文表格</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>金额 128.00</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:br w:type="page"/><w:t>第二页正文</w:t></w:r></w:p><w:altChunk r:id="html"/><w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:bottom="1440" w:left="1440" w:right="1440"/></w:sectPr></w:body></w:document>`);
  zip.file('word/_rels/document.xml.rels','<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="html" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/aFChunk" Target="evil.html"/></Relationships>');
  zip.file('word/evil.html','<script>parent.document.body.dataset.executed="yes"</script><iframe src="https://untrusted.invalid"></iframe>');
  zip.file('word/media/dot.png',Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=','base64'));
  const drawing='<w:p><w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="验收图片"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="1" name="dot.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="image"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>';
  zip.file('word/document.xml',(await zip.file('word/document.xml').async('string')).replace('<w:sectPr>',drawing+'<w:sectPr>'));
  zip.file('word/_rels/document.xml.rels',(await zip.file('word/_rels/document.xml.rels').async('string')).replace('</Relationships>','<Relationship Id="image" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/dot.png"/></Relationships>'));
  return zip.generateAsync({type:'uint8array',compression:'DEFLATE'});
}
const root=Client.createRoot(document.getElementById('root')),h=React.createElement,readingPositions=new Map(),requests=[];
let bodies=new Map(),delayed;
const context={React,h,Button:({children,...props})=>h('button',props,children),readingPositions,rememberReading:(k,v)=>readingPositions.set(k,v),fileDraftKey:(k,s,p)=>[k,s,p].join('\0'),AbortController,Uint8Array:dom.window.Uint8Array,endpoint:(kind,s,p)=>`/${kind}?session=${s}&path=${p}`,loadAsset:async(name)=>{assert.equal(name,'docx.js');return runtime},OfficeViewer:()=>h('div',{'data-wps-preview':true},'WPS 打印版式'),fetch:async(url,options)=>{
  requests.push({url,signal:options.signal});
  if(url.includes('late.docx'))return new Promise(resolve=>{delayed=()=>resolve(new Response(bodies.get('a.docx'),{headers:{etag:'late'}}))});
  const name=new URL(url,'http://fixture').searchParams.get('path');
  return new Response(bodies.get(name)||new Uint8Array([1,2,3]),{headers:{etag:'version-'+bodies.get(name)?.length}});
}};
vm.runInNewContext(source.slice(source.indexOf('    async function loadDocxBytes('),source.indexOf('    function visiblePoll('))+'\nthis.DocxViewer=DocxViewer;',context);
const props={path:'a.docx',title:'报告.docx',scope:{sessionId:'owner'}};
async function waitFor(check){for(let i=0;i<100;i++){await React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,15))});if(check())return}throw new Error(document.body.innerHTML.slice(0,1200))}
async function show(p){await React.act(async()=>root.render(h(context.DocxViewer,p)))}
async function click(text){const button=[...document.querySelectorAll('button')].find(b=>b.textContent===text);assert.ok(button,text);await React.act(async()=>button.click())}
(async()=>{
  bodies.set('a.docx',await fixture('第一份报告'));bodies.set('b.docx',await fixture('切换后的报告'));
  await show(props);await waitFor(()=>!document.querySelector('[role="status"]'));
  assert.equal(document.querySelector('[role="alert"]'),null);
  let frame=document.querySelector('iframe');assert.equal(frame.getAttribute('sandbox'),'allow-same-origin');
  assert.ok(frame.contentDocument.body.textContent.includes('中文表格'));assert.ok(frame.contentDocument.body.textContent.includes('金额 128.00'));
  assert.equal(frame.contentDocument.querySelectorAll('section.docx').length,2);
  assert.equal(frame.contentDocument.querySelectorAll('img').length,1);
  assert.ok(frame.contentDocument.querySelector('img').src.startsWith('data:image/png;base64,'));
  assert.equal(frame.contentDocument.querySelectorAll('script,iframe').length,0);
  assert.ok(frame.contentDocument.querySelector('meta[http-equiv="Content-Security-Policy"]'));
  assert.equal(document.body.dataset.executed,undefined);
  assert.ok(requests.every(r=>r.url.startsWith('/file?')),'default preview called an Office conversion route');
  await click('下一页');assert.equal(document.querySelector('[aria-label="DOCX 页码"]').value,'2');
  const select=document.querySelector('[aria-label="DOCX 缩放"]');await React.act(async()=>{select.value='1.5';select.dispatchEvent(new dom.window.Event('change',{bubbles:true}))});
  assert.equal(frame.contentDocument.querySelector('.docx-wrapper').style.zoom,'1.5');
  const oldDoc=frame.contentDocument;await click('刷新');await waitFor(()=>!document.querySelector('[role="status"]'));
  assert.equal(oldDoc.querySelectorAll('section.docx').length,0,'refresh retained the old document DOM');
  await click('打印版式');assert.ok(document.querySelector('[data-wps-preview]'));
  await click('返回文档预览');await waitFor(()=>!document.querySelector('[role="status"]'));
  await show({...props,path:'late.docx'});await waitFor(()=>!!delayed);const pending=requests.at(-1);
  await show({...props,path:'b.docx'});await waitFor(()=>!document.querySelector('[role="status"]'));
  assert.ok(pending.signal.aborted);await React.act(async()=>{delayed();await new Promise(resolve=>setTimeout(resolve,30))});
  assert.ok(document.querySelector('iframe').contentDocument.body.textContent.includes('切换后的报告'));
  await show({...props,path:'bad.docx'});await waitFor(()=>!!document.querySelector('[role="alert"]'));
  assert.ok(document.querySelector('a[download]'));assert.equal(document.querySelector('[data-wps-preview]'),null);
  const bytes=new dom.window.Uint8Array(bodies.get('a.docx')),bomb=bytes.slice();
  for(let i=0;i<bomb.length-46;i++)if(new DataView(bomb.buffer).getUint32(i,true)===0x02014b50){new DataView(bomb.buffer).setUint32(i+24,129*1024*1024,true);break}
  assert.throws(()=>runtime.validateArchive(bomb),/展开后/);
  await React.act(async()=>root.unmount());dom.window.close();
  console.log('PASS real docx-preview rendering, Chinese tables, page breaks, zoom, WPS opt-in, stale response and cleanup');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
