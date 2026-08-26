import fs from 'node:fs/promises'

const gui = process.env.DSH_PREVIEW_GUI || 'http://127.0.0.1:58081/'
const debug = process.env.DSH_CDP || 'http://127.0.0.1:9223'
const evidenceDir = new URL('./preview-evidence/', import.meta.url)
await fs.mkdir(evidenceDir, { recursive: true })

const targets = await (await fetch(`${debug}/json/list`)).json()
const target = targets.find((item) => item.type === 'page')
if (!target) throw new Error('no CDP page target')
const ws = new WebSocket(target.webSocketDebuggerUrl)
await new Promise((resolve, reject) => {
  ws.addEventListener('open', resolve, { once: true })
  ws.addEventListener('error', reject, { once: true })
})
let nextId = 1
const pending = new Map()
const browserErrors = []
const contexts = new Map()
ws.addEventListener('message', (event) => {
  const message = JSON.parse(String(event.data))
  if (message.id) {
    const pair = pending.get(message.id)
    if (!pair) return
    pending.delete(message.id)
    if (message.error) pair.reject(new Error(message.error.message))
    else pair.resolve(message.result)
    return
  }
  if (message.method === 'Runtime.exceptionThrown') {
    browserErrors.push(message.params.exceptionDetails.text || 'Runtime exception')
  }
  if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
    browserErrors.push(message.params.args.map((arg) => arg.value || arg.description || '').join(' '))
  }
  if (message.method === 'Runtime.executionContextCreated') {
    const ctx = message.params.context
    contexts.set(ctx.id, ctx)
  }
  if (message.method === 'Runtime.executionContextDestroyed') contexts.delete(message.params.executionContextId)
})
function cdp(method, params = {}) {
  const id = nextId++
  ws.send(JSON.stringify({ id, method, params }))
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }))
}
async function evaluate(expression, options = {}) {
  const result = await cdp('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
    ...options,
  })
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text || 'evaluation failed')
  return result.result.value
}
async function waitFor(expression, label, timeout = 20000) {
  const deadline = Date.now() + timeout
  let last
  while (Date.now() < deadline) {
    try {
      last = await evaluate(expression)
      if (last) return last
    } catch (error) {
      last = String(error)
    }
    await new Promise((resolve) => setTimeout(resolve, 150))
  }
  throw new Error(`timeout waiting for ${label}: ${JSON.stringify(last)}`)
}
async function click(selector, label = selector) {
  const ok = await evaluate(`(() => { const el=document.querySelector(${JSON.stringify(selector)}); if(!el)return false; el.click(); return true })()`)
  if (!ok) throw new Error(`missing click target ${label}`)
}
async function clickText(selector, text, label = text) {
  const ok = await evaluate(`(() => { const el=[...document.querySelectorAll(${JSON.stringify(selector)})].find((x)=>(x.textContent||'').trim().includes(${JSON.stringify(text)})); if(!el)return false; el.click(); return true })()`)
  if (!ok) throw new Error(`missing text click ${label}`)
}
async function setTextarea(selector, value) {
  const ok = await evaluate(`(() => { const el=document.querySelector(${JSON.stringify(selector)}); if(!el)return false; const set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set; set.call(el,${JSON.stringify(value)}); el.dispatchEvent(new Event('input',{bubbles:true})); return true })()`)
  if (!ok) throw new Error(`missing textarea ${selector}`)
}
async function screenshot(name) {
  const shot = await cdp('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
  await fs.writeFile(new URL(`${name}.png`, evidenceDir), Buffer.from(shot.data, 'base64'))
}

await cdp('Page.enable')
await cdp('Runtime.enable')
await cdp('Network.enable')
await cdp('Page.navigate', { url: gui })
await waitFor(`document.readyState === 'complete'`, 'page load')
await waitFor(`!!document.querySelector('button[aria-label="打开工作区预览"]')`, 'preview launch button', 30000)
await click('button[aria-label="打开工作区预览"]')
await waitFor(`!!document.querySelector('.dwp-shell')`, 'preview panel')
await waitFor(`document.querySelectorAll('.dwp-row').length >= 4`, 'root file listing')
const rootListing = await evaluate(`[...document.querySelectorAll('.dwp-row')].map((el)=>(el.textContent||'').trim())`)
if (rootListing.some((text) => /\.env|\.git|node_modules/i.test(text))) throw new Error('sensitive entries leaked into root listing')

await clickText('.dwp-row', 'README.md')
await waitFor(`document.querySelector('.dwp-markdown')?.textContent.includes('预览验收')`, 'Markdown rendering')
await evaluate(`(() => { const root=document.querySelector('.dwp-markdown'); const walker=document.createTreeWalker(root,NodeFilter.SHOW_TEXT); let node; while(node=walker.nextNode()){const at=node.data.indexOf('请选择这段文字');if(at>=0){const range=document.createRange();range.setStart(node,at);range.setEnd(node,at+'请选择这段文字'.length);const sel=getSelection();sel.removeAllRanges();sel.addRange(range);root.dispatchEvent(new MouseEvent('mouseup',{bubbles:true}));return true}}return false })()`)
await waitFor(`!!document.querySelector('.dwp-annotation')`, 'Markdown annotation card')
await setTextarea('.dwp-note', '改成更明确的验收说明')
await clickText('.dwp-ann-actions button', '发送到对话')
await waitFor(`[...document.querySelectorAll('textarea')].some((el)=>el.value.includes('改成更明确的验收说明'))`, 'annotation draft handoff')

await clickText('.dwp-crumb', '工作区')
await clickText('.dwp-row', 'assets')
await waitFor(`[...document.querySelectorAll('.dwp-row')].some((el)=>el.textContent.includes('sample.png'))`, 'asset directory')
await clickText('.dwp-row', 'sample.png')
await waitFor(`document.querySelector('.dwp-image')?.naturalWidth === 120`, 'image preview')

await clickText('.dwp-crumb', '工作区')
await clickText('.dwp-row', 'index.html')
await waitFor(`!!document.querySelector('.dwp-frame')`, 'HTML frame')
await waitFor(`document.querySelector('.dwp-frame')?.src.includes('/__dsh-preview/site/')`, 'isolated site URL')
await new Promise((resolve) => setTimeout(resolve, 1200))
const childContext = [...contexts.values()].find((ctx) => ctx.auxData?.frameId && !ctx.auxData?.isDefault === false && ctx.auxData?.type !== 'isolated')
const frameContexts = [...contexts.values()].filter((ctx) => ctx.auxData?.frameId && ctx.auxData?.isDefault)
let siteContext
for (const ctx of frameContexts) {
  try {
    const href = await evaluate('location.href', { contextId: ctx.id })
    if (String(href).includes('/__dsh-preview/site/')) siteContext = ctx
  } catch {}
}
if (!siteContext) throw new Error('site execution context not found')
const siteLoaded = await evaluate(`document.body.dataset.scriptLoaded === 'true' && document.getElementById('mark')?.textContent === '脚本已加载'`, { contextId: siteContext.id })
if (!siteLoaded) throw new Error('site JS/CSS resources did not load')
await click('button[title="元素标注"]')
await evaluate(`document.getElementById('mark').click(); true`, { contextId: siteContext.id })
await waitFor(`document.querySelector('.dwp-annotation')?.textContent.includes('元素')`, 'element annotation card')
await setTextarea('.dwp-note', '按钮文案改为完成')
await clickText('.dwp-ann-actions button', '发送到对话')
await waitFor(`[...document.querySelectorAll('textarea')].some((el)=>el.value.includes('按钮文案改为完成'))`, 'element annotation handoff')

await evaluate(`(() => { const shell=document.querySelector('.dwp-shell'); const dt=new DataTransfer(); dt.items.add(new File(['drop e2e'],'拖入验收.txt',{type:'text/plain'})); shell.dispatchEvent(new DragEvent('dragenter',{bubbles:true,cancelable:true,dataTransfer:dt})); shell.dispatchEvent(new DragEvent('drop',{bubbles:true,cancelable:true,dataTransfer:dt})); return true })()`)
await waitFor(`[...document.querySelectorAll('textarea')].some((el)=>el.value.includes('.dsh-drops/'))`, 'drop upload draft handoff', 30000)

await clickText('.dwp-crumb', '工作区')
await clickText('.dwp-project button', '运行')
await waitFor(`!!document.querySelector('.dwp-confirm')`, 'project confirmation card')
const command = await evaluate(`document.querySelector('.dwp-command')?.textContent`)
if (!/npm\s+run\s+dev/.test(command || '')) throw new Error(`unexpected detected command: ${command}`)
await clickText('.dwp-confirm-actions button', '确认运行')
await waitFor(`['running','failed','completed'].includes(document.querySelector('.dwp-project-state')?.textContent?.trim())`, 'project launch status', 30000)
await new Promise((resolve) => setTimeout(resolve, 2200))
const projectStatus = await evaluate(`document.querySelector('.dwp-project-state')?.textContent?.trim()`)
const projectLog = await evaluate(`document.querySelector('.dwp-log')?.textContent || ''`)
if (projectStatus === 'running') await clickText('.dwp-project button', '停止')

await screenshot('preview-panel-e2e')
const final = {
  rootListing,
  command,
  projectStatus,
  projectLog: String(projectLog).slice(-2000),
  browserErrors,
  draftPreview: await evaluate(`[...document.querySelectorAll('textarea')].map((el)=>el.value).find((value)=>value.includes('预览批注'))?.slice(-3000) || ''`),
  panelTitle: await evaluate(`document.querySelector('.dwp-title')?.textContent || ''`),
}
await fs.writeFile(new URL('result.json', evidenceDir), JSON.stringify(final, null, 2))
console.log(JSON.stringify(final, null, 2))
if (browserErrors.length) throw new Error(`browser errors: ${browserErrors.join(' | ')}`)
ws.close()
