const CACHE = 'voice-shell-v9';
const SHELL = ['.', 'index.html', 'live.js', 'puppet-client.js', 'puppet-drivers.js', 'puppet-tools.js', 'puppet.js', 'lib/render.js', 'manifest.webmanifest', 'icons/icon.svg'];
const HASHED = /\/lib\/[^/]+-[A-Z0-9]{8}\.(js|css|woff2)$/;

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
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
  const network = () => fetch(req).then((res) => {
    if (res && res.ok) caches.open(CACHE).then((c) => c.put(req, res.clone()));
    return res;
  });
  e.respondWith(HASHED.test(url.pathname) ? caches.match(req).then((hit) => hit || network()) : network().catch(() => caches.match(req)));
});
