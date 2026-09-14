import assert from 'node:assert/strict';
import test from 'node:test';
import { TranscriptMoodDriver, transcriptMood } from '../docs/puppet-drivers.js';

test('the transcript classifier reaches the complete mood table', () => {
  const examples = {
    curious: 'I wonder why that happened?', amused: 'That joke was funny', puzzled: "I'm confused by this",
    thinking: 'Let me think through it', pleased: 'Great, that is done', apologetic: 'Sorry, my fault',
    alert: 'Warning, be careful', sleepy: 'I am tired and sleepy', surprised: 'Wow, that was unexpected',
    skeptical: 'I doubt that; I am not convinced',
  };
  for (const [mood, text] of Object.entries(examples)) assert.equal(transcriptMood(text), mood);
  assert.equal(transcriptMood('A neutral statement.'), null);
});

test('streamed transcript follows the newest cue and resets between sessions', () => {
  const applied = [];
  const driver = new TranscriptMoodDriver((mood) => applied.push(mood));
  driver.push('That is won');
  driver.push('derful');
  driver.push('. Sorry about that');
  driver.reset();
  driver.push('Wonderful');
  assert.deepEqual(applied, ['pleased', 'apologetic', 'pleased']);
});

test('the newest repeated cue wins and neutral deltas do not replay it', () => {
  const applied = [];
  const driver = new TranscriptMoodDriver((mood) => applied.push(mood));
  driver.push('Great. Sorry. Great.');
  driver.push(' Here is the result.');
  assert.deepEqual(applied, ['pleased']);
});
