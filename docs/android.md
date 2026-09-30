# Live Voice on Android

[Install Live Voice](https://github.com/bddap-bot/voice/releases/download/android-v1/live-voice.apk) (Android 9 or newer). The app loads the same [Live page](https://bddap-bot.github.io/voice/) as the browser. Conversation logic, authentication, wake detection, speaker filtering and replies remain in that page; the APK supplies Android lifecycle ownership.

Tap the APK link → Download → Open → Settings → Allow from this source → Back → Install → Open. Tap **Open Live Voice**, allow microphone access while using the app, and allow notifications. Paste your existing connection token into the Live page and connect, then start a conversation as usual. Browser storage does not transfer to the app. You can turn off “Allow from this source” after installation.

Switch apps or lock the screen after the session starts. The **Live Voice is open** notification identifies the active service. **Stop** in that notification or **Stop and close** in the app releases the microphone, WebView and wake lock. Android force-stop also ends the session. The ordinary page's sleep/wake and mute controls retain their existing meaning; close the Android host to end background operation entirely. An OS-killed process does not silently restart capture.

## Diagnosis, from capture upward

The reported symptom is capture apparently stopping shortly after an app switch. There is no recording or lifecycle trace from that phone, so this report does **not** establish a muted microphone, ended track, suspended context, broken transport, or frozen/discarded page as the observed cause.

The source path is `getUserMedia` in `index.html` → optional speaker AudioWorklet gate → `RTCPeerConnection.addTrack` → model. Replies return as a remote WebRTC audio track → `LivePlayback` AudioContext/AudioWorklet → the speaker audio element. The authenticated relay exchanges the offer and session events separately. Hiding the page pauses puppet rendering, not the microphone intentionally. A screen wake lock only helps while the page is visible; it does not confer background microphone permission.

`audio-diagnostics.js` records, locally, the microphone's initial/mute/unmute/end states, AudioContext state transitions for playback/speaker filtering/wake detection, peer connection transitions, five-second audio RTP counters and microphone sample duration/energy, visibility/freeze/resume/pagehide/pageshow, and `document.wasDiscarded` at load. The newest 300 records survive reload under `voice.audio-diagnostics.v1` in local storage. No audio, transcript, credential, SDP, network address or device identifier is recorded by these diagnostics. They are not uploaded. A gap alone cannot identify whether the process was frozen or killed.

Interpret the lowest failing layer first:

- Microphone `muted: true` or `state: ended` establishes a capture interruption. An advancing sample duration with zero energy alone may simply be silence.
- A suspended gate context can stop outgoing filtered audio even with a live microphone. A suspended playback context can stop spoken replies with a connected peer.
- Advancing microphone samples with stalled outbound packets moves investigation to the sender/transport. Peer `disconnected`/`failed` is separate evidence from capture loss. Inbound counters establish received audio, not audible speaker output.
- `freeze`, followed by a record gap and `resume`, establishes a frozen-page interval. `discarded: true` establishes a later reload after discard. Missing records without those signals are inconclusive.

Chrome describes frozen pages as having freezable tasks suspended and notes that mobile termination is not reliably observable: [Page Lifecycle API](https://developer.chrome.com/docs/web-platform/page-lifecycle-api). Installing a PWA does not give this application an Android microphone foreground service. A browser-only fix cannot promise both background and locked-screen operation across Android browsers.

Android explicitly supports continued background microphone capture through the [microphone foreground service type](https://developer.android.com/develop/background-work/services/fgs/service-types#microphone). The app starts that service while its activity is visible and after microphone permission; [background-start restrictions](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start) prohibit treating permission alone as authorization to start capture invisibly. The service also declares media playback, retains the WebView independently of activity recreation, keeps the renderer important when invisible, and holds a partial CPU wake lock. It never calls WebView `onPause` or `pauseTimers` when the activity hides; those are explicit host operations described by the [WebView API](https://developer.android.com/reference/android/webkit/WebView). WebView still owns media capture; there is no second native conversation or PCM client.

The notification covers the lifetime of the open host, including idle time before a conversation. OEM battery policies, microphone privacy controls, calls or other apps competing for audio, network loss and Android force-stop can interrupt it. A foreground-service declaration is an implementation mechanism, not evidence that a particular phone passed the two background scenarios.

## Build and verification

Install JDK 17 or newer, Android SDK platform 35, build-tools 35.0.0 and zip. Set `ANDROID_HOME`, `VOICE_KEYSTORE` and `VOICE_STORE_PASSWORD`, then run `bash android/build.sh`. It compiles Java, builds DEX, packages and aligns the APK, signs it, and verifies the signature. The result is `android/build/live-voice.apk`. Keep the signing key private and reuse it for updates. The APK contains no connection token and does not permit cleartext HTTP, file access, backups or navigation to other pages. Only audio capture is granted, only for the deployed HTTPS origin while the top-level page is Live; no JavaScript-to-native bridge is exposed.

Run `npm test`, `npm run build` and `npm run smoke:ci` for the page. Browser unit tests exercise the diagnostic state distinctions, persistence bound and exclusion of content/credentials. Android runtime evidence and remaining physical-device checks are reported with the release; desktop tests alone cannot establish Android background capture.
