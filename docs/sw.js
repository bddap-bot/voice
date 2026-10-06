const CACHE = 'voice-shell-v13';
const SHELL = ['.', 'index.html', 'main.js', 'live.js', 'config.js', 'puppet-client.js', 'puppet-drivers.js', 'puppet.js', 'lib/render.js', 'manifest.webmanifest', 'icons/icon.svg'];
const HASHED = /\/lib\/[^/]+-[A-Z0-9]{8}\.(js|css|woff2|wasm)$/;

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL.map((path) => new Request(path, { cache: 'no-cache' })))).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith('voice-shell-') && k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (e) => {
  const req = e.request;
  if (req.method !== 'GET') return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  const network = (init) => fetch(req, init).then((res) => {
    if (res && res.ok) {
      const copy = res.clone();
      e.waitUntil(caches.open(CACHE).then((c) => c.put(req, copy)));
    }
    return res;
  });
  e.respondWith(HASHED.test(url.pathname) ? caches.match(req).then((hit) => hit || network()) : network({ cache: 'no-cache' }).catch(() => caches.match(req)));
});
