const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const source = fs.readFileSync(process.env.DSH_RUNTIME_PLUGINS_DIR ? path.join(process.env.DSH_RUNTIME_PLUGINS_DIR, 'client-runtime.js') : path.join(__dirname, '../../web/dist/plugins/client-runtime.js'), 'utf8');
const start = source.indexOf('function sessionReferenceParts('), end = source.indexOf('\n\t\tvar SessionManager = class', start);
const context = { console, resolvedClientTimeZone: () => 'Asia/Shanghai', transportError: error => ({ ok: false, error: { code: 'transport-error', message: error.message } }) };
const trackerStart=source.indexOf('const activePromptRequests ='),trackerEnd=source.indexOf('function resolvedClientTimeZone(',trackerStart);
assert.ok(trackerStart>=0,'runtime plugin must include shared prompt tracking');vm.runInNewContext(source.slice(trackerStart,trackerEnd),context);
vm.runInNewContext(source.slice(start, end), context);
function fixture(address) {
  const session = Object.create(context.Session.prototype), calls = [], pending = [], cancelCalls = [];
  const prompt = payload => { calls.push(payload); return new Promise(resolve => pending.push(resolve)); };
  const cancel = async payload => { cancelCalls.push(payload); return {result:{ok:true}}; };
  Object.assign(session, { sessionId: address?.childSessionId ?? 'main', address, promptInFlight: [], promptRetry: null, promptCancelGeneration: 0,
    queueMirror: new context.SessionQueueMirror(), notifier: { markDirty() {} }, returnLatest: async () => {},
    api: { sessions: { prompt, cancel, updateQueue: async payload => { calls.push(payload); return { result: { ok: true } }; } }, subagents: { prompt, interrupt: cancel } },
    handleRunning(value) { this.running = value; }, runningRevision: 0, blankBit: false });
  return { session, calls, pending, cancelCalls };
}
(async () => {
  const content = [{ type: 'text', text: 'queue fixture' }];
  {
    const { session, calls } = fixture();
    session.returnLatest = async () => { throw new Error('history unavailable'); };
    const result = await session.prompt(content, 'queue');
    assert.equal(result.ok, false); assert.equal(session.promptError.op, 'send');
    assert.equal(calls.length, 0, 'failed history synchronization is reported before admission');
    assert.equal(session.queueMirror.snapshot().length, 0);
  }
  for (const address of [undefined, { parentSessionId: 'parent', childSessionId: 'child', mode: 'continuable' }]) {
    const { session, calls, pending } = fixture(address);
    const first = session.prompt(content, 'steer');
    const duplicate = session.prompt(structuredClone(content), 'steer');
    assert.equal(first, duplicate, 'duplicate clicks share one in-flight request');
    assert.equal(session.queueMirror.snapshot()[0].placement, 'sending');
    session.queueMirror.reset();
    assert.equal(session.queueMirror.snapshot()[0].placement, 'sending', 'reconnect preserves unacknowledged sends');
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(calls.length, 1);
    const requestId = calls[0].requestId;
    assert.ok(requestId);
    assert.equal(address ? calls[0].delivery : calls[0].mode, 'steer');
    session.queueMirror.replace([{ id: 'queue-1', placement: 'queued', message: { id: 'message-1', content, source: { kind: 'user', rpcId: requestId } } }]);
    assert.equal(session.queueMirror.snapshot().length, 1, 'Host acknowledgement merges the local sending row');
    pending.shift()({ result: { ok: true, value: { accepted: true, messageId: 'message-1' } } });
    await first;
    assert.equal(session.queueMirror.snapshot()[0].placement, 'queued');
    await session.updateQueue('queue-1', { kind: 'steer' });
    assert.equal(calls[1].sessionId, address ? 'child' : 'main');
    assert.equal(calls[1].parentSessionId, address?.parentSessionId);
    assert.equal(calls[1].mode, address?.mode);
    session.queueMirror.acceptDurable({ type: 'user/message', data: { id: 'message-1', source: { rpcId: requestId } } });
    assert.equal(session.queueMirror.snapshot().length, 0, 'durable message retires the transient queue row');
    const failed = session.prompt(content, 'queue'); await new Promise(resolve => setImmediate(resolve));
    const failureRequestId = calls.at(-1).requestId;
    pending.shift()({ result: { ok: false, error: { code: 'transport-error', message: 'connection lost' } } }); await failed;
    assert.equal(session.queueMirror.snapshot().length, 0);
    const retry = session.prompt(content, 'queue'); await new Promise(resolve => setImmediate(resolve));
    assert.equal(calls.at(-1).requestId, failureRequestId, 'retry after an uncertain reply reuses admission identity');
    pending.shift()({ result: { ok: true, value: { accepted: true, messageId: 'retry-message' } } }); await retry;
    const cancelled=session.prompt(content,'queue');await new Promise(resolve=>setImmediate(resolve));
    const cancelledId=calls.at(-1).requestId;
    pending.shift()({result:{ok:false,error:{code:'cancelled',message:'original admission was cancelled'}}});await cancelled;
    assert.equal(session.promptRetry,null,'a cancelled logical request must not be retried forever');
    const fresh=session.prompt(content,'queue');await new Promise(resolve=>setImmediate(resolve));
    assert.notEqual(calls.at(-1).requestId,cancelledId,'explicit resending after cancellation creates a new admission');
    pending.shift()({result:{ok:true,value:{accepted:true,messageId:'already-finished',running:false}}});await fresh;
    assert.equal(session.running,false,'an already settled receipt cannot create a false running spinner');
    const late=session.prompt(content,'queue');await new Promise(resolve=>setImmediate(resolve));
    session.runningRevision+=1;session.running=false;
    pending.shift()({result:{ok:true,value:{accepted:true,messageId:'finished-before-reply',running:true}}});await late;
    assert.equal(session.running,false,'a late receipt must not overwrite a newer idle event');
  }
  {
    const { session, pending } = fixture();
    const upload = [{ type: 'image', mediaType: 'image/png', data: 'x'.repeat(8 * 1024 * 1024) }];
    const failed = session.prompt(upload, 'queue');
    await new Promise(resolve => setImmediate(resolve));
    pending.shift()({result:{ok:false,error:{code:'attachment-error',details:{reason:'IMAGE_TOO_LARGE'}}}});
    await failed;
    assert.equal(session.promptRetry, null, 'a definite attachment rejection must release the encoded upload');
    assert.equal(session.promptInFlight.length, 0);
    assert.equal(session.queueMirror.snapshot().length, 0);
  }
  for (const address of [undefined, {parentSessionId:'parent',childSessionId:'child',mode:'continuable'}]) {
    const {session,calls,pending,cancelCalls}=fixture(address);let finishHistory;
    session.returnLatest=()=>new Promise(resolve=>{finishHistory=resolve});
    const held=session.prompt(content,'queue');const heldId=session.promptInFlight[0].requestId;await session.cancel();finishHistory();
    assert.deepEqual(JSON.parse(JSON.stringify(cancelCalls[0].requestIds)),[heldId],'Stop carries the identity even before the prompt reaches the server');
    assert.equal((await held).error.code,'cancelled');assert.equal(calls.length,0,'Stop fences a prompt waiting on history before backend dispatch');
    session.returnLatest=async()=>{};
    const late=session.prompt(content,'queue');await new Promise(resolve=>setImmediate(resolve));await session.cancel();session.running=false;
    const later=session.prompt(content,'queue');assert.notEqual(late,later,'new explicit sends do not reuse a stopped in-flight request');await new Promise(resolve=>setImmediate(resolve));
    pending.shift()({result:{ok:true,value:{accepted:true,running:true}}});await late;assert.equal(session.running,false,'late success cannot revive the running spinner after Stop');
    pending.shift()({result:{ok:true,value:{accepted:true,running:false}}});await later;
    const failed=session.prompt(content,'queue');await new Promise(resolve=>setImmediate(resolve));await session.cancel();session.promptError={op:'stop',error:{code:'current'}};
    pending.shift()({result:{ok:false,error:{code:'transport-error',message:'old failure'}}});await failed;
    assert.equal(session.promptRetry,null,'cancelled sends cannot repopulate the retry payload');assert.equal(session.promptError.error.code,'current','late failure cannot replace newer stop feedback');
  }
  {
    const {session,calls,pending,cancelCalls}=fixture();
    const first=session.prompt(content,'queue');await new Promise(resolve=>setImmediate(resolve));const originalId=calls[0].requestId;
    const stopReplies=[];session.api.sessions.cancel=payload=>{cancelCalls.push(payload);return new Promise(resolve=>{stopReplies.push(resolve)})};
    const stopping=session.cancel();const waiting=session.prompt([{type:'text',text:'new intent while stopping'}],'queue');
    const waitingId=session.promptInFlight.at(-1).requestId;
    assert.equal(calls.length,1,'new input waits for the in-flight Stop acknowledgement');
    const stoppingAgain=session.cancel();
    const afterStop=session.prompt([{type:'text',text:'explicit new intent after the second Stop'}],'queue');
    const afterStopId=session.promptInFlight.at(-1).requestId;
    stopReplies.shift()({result:{ok:true}});await new Promise(resolve=>setImmediate(resolve));
    assert.equal(cancelCalls.length,2,'a repeated Stop sends identities added while the first Stop was waiting');
    assert.deepEqual(JSON.parse(JSON.stringify(cancelCalls[1].requestIds)),[waitingId]);
    assert.equal(calls.length,1,'newer sends wait until every prior Stop receipt has arrived');
    stopReplies.shift()({result:{ok:true}});await Promise.all([stopping,stoppingAgain]);assert.equal((await waiting).error.code,'cancelled');
    await new Promise(resolve=>setImmediate(resolve));
    assert.equal(calls.at(-1).requestId,afterStopId,'a send explicitly made after the last Stop dispatches after its receipt');
    assert.equal(cancelCalls.some(call=>call.requestIds.includes(afterStopId)),false,'a follow-on Stop batch cannot capture a later explicit send');
    pending.shift()({result:{ok:false,error:{code:'cancelled'}}});await first;
    pending.shift()({result:{ok:true,value:{accepted:true}}});await afterStop;
    assert.equal(calls.length,2);assert.deepEqual(JSON.parse(JSON.stringify(cancelCalls[0].requestIds)),[originalId]);
    const uncertain=session.prompt(content,'queue');await new Promise(resolve=>setImmediate(resolve));const uncertainId=calls.at(-1).requestId;
    session.api.sessions.cancel=async payload=>{cancelCalls.push(payload);return{result:{ok:false,error:{code:'transport-error',message:'uncertain Stop reply'}}}};
    await session.cancel();pending.shift()({result:{ok:true,value:{accepted:true}}});await uncertain;
    session.api.sessions.cancel=async payload=>{cancelCalls.push(payload);return{result:{ok:true}}};await session.cancel();
    assert.deepEqual(JSON.parse(JSON.stringify(cancelCalls.at(-1).requestIds)),[uncertainId],'retrying an uncertain Stop retains the original request IDs after the send settles');
    assert.equal(session.stopRetryIds.length,0);
    session.promptInFlight=Array.from({length:129},(_,index)=>({requestId:'pending-'+index}));const prior=cancelCalls.length;await session.cancel();
    assert.deepEqual(cancelCalls.slice(prior).map(call=>call.requestIds.length),[64,64,1],'bulk cancellation obeys the server input bound without dropping identities');
  }
  {
    const address={parentSessionId:'parent',childSessionId:'child',mode:'continuable'};
    const {session,cancelCalls}=fixture(address);
    const preview=context.trackPromptRequest(address,'preview-prearrival');const upload=context.trackPromptRequest(address,'main-upload-prearrival');
    const sibling=context.trackPromptRequest({...address,childSessionId:'sibling'},'keep-sibling');
    await session.cancel();
    assert.deepEqual(JSON.parse(JSON.stringify(cancelCalls[0].requestIds)).sort(),['main-upload-prearrival','preview-prearrival']);
    assert.equal(preview.cancelled(),true);assert.equal(upload.cancelled(),true);assert.equal(sibling.cancelled(),false,'cross-view Stop remains scoped to the exact child');
    preview.release();upload.release();sibling.release();assert.equal(context.pendingPromptRequestIds(address).length,0,'completed view leases leave no resident request identities');
  }
  {
    const address={parentSessionId:'parent-cross-view',childSessionId:'child-cross-view',mode:'continuable'};
    const {session,calls,pending}=fixture(address), batches=[],stopReplies=[];
    const preview=context.trackPromptRequest(address,'first-preview-request');
    const previewStop=context.stopPromptRequests(address,[preview.requestId],ids=>{batches.push([...ids]);return new Promise(resolve=>stopReplies.push(resolve));});
    const waiting=session.prompt(content,'queue');const waitingId=session.promptInFlight.at(-1).requestId;
    await new Promise(resolve=>setImmediate(resolve));assert.equal(calls.length,0,'main child view waits for a Stop started by the preview');
    const mainStop=session.cancel();
    const fresh=session.prompt([{type:'text',text:'fresh cross-view request'}],'queue');
    stopReplies.shift()({ok:true});await new Promise(resolve=>setImmediate(resolve));
    assert.deepEqual(batches,[[preview.requestId],[waitingId]],'cross-view Stop joins the same serial drain and covers newly reserved IDs');
    assert.equal(calls.length,0);
    stopReplies.shift()({ok:true});await Promise.all([previewStop,mainStop]);
    assert.equal((await waiting).error.code,'cancelled');await new Promise(resolve=>setImmediate(resolve));
    assert.equal(calls.length,1);pending.shift()({result:{ok:true,value:{accepted:true}}});await fresh;
    preview.release();assert.equal(context.pendingPromptStop(address),undefined,'completed Stop retains no per-scope barrier');
    assert.equal(context.pendingPromptRequestIds(address).length,0);
    for(const receipt of [{ok:true,value:{accepted:true,running:true}},{ok:false,error:{code:'transport-error',message:'old transport'}}]){
      const late=session.prompt(content,'queue');await new Promise(resolve=>setImmediate(resolve));
      await context.stopPromptRequests(address,context.pendingPromptRequestIds(address),async()=>({ok:true}));
      session.running=false;session.promptError={op:'stop',error:{code:'current-stop'}};
      pending.shift()({result:receipt});await late;
      assert.equal(session.running,false,'preview Stop fences a late main-view success without reviving its spinner');
      assert.equal(session.promptRetry,null,'preview Stop prevents late main-view failure from retaining a stopped retry');
      assert.equal(session.promptError.error.code,'current-stop');
    }
  }
  console.log('PASS session queues: main/child delivery, sending, reconnect, acknowledgement merge, retry and durable handoff');
})().catch(error => { console.error(error); process.exitCode = 1; });
