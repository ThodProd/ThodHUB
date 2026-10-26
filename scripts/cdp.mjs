// Small native WebView2 QA driver; Node's built-in WebSocket, no browser dependency.
import fs from 'node:fs/promises';
const targets=await fetch('http://127.0.0.1:9225/json/list').then(r=>r.json());
const target=targets.find(t=>t.type==='page'&&t.url.startsWith('https://thodhub.local/'));
if(!target)throw new Error('ThodHUB WebView2 target not found');
const ws=new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve,reject)=>{ws.addEventListener('open',resolve,{once:true});ws.addEventListener('error',reject,{once:true});});
let id=0;const pending=new Map();
ws.addEventListener('message',e=>{const m=JSON.parse(e.data);const p=pending.get(m.id);if(p){pending.delete(m.id);m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result);}});
function send(method,params={}){return new Promise((resolve,reject)=>{const key=++id;pending.set(key,{resolve,reject});ws.send(JSON.stringify({id:key,method,params}));});}
const action=process.argv[2];
try{
  if(action==='eval'){
    const result=await send('Runtime.evaluate',{expression:process.argv[3],awaitPromise:true,returnByValue:true});
    if(result.exceptionDetails)throw new Error(JSON.stringify(result.exceptionDetails));
    console.log(JSON.stringify(result.result.value??result.result,null,2));
  }else if(action==='shot'){
    if(process.argv[4])await send('Emulation.setDeviceMetricsOverride',{width:Number(process.argv[4]),height:Number(process.argv[5]),deviceScaleFactor:1,mobile:false});
    await send('Runtime.evaluate',{expression:'new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))',awaitPromise:true});
    const shot=await send('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});
    await fs.writeFile(process.argv[3],Buffer.from(shot.data,'base64'));console.log(process.argv[3]);
  }else if(action==='key'){
    await send('Input.dispatchKeyEvent',{type:'keyDown',key:process.argv[3],code:process.argv[3]});
    await send('Input.dispatchKeyEvent',{type:'keyUp',key:process.argv[3],code:process.argv[3]});
  }else throw new Error('Expected eval, shot or key');
}finally{ws.close();}
