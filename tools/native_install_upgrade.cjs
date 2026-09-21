'use strict';
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const embeddedHashes='__DSH_NATIVE_EXPECTED_HASHES__';
const files={'dsh-windows-native.exe':'sha256','dsh-command-runner.exe':'commandRunnerSha256','dsh-windows-sandbox-setup.exe':'setupSha256'};
function readJson(file){try{const data=fs.readFileSync(file);if(data.length>131072)throw Error('Configuration exceeds size limit');return JSON.parse(data.toString('utf8').replace(/^\uFEFF/,''))}catch(e){if(e.code==='ENOENT')return null;throw e}}
function identity(file){const value=fs.realpathSync(file);return process.platform==='win32'?value.toLowerCase():value}
function migrate(installRoot,home,expected){
  const native=path.join(installRoot,'native-sandbox'),hashes={};
  for(const [name,field] of Object.entries(files)){
    const hash=crypto.createHash('sha256').update(fs.readFileSync(path.join(native,name))).digest('hex');
    if(hash!==expected[name])throw Error('Installed native helper does not match this installer: '+name);
    hashes[field]=hash;
  }
  const roots=new Set([path.resolve(home)]);let current=path.resolve(home);
  for(let depth=0;depth<16;depth++){
    const marker=readJson(path.join(current,'.dsh-home-redirect.json'));
    if(!marker?.target)break;
    if(!path.isAbsolute(marker.target)||roots.has(path.resolve(marker.target)))throw Error('Invalid home redirect');
    current=path.resolve(marker.target);roots.add(current);
    if(depth===15)throw Error('Home redirect limit exceeded');
  }
  for(const root of [...roots]){
    const runtime=readJson(path.join(root,'.runtime-paths.json'));
    if(runtime?.cacheDirectory&&path.isAbsolute(runtime.cacheDirectory))roots.add(path.dirname(runtime.cacheDirectory));
    if(runtime?.dataDirectory&&path.isAbsolute(runtime.dataDirectory))roots.add(path.resolve(runtime.dataDirectory));
  }
  const changed=[];
  for(const root of roots){
    const file=path.join(root,'windows-sandbox.json'),config=readJson(file);
    if(!config)continue;
    if(config.version!==1||config.backend!=='windows-native'||typeof config.runner!=='string'||!path.isAbsolute(config.runner))throw Error('Invalid native selection: '+file);
    if(typeof config.stateDirectory!=='string'||!path.isAbsolute(config.stateDirectory)||
       (config.workspaces!==undefined&&(!Array.isArray(config.workspaces)||config.workspaces.some(p=>typeof p!=='string'||!path.isAbsolute(p))))||
       Object.values(files).some(key=>typeof config[key]!=='string'||!/^[a-fA-F0-9]{64}$/.test(config[key])))throw Error('Incomplete native selection: '+file);
    let selected;
    try{selected=identity(config.runner)}catch(error){if(error.code==='ENOENT')continue;throw error}
    if(selected!==identity(path.join(native,'dsh-windows-native.exe')))continue;
    if(Object.entries(hashes).every(([key,value])=>config[key]===value))continue;
    const suffix=crypto.randomUUID(),backup=file+'.before-upgrade-'+suffix;
    fs.copyFileSync(file,backup,fs.constants.COPYFILE_EXCL);
    const temporary=file+'.'+suffix+'.tmp';
    const handle=fs.openSync(temporary,'wx',0o600);
    try{fs.writeFileSync(handle,JSON.stringify({...config,...hashes},null,2)+'\n');fs.fsyncSync(handle)}finally{fs.closeSync(handle)}
    fs.renameSync(temporary,file);changed.push({file,backup});
  }
  return {migrated:changed.length,changed};
}
module.exports={migrate};
if(require.main===module){
  try{
    const home=process.env.DSH_HOME||(process.env.LOCALAPPDATA&&path.join(process.env.LOCALAPPDATA,'DeepSeek Harness'));
    if(!home||!path.isAbsolute(home))throw Error('User data directory unavailable');
    console.log(JSON.stringify(migrate(path.resolve(__dirname,'..'),home,JSON.parse(embeddedHashes))));
  }catch(error){console.error('Native sandbox upgrade failed: '+error.message);process.exitCode=1}
}
