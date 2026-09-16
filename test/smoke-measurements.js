export const smokeViewports = [
  { name: 'phone', width: 390, height: 844, scale: 3, mobile: true },
  { name: 'laptop', width: 1440, height: 900, scale: 1, mobile: false },
  { name: 'tv', width: 1920, height: 1080, scale: 1, mobile: false },
];

export const smokeLimits = {
  cumulativeLayoutShift: 0.1,
  heightDrift: 1,
  droppedFrameMs: 50,
};

export function transitionFrameSampler(gaps, limit, schedule = requestAnimationFrame, cancel = cancelAnimationFrame) {
  let frameId;
  let lastFrame;
  const frame = (now) => {
    if (lastFrame !== undefined && now - lastFrame > limit) gaps.push(Math.round(now - lastFrame));
    lastFrame = now;
    frameId = schedule(frame);
  };
  return {
    start() {
      if (frameId !== undefined) cancel(frameId);
      lastFrame = undefined;
      frameId = schedule(frame);
    },
    stop() {
      if (frameId !== undefined) cancel(frameId);
      frameId = undefined;
      lastFrame = undefined;
    },
  };
}

export function installSmokeMeasurements() {
  const selectors = ['header', '#saved', '.puppet-picker', '#puppet-credit', '#elapsed', '.share', '.display', '.ledger'];
  const state = { cls: 0, moves: [], overlaps: [], heights: [], blankFrames: [], frameGaps: [], errors: [], telemetryRejections: [] };
  const canvas = document.querySelector('#puppet');
  const rect = (element) => {
    const value = element.getBoundingClientRect();
    return { left: value.left, right: value.right, top: value.top, bottom: value.bottom, width: value.width, height: value.height };
  };
  const intersects = (a, b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
  let previous = new Map();
  let second = 0;
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) if (!entry.hadRecentInput) state.cls += entry.value;
  }).observe({ type: 'layout-shift', buffered: true });
  window.addEventListener('error', (event) => state.errors.push(event.message));
  window.addEventListener('unhandledrejection', (event) => state.errors.push(String(event.reason?.message ?? event.reason)));
  const originalWarn = console.warn;
  console.warn = (...values) => {
    const line = values.map(String).join(' ');
    if (/telemetry|acknowledgment|unrecognized request/i.test(line)) state.telemetryRejections.push(line);
    originalWarn(...values);
  };
  const transitionFrames = transitionFrameSampler(state.frameGaps, smokeLimits.droppedFrameMs);
  const sample = () => {
    second++;
    const canvasRect = rect(canvas);
    const status = document.querySelector('#status')?.textContent ?? '';
    if (/telemetry|acknowledgment|unrecognized request/i.test(status) && !state.telemetryRejections.includes(status)) state.telemetryRejections.push(status);
    state.heights.push({ second, canvas: canvasRect.height, stage: document.querySelector('main').getBoundingClientRect().height });
    for (const selector of selectors) {
      const element = document.querySelector(selector);
      if (!element || element.classList.contains('hidden')) continue;
      const value = rect(element);
      const before = previous.get(selector);
      if (before && (Math.abs(before.left - value.left) > 1 || Math.abs(before.top - value.top) > 1 || Math.abs(before.width - value.width) > 1 || Math.abs(before.height - value.height) > 1) && element.getAnimations({ subtree: true }).length === 0) state.moves.push({ second, selector, before, after: value });
      previous.set(selector, value);
      if (intersects(canvasRect, value)) state.overlaps.push({ second, selector });
    }
    try {
      const blank = document.createElement('canvas');
      blank.width = canvas.width;
      blank.height = canvas.height;
      const renderCalls = globalThis.__smokeRuntime?.renderer?.info?.render?.calls;
      if (renderCalls === 0 || (renderCalls === undefined && canvas.toDataURL() === blank.toDataURL())) state.blankFrames.push(second);
    } catch (error) {
      state.errors.push(error.message);
    }
  };
  return {
    state,
    sample,
    startTransition() { transitionFrames.start(); },
    stopTransition() { transitionFrames.stop(); },
  };
}

export function assessSmoke(state) {
  const canvasHeights = state.heights.map((value) => value.canvas);
  const stageHeights = state.heights.map((value) => value.stage);
  const drift = (values) => values.length ? Math.max(...values) - Math.min(...values) : Infinity;
  const checks = {
    layoutShift: state.cls <= smokeLimits.cumulativeLayoutShift,
    unexplainedMovement: state.moves.length === 0,
    overlaps: state.overlaps.length === 0,
    canvasGrowth: drift(canvasHeights) <= smokeLimits.heightDrift && drift(stageHeights) <= smokeLimits.heightDrift,
    paintedPuppet: state.blankFrames.length === 0,
    browserErrors: state.errors.length === 0,
    telemetry: state.telemetryRejections.length === 0,
    transitionFrames: state.frameGaps.length === 0,
  };
  return { pass: Object.values(checks).every(Boolean), checks, metrics: { cumulativeLayoutShift: state.cls, canvasHeightDrift: drift(canvasHeights), stageHeightDrift: drift(stageHeights), maximumFrameGapMs: Math.max(0, ...state.frameGaps) }, failures: Object.entries(checks).filter(([, pass]) => !pass).map(([name]) => name) };
}
