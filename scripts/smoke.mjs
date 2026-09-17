import { execFile, spawn } from 'node:child_process';
import { access, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { constants } from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { promisify } from 'node:util';
import { assessSmoke, clipClearsStage, evidenceRegion, installSmokeMeasurements, smokeLimits, smokeStatusText, smokeViewports, transitionFrameSampler } from '../test/smoke-measurements.js';

const execute = promisify(execFile);
import { serveDevelopment } from './dev.mjs';

const development = process.argv.includes('--dev');
const mode = (development || process.argv.includes('--private')) ? 'private' : 'public';
const live = development;
if (process.argv.includes('--live') && !development) throw new Error('Live smoke requires --dev');
const deployed = process.argv.includes('--deployed');
if (development && deployed) throw new Error('--dev and --deployed are exclusive');
const neutralSilhouette = mode === 'public';
const transitions = mode === 'private' && !live
  ? ["__smokeRuntime.pose('stand')", "__smokeRuntime.pose('sit')"]
  : ["document.querySelector('#puppet').click()", "document.querySelector('#puppet').click()"];
const outputFlag = process.argv.indexOf('--output');
const output = path.resolve(outputFlag >= 0 ? process.argv[outputFlag + 1] : 'smoke-artifacts');
const durationFlag = process.argv.indexOf('--duration');
const duration = Number(durationFlag >= 0 ? process.argv[durationFlag + 1] : 30);
const viewportFlag = process.argv.indexOf('--viewport');
const selectedViewports = viewportFlag >= 0 ? smokeViewports.filter((viewport) => viewport.name === process.argv[viewportFlag + 1]) : smokeViewports;
const baselineFlag = process.argv.indexOf('--baseline');
const baseline = baselineFlag >= 0 ? JSON.parse(await readFile(process.argv[baselineFlag + 1], 'utf8')) : {};
const root = path.resolve(new URL('..', import.meta.url).pathname);
const wasmRoot = process.env.VOICE_WASM_DIR ? path.resolve(process.env.VOICE_WASM_DIR) : null;
await mkdir(output, { recursive: true });

async function chromiumExecutable() {
  if (process.env.CHROMIUM_BIN) return process.env.CHROMIUM_BIN;
  const candidates = ['chromium', 'chromium-browser', 'google-chrome'];
  try { candidates.push(...(await readdir('/nix/store')).filter((name) => name.includes('-chromium-')).map((name) => `/nix/store/${name}/bin/chromium`)); } catch {}
  for (const candidate of candidates) for (const location of candidate.includes('/') ? [candidate] : (process.env.PATH ?? '').split(':').map((directory) => path.join(directory, candidate))) try { await access(location, constants.X_OK); return location; } catch {}
  throw new Error('headless Chromium is required; set CHROMIUM_BIN');
}

const mockWasm = `
const enc = new TextEncoder(); const dec = new TextDecoder(); const queue = [enc.encode('{"ok":true}')]; const waiting = [];
const push = (value) => { const resolve = waiting.shift(); if (resolve) resolve(value); else queue.push(value); };
export default async function wbgInit() {} export async function init() {} export async function connect() {}
export async function recv() { if (queue.length) return queue.shift(); return new Promise((resolve) => waiting.push(resolve)); }
export async function send_only(bytes) {
  const value = dec.decode(bytes);
  if (value === 'puppets') push(enc.encode('puppets\\n'+JSON.stringify({active:'neutral',avatars:[{id:'neutral',size:1,contentHash:'neutral',creditLine:'',licenseFlags:{creditRequired:false}}]})));
  else if (value === 'clips') push(enc.encode('clips\\n'+JSON.stringify({clips:[{action:'idle',name:'idle.fbx',format:'fbx',contentHash:'idle'},{action:'sit',name:'sit.fbx',format:'fbx',contentHash:'sit'}]})));
  else if (value.startsWith('puppet\\n')) { const zipped = new Uint8Array(await new Response(new Blob([new Uint8Array([1])]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer()); const id='neutral'; push(enc.encode('puppet-start\\n'+JSON.stringify({id,size:zipped.length,originalSize:1,contentHash:'neutral',encoding:'gzip'}))); const prefix=enc.encode('puppet-chunk\\n'+id+'\\n'); const chunk=new Uint8Array(prefix.length+zipped.length); chunk.set(prefix); chunk.set(zipped,prefix.length); push(chunk); push(enc.encode('puppet-end\\n'+id)); }
  else if (value.startsWith('track\\n')) { const request=JSON.parse(value.slice(value.indexOf('\\n')+1)); const zipped=new Uint8Array(await new Response(new Blob([new Uint8Array([1])]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer()); push(enc.encode('track-start\\n'+JSON.stringify({id:request.id,size:zipped.length,originalSize:1,contentHash:request.modelHash+'-'+request.clipHash,encoding:'gzip'}))); const prefix=enc.encode('track-chunk\\n'+request.id+'\\n'); const chunk=new Uint8Array(prefix.length+zipped.length); chunk.set(prefix); chunk.set(zipped,prefix.length); push(chunk); push(enc.encode('track-end\\n'+request.id)); }
  else if (value.startsWith('puppet-select\\n')) { const request=JSON.parse(value.slice(value.indexOf('\\n')+1)); push(enc.encode('puppet-selected\\n'+JSON.stringify({id:request.id}))); }
  else if (value.startsWith('offer\\n')) { const offer=JSON.parse(value.slice(6)); push(enc.encode('answer\\n'+JSON.stringify({offer_id:offer.id,sdp:'answer',cap_seconds:60}))); }
  else if (value.startsWith('delegate\\n')) { const request=JSON.parse(value.slice(value.indexOf('\\n')+1)); push(enc.encode('hub\\n'+JSON.stringify({id:request.id,reply:'The fixture is complete.',timing_ms:42,stamp:'fixture'}))); }
  else if (value.startsWith('telemetry\\n')) { const batch=JSON.parse(value.slice(value.indexOf('\\n')+1)); push(enc.encode('telemetry-ack\\n'+JSON.stringify({batch_id:batch.batch_id}))); }
}`;

const neutralSilhouettePuppet = `
export class PuppetRuntime {
  constructor(canvas) { this.canvas=canvas; this.poseName='sit'; globalThis.__smokeRuntime=this; }
  async load(bytes, clip, valid, beforeCommit) { await beforeCommit(); this.draw(false); return valid(); }
  async loadClips() {} start() {} pause() {} clear() {} dispose() {} async attachAudio() {} async detachAudio() {}
  draw(standing) { const c=this.canvas, width=Math.max(1,c.clientWidth), height=Math.max(1,c.clientHeight); if(c.width!==width)c.width=width;if(c.height!==height)c.height=height;const x=c.getContext('2d'); x.clearRect(0,0,c.width,c.height); x.fillStyle='#b9bdc7'; const cx=c.width/2, head=c.height*.2; x.beginPath(); x.arc(cx,head,c.height*.055,0,Math.PI*2); x.fill(); x.lineWidth=Math.max(8,c.width*.025); x.strokeStyle='#b9bdc7'; x.beginPath(); x.moveTo(cx,head+c.height*.06); x.lineTo(cx,standing?c.height*.58:c.height*.52); x.moveTo(cx,head+c.height*.15); x.lineTo(cx-c.width*.1,c.height*.42); x.moveTo(cx,head+c.height*.15); x.lineTo(cx+c.width*.1,c.height*.42); x.moveTo(cx,standing?c.height*.58:c.height*.52); x.lineTo(cx-c.width*.07,standing?c.height*.82:c.height*.65); x.moveTo(cx,standing?c.height*.58:c.height*.52); x.lineTo(cx+c.width*.07,standing?c.height*.82:c.height*.65); x.stroke(); }
  pose(name) { this.poseName=name; this.draw(name!=='sit'); } gesture() { return true; } look() {} mood() {} waiting() {} listening() {} speak() {}
}`;

const browserMocks = `
Object.defineProperty(navigator,'mediaDevices',{value:{getUserMedia:async()=>({getTracks:()=>[{stop(){}}]})}});
class Recorder extends EventTarget { static isTypeSupported(){return true} start(){this.state='recording'} stop(){this.state='inactive';this.dispatchEvent(new Event('stop'))} } globalThis.MediaRecorder=Recorder;
class Channel extends EventTarget { constructor(){super();this.readyState='open'} send(value){const e=JSON.parse(value);if(e.type==='session.close')queueMicrotask(()=>this.dispatchEvent(new MessageEvent('message',{data:JSON.stringify({type:'session.closed',usage:{seconds:0}})})))} close(){this.readyState='closed'} }
class Peer { constructor(){this.iceGatheringState='complete';this.localDescription={sdp:'offer'}} createDataChannel(){this.channel=new Channel();globalThis.__smokeChannel=this.channel;return this.channel}async createOffer(){return {type:'offer',sdp:'offer'}}async setLocalDescription(v){this.localDescription=v}async setRemoteDescription(){queueMicrotask(()=>this.channel.dispatchEvent(new MessageEvent('message',{data:JSON.stringify({type:'session.started'})})))}addTrack(){}close(){} } globalThis.RTCPeerConnection=Peer;
globalThis.__voiceLoadEmbedder=async()=>async(texts)=>texts.map(()=>[1]);
const cache=new Map();Object.defineProperty(globalThis,'caches',{value:{open:async()=>({match:async(r)=>cache.get(r.url)?.clone(),put:async(r,v)=>cache.set(r.url,v.clone())})}});
`;

async function makeServer() {
  if (development) return serveDevelopment({ port: 0, wasmRoot });
  let index = await readFile(path.join(root, 'docs/index.html'), 'utf8');
  if (wasmRoot) index = index.replace('https://bddap-bot.github.io/botq/botq_dash_wasm.js', '/botq_dash_wasm.js');
  let token;
  if (mode === 'public') {
    index = index.replace('https://bddap-bot.github.io/botq/botq_dash_wasm.js', '/smoke-wasm.js').replace('./puppet.js', '/smoke-puppet.js').replace('</head>', `<script>${browserMocks}</script></head>`);
    token = Buffer.from(JSON.stringify({ endpoint_id: 'smoke', secret: 'smoke' })).toString('base64url');
  } else {
    const { stdout } = await execute('voice-web', ['token']);
    token = stdout.trim();
    index = index.replace('puppetRuntime = new PuppetRuntime($(\'puppet\'), $(\'display\'));', "puppetRuntime = new PuppetRuntime($('puppet'), $('display')); globalThis.__smokeRuntime = puppetRuntime;");
  }
  if (deployed) return { url: 'https://bddap-bot.github.io/voice/', token, close() {} };
  index = index.replace('</head>', `<script>localStorage.setItem('voice.token', ${JSON.stringify(token)});</script></head>`);
  const server = http.createServer(async (request, response) => {
    const url = new URL(request.url, 'http://localhost');
    if (wasmRoot && (url.pathname === '/botq_dash_wasm.js' || url.pathname === '/botq_dash_wasm_bg.wasm')) {
      const body = await readFile(path.join(wasmRoot, url.pathname.slice(1)));
      return response.writeHead(200, { 'content-type': url.pathname.endsWith('.wasm') ? 'application/wasm' : 'text/javascript' }).end(body);
    }
    if (url.pathname === '/smoke-wasm.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(mockWasm);
    if (url.pathname === '/smoke-puppet.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(neutralSilhouettePuppet);
    const relative = url.pathname === '/' ? 'index.html' : url.pathname.slice(1);
    try { const body = relative === 'index.html' ? index : await readFile(path.join(root, 'docs', relative)); response.writeHead(200, { 'content-type': relative.endsWith('.js') ? 'text/javascript' : relative.endsWith('.html') ? 'text/html' : 'application/octet-stream' }).end(body); } catch { response.writeHead(404).end(); }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { server, url: `http://127.0.0.1:${server.address().port}/`, token, close: () => server.close() };
}

async function connectCdp(port) {
  let targets;
  for (let index = 0; index < 300; index++) { try { targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json(); break; } catch { await new Promise((resolve) => setTimeout(resolve, 100)); } }
  if (!targets) throw new Error('Chromium DevTools did not become ready within 30 seconds');
  const socket = new WebSocket(targets.find((target) => target.type === 'page').webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let id = 0;
  const pending = new Map();
  const consoleErrors = [];
  socket.onmessage = ({ data }) => { const message=JSON.parse(data);if(pending.has(message.id)){pending.get(message.id)(message);pending.delete(message.id)}else if(message.method==='Runtime.exceptionThrown')consoleErrors.push(message.params.exceptionDetails.exception?.description??message.params.exceptionDetails.text);else if(message.method==='Runtime.consoleAPICalled'&&message.params.type==='error')consoleErrors.push(message.params.args.map((value)=>value.value??value.description).join(' ')); };
  const call = (method, params={}) => new Promise((resolve) => { const next=++id;pending.set(next,resolve);socket.send(JSON.stringify({id:next,method,params})); });
  const evaluate = async (expression) => { const message=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(message.result.exceptionDetails)throw new Error(message.result.exceptionDetails.exception?.description??message.result.exceptionDetails.text);return message.result.result.value; };
  return { call, evaluate, consoleErrors, close: () => socket.close() };
}

async function runViewport(viewport, executable, server) {
  const scratch = await mkdtemp(path.join(root, '.smoke-'));
  const devPort = await new Promise((resolve) => { const listener=net.createServer().listen(0,'127.0.0.1',()=>{const value=listener.address().port;listener.close(()=>resolve(value))}); });
  const args=['--headless=new','--no-sandbox','--disable-background-timer-throttling','--disable-renderer-backgrounding','--hide-scrollbars','--use-fake-device-for-media-stream','--use-fake-ui-for-media-stream',`--window-size=${viewport.width},${viewport.height}`,`--user-data-dir=${path.join(scratch,'profile')}`,`--remote-debugging-port=${devPort}`,'--remote-debugging-address=127.0.0.1',...(viewport.mobile?['--user-agent=Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/140.0 Mobile Safari/537.36']:[]),'about:blank'];
  const chrome=spawn(executable,args,{stdio:['ignore','ignore','pipe']});
  let chromeError='';chrome.stderr.on('data',(chunk)=>{chromeError+=chunk});
  const cdp=await connectCdp(devPort);
  try {
    if (development) await cdp.call('Page.addScriptToEvaluateOnNewDocument', { source: `
      globalThis.__smokeLiveChannels = [];
      const Peer = RTCPeerConnection;
      globalThis.RTCPeerConnection = class extends Peer {
        createDataChannel(...args) {
          const channel = super.createDataChannel(...args);
          const events = []; globalThis.__smokeLiveChannels.push(events);
          channel.addEventListener('message', ({ data }) => { const event = JSON.parse(data); if (['session.started', 'session.closed'].includes(event.type)) events.push(event.type); });
          return channel;
        }
      };
    ` });
    await cdp.call('Page.enable');await cdp.call('Runtime.enable');await cdp.call('Page.addScriptToEvaluateOnNewDocument',{source:`localStorage.setItem('voice.token', ${JSON.stringify(server.token)})`});await cdp.call('Emulation.setDeviceMetricsOverride',{width:viewport.width,height:viewport.height,deviceScaleFactor:viewport.scale,mobile:viewport.mobile});await cdp.call('Page.navigate',{url:server.url});
    const deadline=Date.now()+180000;
    let ready=false;
    while(Date.now()<deadline){try{const page=JSON.parse(await cdp.evaluate("JSON.stringify({ready:document.querySelector('#puppet')?.getAttribute('aria-disabled')==='false',status:document.querySelector('#status')?.textContent,error:document.querySelector('#status')?.classList.contains('err')})"));ready=page.ready;if(ready)break;if(page.error)throw new Error(`page connection failed: ${page.status}`)}catch(error){if(error.message?.startsWith('page connection failed:'))throw error}await new Promise((resolve)=>setTimeout(resolve,250));}
    if(!ready){const probe=await cdp.evaluate(`JSON.stringify({status:document.querySelector('#status')?.textContent,disabled:document.querySelector('#puppet')?.getAttribute('aria-disabled'),body:document.body?.innerText?.slice(0,500)})`);throw new Error(`page did not become ready: ${probe} ${cdp.consoleErrors.join('; ')} ${chromeError.slice(-500)}`)}
    await cdp.evaluate(`const smokeLimits = ${JSON.stringify(smokeLimits)}; const transitionFrameSampler = ${transitionFrameSampler.toString()}; globalThis.__smoke = (${installSmokeMeasurements.toString()})()`);
    const stageGeometry = async () => JSON.parse(await cdp.evaluate("JSON.stringify((() => { const box = document.querySelector('#puppet').getBoundingClientRect(); return { canvas: { left: box.left + scrollX, right: box.right + scrollX, top: box.top + scrollY, bottom: box.bottom + scrollY }, viewport: { x: scrollX, y: scrollY, width: innerWidth, height: innerHeight } }; })())"));
    const region = evidenceRegion({ neutralSilhouette, ...(await stageGeometry()) });
    const frames=[];
    let measuringTransition=false;
    let liveSessionOpened = false;
    for(let second=0;second<duration;second++){
      if(second===1){measuringTransition=true;await cdp.evaluate(`__smoke.startTransition();${transitions[0]}`);if(mode==='public')await cdp.evaluate(`new Promise(async(resolve)=>{while(!globalThis.__smokeChannel)await new Promise(done=>setTimeout(done,10));const emit=(event)=>__smokeChannel.dispatchEvent(new MessageEvent('message',{data:JSON.stringify(event)}));emit({type:'session.output_transcript.delta',delta:'I will inspect the fixture.',start_ms:0,end_ms:20});emit({type:'session.input_transcript.delta',delta:'Check the fixture stream.'});emit({type:'session.delegation.created',delegation:{id:'fixture'}});setTimeout(resolve,150)})`)}
      if (development && second === 2) {
        await cdp.evaluate(`new Promise((resolve, reject) => { const deadline = Date.now() + 60000; const check = () => { if (document.querySelector('#puppet').getAttribute('aria-pressed') === 'true') return resolve(); if (Date.now() > deadline || document.querySelector('#status').classList.contains('err')) return reject(new Error(document.querySelector('#status').textContent)); setTimeout(check, 100); }; check(); })`);
        liveSessionOpened = true;
      }
      if(second===3)await cdp.evaluate(`document.querySelector('#status').textContent=${JSON.stringify(smokeStatusText)}`);
      if(second===6){measuringTransition=false;await cdp.evaluate(`__smoke.stopTransition()`)}
      if(second===Math.max(8,duration-6)){measuringTransition=true;await cdp.evaluate(`__smoke.startTransition();${transitions[1]}`)}
      if(second===duration-1){measuringTransition=false;await cdp.evaluate(`__smoke.stopTransition()`)}
      await cdp.evaluate('__smoke.sample()');
      if(measuringTransition)await cdp.evaluate('__smoke.stopTransition()');
      if(!neutralSilhouette&&!clipClearsStage(region,(await stageGeometry()).canvas))throw new Error(`the stage canvas reached the evidence crop at second ${second}; this run did not load the neutral silhouette`);
      const shot=await cdp.call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false,clip:{...region,scale:viewport.scale}});
      const file=path.join(output,`${viewport.name}-${String(second).padStart(3,'0')}.png`);await writeFile(file,Buffer.from(shot.result.data,'base64'));frames.push(file);
      if(measuringTransition)await cdp.evaluate('new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))).then(() => __smoke.startTransition())');
      await new Promise((resolve)=>setTimeout(resolve,1000));
    }
    if (development) {
      if (!liveSessionOpened) throw new Error('development smoke did not open a Live session');
      await cdp.evaluate(`new Promise((resolve, reject) => { const deadline = Date.now() + 30000; const check = () => { if (document.querySelector('#mic-mute').disabled && document.querySelector('#puppet').getAttribute('aria-pressed') === 'false' && document.querySelector('#puppet').getAttribute('aria-disabled') === 'false') return resolve(); if (Date.now() > deadline) return reject(new Error('Live session did not close')); setTimeout(check, 100); }; check(); })`);
      const channels = await cdp.evaluate('__smokeLiveChannels');
      if (!channels.some(events => events.includes('session.started') && events.includes('session.closed'))) throw new Error('Live did not acknowledge start and close on the same channel');
      const sessions = { channels, endpoint: JSON.parse(Buffer.from(server.token, 'base64url')).endpoint_id, liveSessionOpened, liveSessionClosed: true };
      await writeFile(path.join(output, `${viewport.name}-connection.json`), JSON.stringify(sessions, null, 2));
    }
    const state=JSON.parse(await cdp.evaluate('JSON.stringify(__smoke.state)'));
    state.errors.push(...cdp.consoleErrors);
    const report={viewport:viewport.name,...assessSmoke(state),state};
    await writeFile(path.join(output,`${viewport.name}.json`),JSON.stringify(report,null,2));
    await execute('ffmpeg',['-y','-framerate','2','-i',path.join(output,`${viewport.name}-%03d.png`),'-vf','scale=iw/2:ih/2:flags=lanczos','-t','8',path.join(output,`${viewport.name}.gif`)],{timeout:120000,maxBuffer:1024*1024*10});
    return report;
  } finally {
    cdp.close();
    if (chrome.exitCode === null) {
      const exited = new Promise((resolve) => chrome.once('exit', resolve));
      chrome.kill('SIGKILL');
      await exited;
    }
    for (let attempt = 0; attempt < 5; attempt++) {
      try { await rm(scratch,{recursive:true,force:true,maxRetries:3,retryDelay:100}); break; }
      catch (error) { if (attempt === 4) throw error; await new Promise((resolve) => setTimeout(resolve, 250)); }
    }
  }
}

const server=await makeServer();
const executable=await chromiumExecutable();
const reports=[];
try { for(const viewport of selectedViewports) reports.push(await runViewport(viewport,executable,server)); } finally { await server.close(); }
const unexpected = reports.flatMap((report) => report.failures.filter((failure) => !(baseline[report.viewport] ?? []).includes(failure)).map((failure) => `${report.viewport}:${failure}`));
await writeFile(path.join(output,'report.json'),JSON.stringify({mode,createdAt:new Date().toISOString(),reports,unexpected},null,2));
console.log('| viewport | result | failures | CLS | canvas drift | max frame gap |');
console.log('|---|---|---|---:|---:|---:|');
for(const report of reports)console.log(`| ${report.viewport} | ${report.pass?'PASS':'FAIL'} | ${report.failures.join(', ')||'none'} | ${report.metrics.cumulativeLayoutShift.toFixed(3)} | ${report.metrics.canvasHeightDrift} | ${report.metrics.maximumFrameGapMs} |`);
process.exitCode=unexpected.length?1:0;
