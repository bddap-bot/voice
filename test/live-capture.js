const EVENTS = new Set(['session.started', 'input_transcript.added', 'output_transcript.added', 'delegation.created', 'session.context.append', 'session.context.appended', 'delegation.context.append', 'delegation.context.appended']);
const FIELDS = new Set(['type', 'raw', 'channel', 'delegation_item_id', 'content', 'start_ms', 'end_ms']);

export const eventText = (event) => event.item?.text ?? event.content?.map(({ text }) => text).join('') ?? '';

export function recordedRows(rt) {
  return rt.filter(({ dir, event }) => dir === 'state' || EVENTS.has(event.type)).map((row) => {
    const event = Object.fromEntries(Object.entries(row.event).filter(([key]) => FIELDS.has(key)));
    if (row.event.item) event.item = Object.fromEntries(Object.entries(row.event.item).filter(([key]) => ['id', 'text'].includes(key)));
    return { ...row, event };
  });
}

export const hubFrames = (capture) => capture.frames.filter((frame) => frame.verb === 'hub').map((frame) => ({ at: frame.at, ...JSON.parse(frame.text.slice(4)) }));

// v3 events carry no ids: a hub reply is the speakable append addressed to its delegation (or, for a page-ended turn, to the session) with its text, and its ack is the same-addressed acknowledgement in send order.
export function replySent(capture, ch, hub) {
  const addressed = ({ event }) => event.type === 'delegation.context.append' ? event.delegation_item_id === hub.id : event.type === 'session.context.append' && event.channel === 'speakable';
  return capture.rt.find((row) => row.ch === ch && row.dir === 'out' && row.at >= hub.at && addressed(row) && eventText(row.event) === hub.commentary[0]);
}

export function applied(capture, sent) {
  const key = ({ ch, event }) => ch === sent.ch && event.delegation_item_id === sent.event.delegation_item_id;
  const sends = capture.rt.filter((row) => row.dir === 'out' && row.event.type === sent.event.type && key(row));
  const acks = capture.rt.filter((row) => row.dir === 'in' && row.event.type === `${sent.event.type}ed` && key(row));
  return acks[sends.indexOf(sent)];
}
