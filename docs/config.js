export default { token: null, storageKey: 'voice.token', serviceWorker: true, transferTimeout: 30000 };

export function configuredToken(config, saved) {
  return config.token ?? saved;
}
