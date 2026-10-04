# Pose sources

The puppet's body moves only through recorded human motion: motion capture or an animator's work. No pose or motion is keyframed in code. Gaze turns the head a little toward the viewer, and the runtime crossfades between clips; both are computed.

Two sets of clips exist. The server serves the puppet's idles, chair sit and gestures with its own catalog, which records each clip's source and licence. The files in this directory are the ones the page itself loads.

## sit-floor-down.vrma, sit-floor.vrma, sit-floor-up.vrma

In the SteamVR overlay, the puppet sits on the floor of the overlay quad instead of on a chair. These three files are that sit: sitting down, the seated hold with the shins crossed and the hands on the knees, and getting up.

- Source: Motion Capture Database HDM05, scene 4-1 "Chair, table, floor", motion "sit down on floor", trial `HDM_bk_04-01_02_120.amc` with skeleton `HDM_bk.asf`, an optical capture of a live performer at 120 Hz. https://resources.mpi-inf.mpg.de/HDM05/
- Frames: sit-down 5372–5817, hold 5817–5893, get-up 5893–6254. The sit-down runs until the hands have settled on the knees, and the hold stops before the arms start the get-up.
- Licence: [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/). The three `.vrma` files are derivatives and are distributed under the same licence.
- Attribution: The data used in this project was obtained from HDM05. M. Müller, T. Röder, M. Clausen, B. Eberhardt, B. Krüger, A. Weber: Documentation Mocap Database HDM05. Technical Report CG-2007-2, Universität Bonn, 2007.
- Changes: the AMC was rewritten as BVH without changing any joint rotation. The wrist-flex and thumb joints were dropped, and the root was shifted horizontally by the hold's mean position. Each BVH was converted with [vrm-c/bvh2vrma](https://github.com/vrm-c/bvh2vrma) at `da148d9a`, given an explicit humanoid bone map because its automatic mapper misreads this skeleton. Each clip is turned 180° about the vertical axis so the figure faces the viewer. `sit-floor.vrma` holds the captured frames forward and then backward, so it loops without a seam.

## standing.json

The rest pose. The standing idle clips are re-anchored to it. It is the `Standing` preset of the VRM viewer at https://vrm-viewer.ownverse.world/, exported from that viewer as a bone-to-quaternion map. One viewer-specific field was dropped.
