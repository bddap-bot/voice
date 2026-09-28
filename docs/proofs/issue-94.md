# Issue 94: a tap right after a session ends opens a fresh one

## Cause

Stopping sent the relay `cancel`, and the broker then hung up the Live session and closed its channel. The page went on to wait up to 5 s for a `session.closed` event that could no longer arrive. Through that wait it kept the ended conversation, although the puppet had already sat down. A tap during the wait counted as a second stop, so nothing opened. While the page was hidden, timer throttling held the wait open until the page was shown again. Every later start also waited for the whole stop to finish, including the closing of its audio contexts.

Reproduced at `2b78709` with the headless Chromium rig, driving the development relay and real Live sessions, on an instrumented copy of the page:

- **Sign-off:** the model said the sign-off sentence and the puppet sat. A tap 2.4 s later found `conversation: true, stopping: true`. No new offer was sent. The wait ran out 3 s after the tap, and nothing had opened 40 s later.
- **Tap:** the same happened when a tap ended the session while the model was speaking and the next tap came 3 s later.

## Fix

`fd40c8a` releases the conversation the moment it stops. The ended session's recorders, channel and audio are closed without anything waiting on them. `LivePlayback.quiet()` now resolves once playback is closed, so a hub reply queued behind a stopped session still gets its acknowledgment.

## Proof on the deployed page

The rig loaded `https://bddap-bot.github.io/voice/` at `140ea42`, which contains `fd40c8a`, with no instrumentation. It ran against the development relay and real Live sessions.

| Run | Ended by | Tap after the stop | Next session opened | Heard | Replied |
|---|---|---|---|---|---|
| 1 | spoken sign-off | 1.5 s | 2.3 s later | "Can you hear me? Please say the word pineapple" | "What's up? Pineapple." |
| 2 | tap during a hub reply | 2 s | 13.9 s later | "Can you hear me? Please say the word pineapple" | "pineapple" |

Relay journal for both runs. Animation, per-word transcript and action lines are omitted, and runs of identical lines are shown once.

```
2026-09-28T23:01:54.589057Z session live-config-sdp-answer
2026-09-28T23:01:55.524009Z session open
2026-09-28T23:01:55.524050Z session live-config-session-started
2026-09-28T23:02:02.502453Z session input_utterance
2026-09-28T23:02:05.102050Z session sleep
2026-09-28T23:02:09.017789Z session close
2026-09-28T23:02:13.444579Z session live-config-sdp-answer
2026-09-28T23:02:14.051992Z session open
2026-09-28T23:02:14.052196Z session live-config-session-started
2026-09-28T23:02:32.941007Z session input_utterance
2026-09-28T23:02:56.179223Z session live-config-sdp-answer
2026-09-28T23:02:56.518449Z session open
2026-09-28T23:02:56.518498Z session live-config-session-started
2026-09-28T23:03:05.372819Z session input_utterance
2026-09-28T23:03:07.801531Z session live-config-session-delegation-created
2026-09-28T23:03:15.033105Z session close
2026-09-28T23:03:31.320909Z session live-config-sdp-answer
2026-09-28T23:03:35.858854Z session open
2026-09-28T23:03:35.858905Z session live-config-session-started
2026-09-28T23:03:42.721602Z session input_utterance
```
