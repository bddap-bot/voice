# Browser smoke verification

Run `npm ci`, then `npm run smoke:ci` from the repository root. CI and the touched-path acceptance gate use this shared target, including the checked-in baseline and the full viewport set. Changes anywhere under `docs/`, browser source and build inputs, smoke measurements, or gate configuration require it.

The runner requires Node.js 22 or newer, headless Chromium, and ffmpeg. Set `CHROMIUM_BIN` if Chromium is outside the executable search path. Missing prerequisites and failures outside the baseline produce a nonzero exit code; neither is a skipped or successful check. Measurements, viewport frames, and GIFs are written to `smoke-artifacts/`.
