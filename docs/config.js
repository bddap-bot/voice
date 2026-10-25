export default { origin: 'https://voice.dirksen.pw', formerOrigin: 'https://bddap-bot.github.io/voice-moved/', token: null, storageKey: 'voice.token', serviceWorker: true, transferTimeout: 30000 };

export function configuredToken(config, saved) {
  return config.token ?? saved;
}
