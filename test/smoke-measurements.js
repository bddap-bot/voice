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

function stageSafeClip(canvas, viewport) {
  const viewportRight = viewport.x + viewport.width;
  const viewportBottom = viewport.y + viewport.height;
  const clampX = (value) => Math.min(Math.max(value, viewport.x), viewportRight);
  const clampY = (value) => Math.min(Math.max(value, viewport.y), viewportBottom);
  const stageLeft = clampX(Math.floor(canvas.left));
  const stageRight = clampX(Math.ceil(canvas.right));
  const stageTop = clampY(Math.floor(canvas.top));
  const stageBottom = clampY(Math.ceil(canvas.bottom));
  return [
    { x: viewport.x, y: viewport.y, width: stageLeft - viewport.x, height: viewport.height },
    { x: stageRight, y: viewport.y, width: viewportRight - stageRight, height: viewport.height },
    { x: viewport.x, y: viewport.y, width: viewport.width, height: stageTop - viewport.y },
    { x: viewport.x, y: stageBottom, width: viewport.width, height: viewportBottom - stageBottom },
  ].filter((band) => band.width > 0 && band.height > 0).sort((a, b) => b.width * b.height - a.width * a.height)[0] ?? null;
}

export function clipClearsStage(clip, canvas) {
  return clip.x + clip.width <= canvas.left || clip.x >= canvas.right || clip.y + clip.height <= canvas.top || clip.y >= canvas.bottom;
}

export function evidenceRegion({ neutralSilhouette, canvas, viewport }) {
  if (neutralSilhouette) return { ...viewport };
  const clip = stageSafeClip(canvas, viewport);
  if (!clip) throw new Error('no part of the viewport clears the stage canvas, and this run did not load the neutral silhouette');
  return clip;
}

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
