# Smoke frame-gap diagnosis

The browser-smoke artifact from run 35400444098 contains one phone callback gap: 67 ms. Its other nine checks pass: CLS, canvas height drift, and stage height drift are zero; movement, overlap, blank-frame, browser-error, and telemetry-rejection arrays are empty. The other three viewports have empty gap arrays. Their reported 0 ms means no gap exceeded 50 ms, not that frames took no time.

At commit 3708c17, transitionFrameSampler records rounded differences between requestAnimationFrame timestamps over 50 ms. It records neither animation progress nor callback work duration, sample location, or a scheduler trace. The public smoke puppet draws synchronously and has no animation loop. Screenshot sampling is already excluded, with two warm-up callbacks before sampling resumes, but browser scheduling still determines subsequent timestamp gaps.

The artifact establishes an isolated scheduling-sensitive failure, not a phone layout or animation defect. It cannot distinguish runner contention, rendering warm-up, or other browser scheduling delays. Attributing the exact 67 ms interval to one of those causes would require evidence this run did not collect.

Remove the frame-gap assertion, sampler, threshold, and reporting instead of increasing the threshold or adding more warm-up delays. The gate retains all nine visual/error checks, both transition actions, and screenshots at every viewport. This smoke gate does not certify animation frame rate.
