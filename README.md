# voice

Push-to-talk page to the bothouse hub (bddap/bothouse#326). Served from GitHub
Pages (`docs/`), it reaches `voice-web` on bothouse over the same wasm iroh client
the botq dashboard ships (imported from https://bddap-bot.github.io/botq/), with a
token pasted once on the device. The mic exists only while the button is held;
release ends the turn and uploads one 16 kHz wav. Replies the hub queues with
`voice-reply web '<text>'` are pushed down the same stream, spoken, and acked.
The button labels each step: hold to talk → listening… → sending… → thinking… →
hold to talk; the status line carries connection words only, diagnostics go to
the console.

Token: `voice-web token` on bothouse. Tests: `node --test test/*.test.js`.
