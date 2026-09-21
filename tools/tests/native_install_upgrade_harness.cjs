'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path'),crypto=require('node:crypto');
const {migrate}=require('../native_install_upgrade.cjs');
const root=fs.mkdtempSync(path.join(os.tmpdir(),'dsh-native-upgrade-'));
const app=path.join(root,'app'),home=path.join(root,'home'),native=path.join(app,'native-sandbox');
fs.mkdirSync(native,{recursive:true});fs.mkdirSync(home);
const expected={};
for(const name of ['dsh-windows-native.exe','dsh-command-runner.exe','dsh-windows-sandbox-setup.exe']){
  const data=Buffer.from('verified package '+name);fs.writeFileSync(path.join(native,name),data);expected[name]=crypto.createHash('sha256').update(data).digest('hex');
}
const file=path.join(home,'windows-sandbox.json');
const original={version:1,backend:'windows-native',runner:path.join(native,'dsh-windows-native.exe'),stateDirectory:path.join(root,'private-state'),workspaces:[path.join(root,'selected')],sha256:'a'.repeat(64),commandRunnerSha256:'b'.repeat(64),setupSha256:'c'.repeat(64)};
fs.writeFileSync(file,JSON.stringify(original));
const result=migrate(app,home,expected),updated=JSON.parse(fs.readFileSync(file,'utf8'));
assert.equal(result.migrated,1);assert.equal(updated.sha256,expected['dsh-windows-native.exe']);
assert.deepEqual(updated.workspaces,original.workspaces);assert.equal(updated.stateDirectory,original.stateDirectory);
assert.deepEqual(JSON.parse(fs.readFileSync(result.changed[0].backup,'utf8')),original);
assert.equal(migrate(app,home,expected).migrated,0);
fs.writeFileSync(file,JSON.stringify({...original,sha256:''}));
assert.throws(()=>migrate(app,home,expected),/Incomplete native selection/);
assert.equal(JSON.parse(fs.readFileSync(file,'utf8')).sha256,'');
fs.writeFileSync(file,JSON.stringify({...original,runner:path.join(root,'foreign','missing.exe')}));
assert.equal(migrate(app,home,expected).migrated,0);
const before=fs.readFileSync(file);fs.appendFileSync(path.join(native,'dsh-windows-native.exe'),'tampered');
assert.throws(()=>migrate(app,home,expected),/does not match/);assert.deepEqual(fs.readFileSync(file),before);
console.log('PASS native upgrade: verified payload, old identity backup, scope preservation, idempotency and tamper rejection');
assert.equal(fs.realpathSync(path.dirname(root)),fs.realpathSync(os.tmpdir()));
fs.rmSync(root,{recursive:true});
