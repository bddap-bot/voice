import { SIGN_OFF } from '../docs/identity.js';
import { includesPhrase } from '../docs/live.js';
import { spokenBetween } from './wake-reply-measurements.js';

export const REPLY_SAID = /\bseventeen\b|\b17\b/i;
export const SILENCE = /\b(quiet|silent|silence|step(ping)? back|hold(ing)? (off|back)|i'll hold|hang(ing)? back|stay(ing)? out|stand(ing)? by)\b/i;
const NEAR_PHRASE = /^(corvus|korvus|quiet|fight|flight)$/;
const words = (text) => text.toLowerCase().replace(/[^a-z' ]+/g, ' ').split(/\s+/).filter(Boolean);

// Played speech before the relay longer than an acknowledgement is the model's own answer.
const ACKNOWLEDGEMENT_WORDS = 12;
const AUDIBLE = 0.01;
const LEVEL_MS = 50;

// The page's own session records in a window, as `[name, detail, at]`, each resent batch counted once.
export function sessionRecords(capture, from, to, names = /^(hand-off-quiet|hub-reply-)/) {
  const batches = new Map(capture.frames.filter(({ batch }) => batch).map(({ batch, at }) => [batch.id ?? `unread ${at}`, batch.events]));
  return [...batches.values()].flat().filter(({ kind, name, at }) => kind === 'session' && names.test(name) && at >= from && at <= to).map(({ name, detail, at }) => [name, JSON.parse(detail), at]);
}

// Spans of one trial in which the page played none of the model's speech, by its own records; one still open closes with the trial.
function quietSpans(capture, from, to) {
  const spans = [];
  for (const [, { on }, at] of sessionRecords(capture, from, to, /^hand-off-quiet$/)) {
    if (on) spans.push([at, to]);
    else if (spans.length) spans.at(-1)[1] = at;
  }
  return spans;
}

// Two beginnings or two endings in a row, or a batch the capture cut short, mean a record may be missing, which leaves what was played unknown.
function quietUnpaired(capture, from, to) {
  const changes = sessionRecords(capture, from, to, /^hand-off-quiet$/).map(([, { on }]) => on);
  return capture.frames.some(({ batch, at }) => batch?.unread && at >= from && at <= to) || changes.some((on, index) => on === (changes[index - 1] ?? false));
}

// What the model said in a window that the page played, its words in arrival order.
function playedBetween(capture, ch, from, to, spans) {
  const played = { ...capture, rt: capture.rt.filter(({ at }) => !spans.some(([start, end]) => at >= start && at < end)) };
  return spokenBetween(played, ch, from, to);
}

export function delegatesSince(capture, since) {
  return capture.frames.filter((frame) => frame.dir === 'out' && frame.verb === 'delegate' && frame.at >= since).map((frame) => ({ at: frame.at, text: JSON.parse(frame.text.slice(frame.text.indexOf('\n') + 1)).text }));
}

// One trial of a case: `turnAt` and `speechEnd` are when each clip started and the last ended, `replyAt` when the stub reply reached the page (null when nothing was relayed), `end` when the trial stopped watching.
export function scoreTrial({ capture, ch, item, start, turnAt, speechEnd, replyAt, end }) {
  const sent = delegatesSince(capture, start).filter(({ at }) => at <= (replyAt ?? end));
  const heard = new Set(words(sent.map(({ text }) => text).join(' ')));
  const coverage = item.turns.map((turn) => {
    const want = words(turn).filter((word) => word.length > 3 && !NEAR_PHRASE.test(word));
    return want.length ? want.filter((word) => heard.has(word)).length / want.length : 1;
  });
  const firstRelay = sent[0]?.at ?? null;
  const afterPhrase = spokenBetween(capture, ch, turnAt.at(-1), replyAt ?? end);
  const spans = quietSpans(capture, start, end);
  const playedAfterPhrase = playedBetween(capture, ch, turnAt.at(-1), replyAt ?? end, spans);
  const levels = (capture.levels ?? []).filter(({ at }) => at >= speechEnd && at < (replyAt ?? end));
  const afterReply = replyAt === null ? '' : playedBetween(capture, ch, replyAt, end, spans);
  const playedBeforeRelay = playedBetween(capture, ch, start, firstRelay ?? end, spans);
  const played = playedBetween(capture, ch, start, end, spans);
  return {
    name: item.name,
    kind: item.kind,
    relayed: sent.length > 0,
    relays: sent.length,
    whole: sent.length === 1 && coverage.every((share) => share >= 0.5),
    clarifying: /\?/.test(spokenBetween(capture, ch, start, firstRelay ?? end)),
    ownAnswer: /\?/.test(playedBeforeRelay) || words(playedBeforeRelay).length > ACKNOWLEDGEMENT_WORDS || (!sent.length && words(played).length > 0),
    spoke: words(played).length > 0,
    signedOff: includesPhrase(played, SIGN_OFF),
    promisedSilence: SILENCE.test(afterPhrase),
    playedSilence: SILENCE.test(playedAfterPhrase),
    audibleAfterSpeechMs: levels.length ? levels.filter(({ rms }) => rms > AUDIBLE).length * LEVEL_MS : null,
    replySpoken: replyAt === null ? null : REPLY_SAID.test(afterReply),
    relayMs: sent.length ? sent.at(-1).at - speechEnd : null,
    beforeRelay: spokenBetween(capture, ch, start, firstRelay ?? end),
    playedBeforeRelay,
    afterPhrase,
    playedAfterPhrase,
    delegated: sent.map(({ text }) => text),
    afterReply,
    quietUnpaired: quietUnpaired(capture, start, end),
    records: sessionRecords(capture, start, end).map(([name, detail, at]) => [name, detail, at - start]),
  };
}

const median = (values) => {
  const sorted = values.filter(Number.isFinite).sort((a, b) => a - b);
  if (!sorted.length) return null;
  const middle = sorted.length >> 1;
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};

export function summarize(rows) {
  const count = (list, test) => `${list.filter(test).length}/${list.length}`;
  return Object.fromEntries([...new Set(rows.map(({ kind }) => kind))].map((kind) => {
    const list = rows.filter((row) => row.kind === kind);
    const relayed = list.filter((row) => row.replySpoken !== null);
    return [kind, {
      trials: list.length,
      relayed: count(list, (row) => row.relayed),
      whole: count(list, (row) => row.whole),
      fragmented: count(list, (row) => row.relays > 1),
      clarifying: count(list, (row) => row.clarifying),
      ownAnswer: count(list, (row) => row.ownAnswer),
      spoke: count(list, (row) => row.spoke),
      signedOff: count(list, (row) => row.signedOff),
      promisedSilence: count(list, (row) => row.promisedSilence),
      playedSilence: count(list, (row) => row.playedSilence),
      medianAudibleAfterSpeechMs: median(list.map((row) => row.audibleAfterSpeechMs)),
      replySpoken: count(relayed, (row) => row.replySpoken),
      medianRelayMs: median(list.map((row) => row.relayMs)),
    }];
  }));
}
