// Real Host/SDK video transport probe. It never sends desktop input.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const [port, owner, output] = process.argv.slice(2);
const origin = `http://127.0.0.1:${port}`;
const route = `/__dsh-computer-use/stream?ownerSessionId=${encodeURIComponent(owner)}&browserSessionId=default`;
const packets = [], chunks = [], states = [];

(async () => {
  await new Promise((resolve, reject) => {
    const req = http.get(origin + route, {headers:{Origin:'https://untrusted.invalid',Connection:'Upgrade',Upgrade:'websocket','Sec-WebSocket-Version':'13','Sec-WebSocket-Key':Buffer.alloc(16).toString('base64')}}, response => {
      response.resume();
      try { assert.equal(response.statusCode,403); resolve(); } catch(error) { reject(error); }
    });
    req.on('upgrade',(_,socket,head)=>{
      let pending=Buffer.from(head),rejected=false;
      const inspect=()=>{while(pending.length>=2){const opcode=pending[0]&15,length=pending[1]&127;if(length>=126){socket.destroy();reject(new Error('unexpected rejection payload'));return}if(pending.length<length+2)return;const payload=pending.subarray(2,2+length);pending=pending.subarray(2+length);try{assert.notEqual(opcode,2,'untrusted origin received video');if(opcode===1){const value=JSON.parse(payload);assert.equal(value.code,'forbidden');rejected=true}if(opcode===8){assert.equal(payload.readUInt16BE(0),1008);assert.ok(rejected);socket.destroy();resolve();return}}catch(error){socket.destroy();reject(error);return}}};
      socket.setTimeout(5000,()=>{socket.destroy();reject(new Error('origin rejection timed out'))});socket.on('error',reject);socket.on('data',bytes=>{pending=Buffer.concat([pending,bytes]);inspect()});inspect();
    });req.on('error',reject);
  });
  const started = performance.now();
  let requested = false, keyAfterRequest = false;
  await new Promise((resolve,reject) => {
    const socket = new WebSocket(`ws://127.0.0.1:${port}${route}`);
    socket.binaryType = 'arraybuffer';
    const timeout = setTimeout(()=>finish(new Error('video stream timed out')),90000);
    let finished=false;
    function finish(error){if(finished)return;finished=true;clearTimeout(timeout);socket.close();error?reject(error):resolve()}
    socket.onerror=()=>finish(new Error('WebSocket failed'));
    socket.onclose=()=>{if(packets.length<60)finish(new Error(`closed after ${packets.length} frames`))};
    socket.onmessage=event=>{
      try {
        if(typeof event.data==='string'){
          const state=JSON.parse(event.data);assert.notEqual(state.kind,'error',state.message);states.push(state);return;
        }
        const bytes=Buffer.from(event.data);assert.ok(bytes.length>9);assert.ok(states.length);
        const key=bytes[0]===1,timestamp=Number(bytes.readBigUInt64LE(1));
        if(!packets.length)assert.ok(key,'stream starts with an IDR frame');
        if(requested&&key)keyAfterRequest=true;
        packets.push({key,timestamp,size:bytes.length-9,arrived:performance.now()-started});chunks.push(bytes.subarray(9));
        if(packets.length===30){requested=true;socket.send('keyframe')}
        if(packets.length===60)finish();
      } catch(error){finish(error)}
    };
  });
  assert.ok(keyAfterRequest,'decoder recovery gets a new keyframe');
  assert.ok(packets.every((packet,i)=>!i||packet.timestamp>=packets[i-1].timestamp));
  const state=states.at(-1);assert.ok(state.width>1066&&state.height>600,'full resolution video');
  const result={status:'passed',frames:packets.length,fps:Number(((packets.length-1)*1000/(packets.at(-1).arrived-packets[0].arrived)).toFixed(2)),width:state.width,height:state.height,codec:state.codec,keyframeRecovery:keyAfterRequest,crossOriginRejected:true,inputSent:false,packets};
  fs.mkdirSync(output,{recursive:true});fs.writeFileSync(path.join(output,'stream.h264'),Buffer.concat(chunks));fs.writeFileSync(path.join(output,'stream.json'),JSON.stringify(result,null,2));
  console.log(JSON.stringify({...result,packets:undefined}));
})().catch(error=>{console.error(error);process.exitCode=1});
