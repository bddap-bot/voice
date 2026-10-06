import { execFile } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { chromiumExecutable, launchChromium } from './chromium.mjs';
import http from 'node:http';
import net from 'node:net';
import path from 'node:path';
import { promisify } from 'node:util';
import { assessSmoke, clipClearsStage, evidenceRegion, installSmokeMeasurements, smokeLimits, smokeStatusText, smokeViewports } from '../test/smoke-measurements.js';

const execute = promisify(execFile);
import { serveDevelopment } from './dev.mjs';
import { NAME, SIGN_OFF, WAKE_PHRASE } from '../docs/identity.js';
import { includesPhrase } from '../docs/live.js';
import { RATE } from '../docs/wake.js';

const development = process.argv.includes('--dev');
const mode = (development || process.argv.includes('--private')) ? 'private' : 'public';
const live = development;
if (process.argv.includes('--live') && !development) throw new Error('Live smoke requires --dev');
const deployed = process.argv.includes('--deployed');
if (development && deployed) throw new Error('--dev and --deployed are exclusive');
const neutralSilhouette = mode === 'public';
const transitions = mode === 'private' && !live
  ? ["__smokeRuntime.pose('stand')", "__smokeRuntime.pose('sit')"]
  : live ? ['__smokeSay(__smokeSpeech.wake)', '__smokeSay(__smokeSpeech.farewell)']
  : ["document.querySelector('#puppet').click()", "document.querySelector('#puppet').click()"];
async function developmentSpeech() {
  const { HELD_OUT_SENTENCES, heldOutSpeech } = await import('./wake.mjs');
  const [wake, request, farewell, ...ordinary] = await heldOutSpeech([WAKE_PHRASE, 'Ask the hub for the current test beacon status.', `${NAME}, go to sleep.`, ...HELD_OUT_SENTENCES.slice(0, 8)]);
  return { wake, request, farewell, ordinary };
}
const speech = live ? await developmentSpeech() : null;
const outputFlag = process.argv.indexOf('--output');
const output = path.resolve(outputFlag >= 0 ? process.argv[outputFlag + 1] : 'smoke-artifacts');
const durationFlag = process.argv.indexOf('--duration');
const duration = Number(durationFlag >= 0 ? process.argv[durationFlag + 1] : 30);
const viewportFlag = process.argv.indexOf('--viewport');
const selectedViewports = viewportFlag >= 0 ? smokeViewports.filter((viewport) => viewport.name === process.argv[viewportFlag + 1]) : smokeViewports;
const baselineFlag = process.argv.indexOf('--baseline');
const baseline = baselineFlag >= 0 ? JSON.parse(await readFile(process.argv[baselineFlag + 1], 'utf8')) : {};
const root = path.resolve(new URL('..', import.meta.url).pathname);
await mkdir(output, { recursive: true });

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
  else if (value.startsWith('motion\\n')) { const request=JSON.parse(value.slice(value.indexOf('\\n')+1)); const motion=enc.encode(JSON.stringify({name:request.id,duration:1,hipsHeight:1,tracks:[{name:'hips.quaternion',times:[0],values:[0,0,0,1]}]})); const zipped=new Uint8Array(await new Response(new Blob([motion]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer()); push(enc.encode('motion-start\\n'+JSON.stringify({id:request.id,size:zipped.length,originalSize:motion.length,contentHash:request.clipHash,encoding:'gzip'}))); const prefix=enc.encode('motion-chunk\\n'+request.id+'\\n'); const chunk=new Uint8Array(prefix.length+zipped.length); chunk.set(prefix); chunk.set(zipped,prefix.length); push(chunk); push(enc.encode('motion-end\\n'+request.id)); }
  else if (value.startsWith('puppet-select\\n')) { const request=JSON.parse(value.slice(value.indexOf('\\n')+1)); push(enc.encode('puppet-selected\\n'+JSON.stringify({id:request.id}))); }
  else if (value === 'wake-model') push(enc.encode('wake-model-none'));
  else if (value.startsWith('offer\\n')) { const offer=JSON.parse(value.slice(6)); push(enc.encode('answer\\n'+JSON.stringify({offer_id:offer.id,sdp:'answer'}))); }
  else if (value.startsWith('delegate\\n')) { const request=JSON.parse(value.slice(value.indexOf('\\n')+1)); push(enc.encode('hub\\n'+JSON.stringify({id:request.id,commentary:['The fixture is complete.'],instructions:[],timing_ms:42,stamp:'fixture'}))); }
  else if (value.startsWith('telemetry\\n')) { const batch=JSON.parse(value.slice(value.indexOf('\\n')+1)); push(enc.encode('telemetry-ack\\n'+JSON.stringify({batch_id:batch.batch_id}))); }
}`;

const neutralSilhouettePuppet = `
export class PuppetRuntime {
  constructor(canvas) { this.canvas=canvas; this.poseName='sit'; globalThis.__smokeRuntime=this; }
  async load(bytes, valid, beforeCommit, stage) { await stage('select', beforeCommit); this.draw(false); return valid(); }
  async loadClips() {} start() {} pause() {} clear() {} dispose() {} async attachAudio() {} async detachAudio() {}
  draw(standing) { const c=this.canvas, width=Math.max(1,c.clientWidth), height=Math.max(1,c.clientHeight); if(c.width!==width)c.width=width;if(c.height!==height)c.height=height;const x=c.getContext('2d'); x.clearRect(0,0,c.width,c.height); x.fillStyle='#b9bdc7'; const cx=c.width/2, head=c.height*.2; x.beginPath(); x.arc(cx,head,c.height*.055,0,Math.PI*2); x.fill(); x.lineWidth=Math.max(8,c.width*.025); x.strokeStyle='#b9bdc7'; x.beginPath(); x.moveTo(cx,head+c.height*.06); x.lineTo(cx,standing?c.height*.58:c.height*.52); x.moveTo(cx,head+c.height*.15); x.lineTo(cx-c.width*.1,c.height*.42); x.moveTo(cx,head+c.height*.15); x.lineTo(cx+c.width*.1,c.height*.42); x.moveTo(cx,standing?c.height*.58:c.height*.52); x.lineTo(cx-c.width*.07,standing?c.height*.82:c.height*.65); x.moveTo(cx,standing?c.height*.58:c.height*.52); x.lineTo(cx+c.width*.07,standing?c.height*.82:c.height*.65); x.stroke(); }
  pose(name) { this.poseName=name; this.draw(name!=='sit'); } gesture() { return true; } mood() {} waiting() {} speak() {} asleep() {}
}`;

const browserMocks = `
Object.defineProperty(navigator,'mediaDevices',{value:Object.assign(new EventTarget(),{getUserMedia:async()=>new AudioContext().createMediaStreamDestination().stream})});
class Recorder extends EventTarget { static isTypeSupported(){return true} start(){this.state='recording'} stop(){this.state='inactive';this.dispatchEvent(new Event('stop'))} } globalThis.MediaRecorder=Recorder;
class Channel extends EventTarget { constructor(){super();this.readyState='open'} send(value){const e=JSON.parse(value);if(e.type==='session.close')queueMicrotask(()=>this.dispatchEvent(new MessageEvent('message',{data:JSON.stringify({type:'session.closed',usage:{seconds:0}})})))} close(){this.readyState='closed'} }
class Peer extends EventTarget { constructor(){super();this.connectionState='new';this.iceGatheringState='complete';this.localDescription={sdp:'offer'}} createDataChannel(){this.channel=new Channel();globalThis.__smokeChannel=this.channel;return this.channel}async createOffer(){return {type:'offer',sdp:'offer'}}async setLocalDescription(v){this.localDescription=v}async setRemoteDescription(){queueMicrotask(()=>this.channel.dispatchEvent(new MessageEvent('message',{data:JSON.stringify({type:'session.started',session:{model:'gpt-live-1',delegation:{type:'client'}}})})))}addTrack(){}async getStats(){return new Map()}close(){this.connectionState='closed';this.dispatchEvent(new Event('connectionstatechange'))} } globalThis.RTCPeerConnection=Peer;
globalThis.__voiceStartSpotter=async()=>({close(){}});
globalThis.__voiceLoadEmbedder=async()=>async(texts)=>texts.map(()=>[1]);
const cache=new Map();Object.defineProperty(globalThis,'caches',{value:{open:async()=>({match:async(r)=>cache.get(r.url)?.clone(),put:async(r,v)=>cache.set(r.url,v.clone())})}});
`;

async function makeServer() {
  if (development) return serveDevelopment({ port: 0 });
  let index = await readFile(path.join(root, 'docs/index.html'), 'utf8');
  let main = await readFile(path.join(root, 'docs/main.js'), 'utf8');
  let mocks = '';
  let token;
  if (mode === 'public') {
    main = main.replace('./relay/botq_dash_wasm.js', '/smoke-wasm.js').replace('./puppet.js', '/smoke-puppet.js');
    mocks = browserMocks;
    token = Buffer.from(JSON.stringify({ endpoint_id: 'smoke', secret: 'smoke' })).toString('base64url');
  } else {
    const { stdout } = await execute('voice-web', ['token']);
    token = stdout.trim();
    main = main.replace('puppetRuntime = new PuppetRuntime($(\'puppet\'));', "puppetRuntime = new PuppetRuntime($('puppet')); globalThis.__smokeRuntime = puppetRuntime;");
  }
  if (deployed) return { url: 'https://bddap-bot.github.io/voice/', token, close() {} };
  mocks += `localStorage.setItem('voice.token', ${JSON.stringify(token)});`;
  index = index.replace('</head>', '<script src="/smoke-mocks.js"></script></head>');
  const server = http.createServer(async (request, response) => {
    const url = new URL(request.url, 'http://localhost');
    if (url.pathname === '/smoke-wasm.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(mockWasm);
    if (url.pathname === '/smoke-puppet.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(neutralSilhouettePuppet);
    if (url.pathname === '/smoke-mocks.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(mocks);
    if (url.pathname === '/main.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(main);
    const relative = url.pathname === '/' ? 'index.html' : url.pathname.slice(1);
    try { const body = relative === 'index.html' ? index : await readFile(path.join(root, 'docs', relative)); response.writeHead(200, { 'content-type': relative.endsWith('.js') ? 'text/javascript' : relative.endsWith('.html') ? 'text/html' : relative.endsWith('.wasm') ? 'application/wasm' : 'application/octet-stream' }).end(body); } catch { response.writeHead(404).end(); }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { server, url: `http://127.0.0.1:${server.address().port}/`, token, close: () => server.close() };
}

async function connectCdp(port, exit) {
  const seconds = 120;
  const deadline = Date.now() + seconds * 1000;
  let page;
  while (!page && Date.now() < deadline) {
    try { page = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((target) => target.type === 'page'); } catch {}
    if (!page) await new Promise((resolve) => setTimeout(resolve, 100));
  }
  if (!page) throw new Error(`Chromium DevTools page target did not appear within ${seconds} seconds`);
  const browser = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
  const socket = new WebSocket(browser.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let id = 0;
  let pageSession;
  const pending = new Map();
  const consoleErrors = [];
  let workerSession;
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (pending.has(message.id)) {
      const { resolve, reject } = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) reject(new Error(message.error.message)); else resolve(message);
    } else if (message.method === 'Inspector.targetCrashed' && message.sessionId === pageSession) {
      exit(new Error('Chromium renderer crashed'));
    } else if (message.method === 'Target.attachedToTarget' && message.params.targetInfo.type === 'service_worker') {
      workerSession = message.params.sessionId;
    } else if (message.method === 'ServiceWorker.workerErrorReported') {
      consoleErrors.push('service-worker: ' + message.params.errorMessage.errorMessage);
    } else if (message.method === 'Runtime.exceptionThrown') {
      consoleErrors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
    } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
      consoleErrors.push(message.params.args.map((value) => value.value ?? value.description).join(' '));
    }
  };
  const call = (method, params = {}, sessionId = pageSession) => new Promise((resolve, reject) => {
    const next = ++id;
    pending.set(next, { resolve, reject });
    socket.send(JSON.stringify({ id: next, method, params, sessionId }));
  });
  const evaluate = async (expression) => { const message=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(message.result.exceptionDetails)throw new Error(message.result.exceptionDetails.exception?.description??message.result.exceptionDetails.text);return message.result.result.value; };
  pageSession = (await call('Target.attachToTarget', { targetId: page.id, flatten: true })).result.sessionId;
  const prepareWorker = async () => {
    if (!workerSession) throw new Error('service worker was not observed');
    // Delay cache lookup so response consumption wins the race if cloning moves back inside it.
    const result = await call('Runtime.evaluate', { expression: `if (!self.__smokeCacheDelayed) { self.__smokeCacheDelayed = true; const open = caches.open.bind(caches); caches.open = async (...args) => { const cache = await open(...args); await new Promise(resolve => setTimeout(resolve, 100)); return cache; }; }` }, workerSession);
    if (result.result.exceptionDetails) throw new Error('could not delay service-worker cache lookup: ' + JSON.stringify(result.result.exceptionDetails));
  };
  return { call, evaluate, consoleErrors, prepareWorker };
}

async function runViewport(viewport, executable, server) {
  const devPort = await new Promise((resolve) => { const listener=net.createServer().listen(0,'127.0.0.1',()=>{const value=listener.address().port;listener.close(()=>resolve(value))}); });
  const args=['--headless=new','--no-sandbox','--disable-background-timer-throttling','--disable-renderer-backgrounding','--hide-scrollbars','--use-fake-device-for-media-stream','--use-fake-ui-for-media-stream','--autoplay-policy=no-user-gesture-required',`--window-size=${viewport.width},${viewport.height}`,`--remote-debugging-port=${devPort}`,'--remote-debugging-address=127.0.0.1',...(viewport.mobile?['--user-agent=Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/140.0 Mobile Safari/537.36']:[]),'about:blank'];
  const chrome = await launchChromium({ executable, args, prefix: '.smoke-' });
  const { promise: exited, resolve: exit } = Promise.withResolvers();
  chrome.exited.then(exit);
  const run = async () => {
    const cdp=await connectCdp(devPort, exit);
    if (development) await cdp.call('Page.addScriptToEvaluateOnNewDocument', { source: `
      globalThis.__smokeLiveChannels = [];
      globalThis.__smokeSpeech = ${JSON.stringify(speech)};
      globalThis.__smokeHeard = [];
      const microphone = new AudioContext();
      const destination = microphone.createMediaStreamDestination();
      const floor = microphone.createConstantSource();
      floor.offset.value = 0;
      floor.connect(destination);
      floor.start();
      navigator.mediaDevices.getUserMedia = async () => destination.stream;
      globalThis.__smokeSay = async (encoded) => {
        const samples = new Float32Array(Uint8Array.from(atob(encoded), (character) => character.charCodeAt(0)).buffer);
        const source = microphone.createBufferSource();
        source.buffer = microphone.createBuffer(1, samples.length, ${RATE});
        source.buffer.copyToChannel(samples, 0);
        source.connect(destination);
        source.start();
        await new Promise((resolve) => { source.onended = resolve; });
      };
      const SpotterWorker = Worker;
      globalThis.Worker = class extends SpotterWorker {
        constructor(...args) { super(...args); this.addEventListener('message', ({ data }) => __smokeHeard.push({ ...data, at: Date.now() })); }
      };
      import(location.origin + "/puppet.js").then(({ PuppetRuntime }) => {
        const load = PuppetRuntime.prototype.load;
        PuppetRuntime.prototype.load = function(...args) { globalThis.__smokeRuntime = this; return load.apply(this, args); };
      });
      const Peer = RTCPeerConnection;
      globalThis.RTCPeerConnection = class extends Peer {
        createDataChannel(...args) {
          const channel = super.createDataChannel(...args);
          const record = { channel, events: [], spoken: '', spokeAt: 0, delegations: 0, replies: 0 }; globalThis.__smokeLiveChannels.push(record);
          const send = channel.send.bind(channel);
          channel.send = (data) => { if (/^hub_/.test(JSON.parse(data).event_id)) record.replies++; return send(data); };
          channel.addEventListener('message', ({ data }) => {
            const event = JSON.parse(data);
            if (['session.started', 'session.closed'].includes(event.type)) record.events.push(event.type);
            if (event.type === 'session.output_transcript.delta') { record.spoken += event.delta; record.spokeAt = Date.now(); }
            if (event.type === 'session.delegation.created') record.delegations++;
          });
          return channel;
        }
      };
    ` });
    await cdp.call('Page.enable');await cdp.call('Runtime.enable');await cdp.call('ServiceWorker.enable');await cdp.call('Target.setAutoAttach',{autoAttach:true,waitForDebuggerOnStart:false,flatten:true});await cdp.call('Page.addScriptToEvaluateOnNewDocument',{source:`localStorage.setItem('voice.token', ${JSON.stringify(server.token)})`});await cdp.call('Emulation.setDeviceMetricsOverride',{width:viewport.width,height:viewport.height,deviceScaleFactor:viewport.scale,mobile:viewport.mobile});await cdp.call('Page.navigate',{url:server.url});
    if (!development) {
      await cdp.evaluate(`new Promise((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('service worker did not take control')), 30000);
        const controlled = () => { if (navigator.serviceWorker.controller) { clearTimeout(timeout); resolve(); } };
        navigator.serviceWorker.addEventListener('controllerchange', controlled);
        controlled();
      })`);
      await cdp.prepareWorker();
      await cdp.evaluate('globalThis.__smokeBeforeReload = true');
      await cdp.call('Page.reload');
      const reloadDeadline = Date.now() + 30000;
      while (true) {
        try { if (await cdp.evaluate('!globalThis.__smokeBeforeReload && document.readyState === "complete"')) break; } catch {}
        if (Date.now() > reloadDeadline) throw new Error('controlled page reload did not complete');
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
    const deadline=Date.now()+180000;
    let ready=false;
    while(Date.now()<deadline){try{const page=JSON.parse(await cdp.evaluate("JSON.stringify({ready:document.querySelector('#puppet')?.getAttribute('aria-disabled')==='false',status:document.querySelector('#status')?.textContent,error:document.querySelector('#status')?.classList.contains('err')})"));ready=page.ready;if(ready)break;if(page.error)throw new Error(`page connection failed: ${page.status}`)}catch(error){if(error.message?.startsWith('page connection failed:'))throw error}await new Promise((resolve)=>setTimeout(resolve,250));}
    if(!ready){const probe=await cdp.evaluate(`JSON.stringify({status:document.querySelector('#status')?.textContent,disabled:document.querySelector('#puppet')?.getAttribute('aria-disabled'),body:document.body?.innerText?.slice(0,500)})`);throw new Error(`page did not become ready: ${probe} ${cdp.consoleErrors.join('; ')} ${chrome.stderr}`)}
    await cdp.evaluate(`new Promise((resolve, reject) => {
      const deadline = Date.now() + 240000;
      let selected = false;
      const check = () => {
        const choice = document.querySelector('#puppet-choice');
        if (!selected && choice.options.length) { selected = true; choice.dispatchEvent(new Event('change')); }
        if (selected && !choice.disabled && document.querySelector('#puppet-prompt').classList.contains('hidden')) return resolve();
        if (Date.now() > deadline) return reject(new Error('selected avatar did not load'));
        setTimeout(check, 100);
      };
      check();
    })`);
    await cdp.evaluate(`const smokeLimits = ${JSON.stringify(smokeLimits)}; globalThis.__smoke = (${installSmokeMeasurements.toString()})()`);
    const stageGeometry = async () => JSON.parse(await cdp.evaluate("JSON.stringify((() => { const box = document.querySelector('#puppet').getBoundingClientRect(); return { canvas: { left: box.left + scrollX, right: box.right + scrollX, top: box.top + scrollY, bottom: box.bottom + scrollY }, viewport: { x: scrollX, y: scrollY, width: innerWidth, height: innerHeight } }; })())"));
    const region = evidenceRegion({ neutralSilhouette, ...(await stageGeometry()) });
    if (live) {
      await cdp.evaluate(`new Promise((resolve, reject) => { const deadline = Date.now() + 300000; const check = () => { const failure = __smokeHeard.find((message) => message.error); if (failure || Date.now() > deadline) return reject(new Error(failure?.error ?? 'wake spotter did not load')); if (__smokeHeard.some((message) => message.ready)) return resolve(); setTimeout(check, 250); }; check(); })`);
      for (const index of speech.ordinary.keys()) await cdp.evaluate(`__smokeSay(__smokeSpeech.ordinary[${index}]).then(() => new Promise((resolve) => setTimeout(resolve, 1000)))`);
      await cdp.evaluate('new Promise((resolve) => setTimeout(resolve, 3000))');
      if (await cdp.evaluate('__smokeLiveChannels.length')) throw new Error('ordinary speech opened a Live session: ' + await cdp.evaluate('JSON.stringify(__smokeHeard)'));
    }
    const frames=[];
    let liveSessionOpened = false;
    let beforeFarewell;
    const liveRecord = () => cdp.evaluate('__smokeLiveChannels.map(({ channel, events, spoken, delegations, replies }) => ({ events, spoken, delegations, replies, state: channel.readyState }))');
    const quietLive = () => cdp.evaluate(`new Promise((resolve) => { const deadline = Date.now() + 30000; const check = () => { if (Date.now() - __smokeLiveChannels.at(-1).spokeAt > 2000 || Date.now() > deadline) return resolve(); setTimeout(check, 100); }; check(); })`);
    for(let second=0;second<duration;second++){
      if(second===1){await cdp.evaluate(`${transitions[0]}`);if(mode==='public')await cdp.evaluate(`(async()=>{const deadline=Date.now()+30000;while(!globalThis.__smokeChannel){if(Date.now()>deadline||document.querySelector('#status').classList.contains('err'))throw new Error('fixture session did not open: '+document.querySelector('#status').textContent);await new Promise(done=>setTimeout(done,10))}const emit=(event)=>__smokeChannel.dispatchEvent(new MessageEvent('message',{data:JSON.stringify(event)}));emit({type:'session.output_transcript.delta',delta:'I will inspect the fixture.',start_ms:0,end_ms:20});emit({type:'session.input_transcript.delta',delta:'Check the fixture stream.'});emit({type:'session.delegation.created',delegation:{id:'fixture',type:'delegation',target:'client'}});await new Promise(resolve=>setTimeout(resolve,150))})()`)}
      if (development && second === 2) {
        await cdp.evaluate(`new Promise((resolve, reject) => { const deadline = Date.now() + 60000; const check = () => { if (document.querySelector('#puppet').getAttribute('aria-pressed') === 'true') return resolve(); if (Date.now() > deadline || document.querySelector('#status').classList.contains('err')) return reject(new Error(document.querySelector('#status').textContent + ' heard ' + JSON.stringify(globalThis.__smokeHeard ?? []))); setTimeout(check, 100); }; check(); })`);
        liveSessionOpened = true;
        await cdp.evaluate(`new Promise((resolve, reject) => { const deadline = Date.now() + 30000; const check = () => { if (__smokeLiveChannels.at(-1).spoken.trim()) return resolve(); if (Date.now() > deadline) return reject(new Error('the woken session did not greet: ' + JSON.stringify(__smokeLiveChannels.map(({ events, spoken }) => ({ events, spoken }))))); setTimeout(check, 100); }; check(); })`);
        await quietLive();
        await cdp.evaluate('__smokeSay(__smokeSpeech.request)');
        await cdp.evaluate(`new Promise((resolve, reject) => { const deadline = Date.now() + 30000; const check = () => { if (__smokeLiveChannels.at(-1).delegations) return resolve(); if (Date.now() > deadline) return reject(new Error('the request was not delegated: ' + __smokeLiveChannels.at(-1).spoken)); setTimeout(check, 100); }; check(); })`);
      }
      if(second===3)await cdp.evaluate(`document.querySelector('#status').textContent=${JSON.stringify(smokeStatusText)}`);
      if(second===Math.max(8,duration-6)){if(development){await quietLive();beforeFarewell=(await liveRecord()).at(-1);if(beforeFarewell.state!=='open'||beforeFarewell.replies)throw new Error('the sleep request needs an open session with its delegation unanswered: '+JSON.stringify(beforeFarewell))}await cdp.evaluate(`${transitions[1]}`)}
      await cdp.evaluate('__smoke.sample()');
      if(!neutralSilhouette&&!clipClearsStage(region,(await stageGeometry()).canvas))throw new Error(`the stage canvas reached the evidence crop at second ${second}; this run did not load the neutral silhouette`);
      const shot=await cdp.call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false,clip:{...region,scale:1}});
      const file=path.join(output,`${viewport.name}-${String(second).padStart(3,'0')}.png`);await writeFile(file,Buffer.from(shot.result.data,'base64'));frames.push(file);
      await new Promise((resolve)=>setTimeout(resolve,1000));
    }
    if (development) {
      if (!liveSessionOpened) throw new Error('development smoke did not open a Live session');
      await cdp.evaluate(`new Promise((resolve, reject) => { const deadline = Date.now() + 60000; const check = () => { if (document.querySelector('#puppet').getAttribute('aria-pressed') === 'false' && document.querySelector('#puppet').getAttribute('aria-disabled') === 'false') return resolve(); if (Date.now() > deadline) return reject(new Error('Live session did not close after the sleep request: ' + JSON.stringify(__smokeLiveChannels.map(({ events, spoken, delegations }) => ({ events, spoken, delegations }))))); setTimeout(check, 100); }; check(); })`);
      const channels = await liveRecord();
      if (channels.length !== 1 || !channels[0].events.includes('session.started') || channels[0].state !== 'closed') throw new Error('the wake phrase did not open one session that the sleep request closed: ' + JSON.stringify(channels));
      if (!includesPhrase(channels[0].spoken.slice(beforeFarewell.spoken.length), SIGN_OFF) || channels[0].delegations !== beforeFarewell.delegations) throw new Error('the sleep request did not end the session through the sign-off without a hub call: ' + JSON.stringify(channels));
      const heard = await cdp.evaluate('__smokeHeard');
      const sessions = { channels, heard, endpoint: JSON.parse(Buffer.from(server.token, 'base64url')).endpoint_id, liveSessionOpened, liveSessionClosed: true };
      await writeFile(path.join(output, `${viewport.name}-connection.json`), JSON.stringify(sessions, null, 2));
    }
    const state=JSON.parse(await cdp.evaluate('JSON.stringify(__smoke.state)'));
    state.errors.push(...cdp.consoleErrors);
    const report={viewport:viewport.name,...assessSmoke(state),state};
    await writeFile(path.join(output,`${viewport.name}.json`),JSON.stringify(report,null,2));
    await execute('ffmpeg',['-y','-framerate','2','-i',path.join(output,`${viewport.name}-%03d.png`),'-vf','scale=iw/2:ih/2:flags=lanczos','-t','8',path.join(output,`${viewport.name}.gif`)],{timeout:120000,maxBuffer:1024*1024*10});
    return report;
  };
  try {
    return await Promise.race([exited.then((error) => { throw error; }), run()]);
  } finally {
    await chrome.close();
  }
}

const server=await makeServer();
const executable=await chromiumExecutable();
const reports=[];
try { for(const viewport of selectedViewports) reports.push(await runViewport(viewport,executable,server)); } finally { await server.close(); }
const unexpected = reports.flatMap((report) => report.failures.filter((failure) => !(baseline[report.viewport] ?? []).includes(failure)).map((failure) => `${report.viewport}:${failure}`));
await writeFile(path.join(output,'report.json'),JSON.stringify({mode,createdAt:new Date().toISOString(),reports,unexpected},null,2));
console.log('| viewport | result | failures | CLS | canvas drift |');
console.log('|---|---|---|---:|---:|');
for(const report of reports)console.log(`| ${report.viewport} | ${report.pass?'PASS':'FAIL'} | ${report.failures.join(', ')||'none'} | ${report.metrics.cumulativeLayoutShift.toFixed(3)} | ${report.metrics.canvasHeightDrift} |`);
process.exitCode=unexpected.length?1:0;
