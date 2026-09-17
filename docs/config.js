export default { token: null, storageKey: 'voice.token', serviceWorker: true };

export function configuredToken(config, saved) {
  return config.token ?? saved;
}
