# Pose sources

Every body pose and motion the puppet plays comes from a human: motion capture or an animator's file. None is authored in code.

## sit-floor.vrma

The overlay's seat: a floor sit with the shins crossed and the hands on the knees.

- Source: Motion Capture Database HDM05, scene 4-1 "Chair, table, floor", motion "sit down on floor", trial `HDM_bk_04-01_02_120.amc` with skeleton `HDM_bk.asf` (optical capture of a live performer, 120 Hz). https://resources.mpi-inf.mpg.de/HDM05/
- Frames: AMC 5777–5904, the settled part of the seated hold.
- Licence: [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/). This file is a derivative and is distributed under the same licence.
- Attribution: The data used in this project was obtained from HDM05. M. Müller, T. Röder, M. Clausen, B. Eberhardt, B. Krüger, A. Weber: Documentation Mocap Database HDM05. Technical Report CG-2007-2, Universität Bonn, 2007.
- Changes: the AMC was rewritten as BVH without changing any joint rotation (the wrist-flex and thumb joints are dropped), the root was centred horizontally, and the BVH was converted with [vrm-c/bvh2vrma](https://github.com/vrm-c/bvh2vrma) at `da148d9a` using an explicit humanoid bone map. The whole clip is turned 180° about the vertical axis so the figure faces the viewer, and the captured frames play forward then backward so the loop has no seam.

## standing.json

The rest pose under the standing idles: the `Standing` preset exported from https://vrm-viewer.ownverse.world/, with the viewer's `code` field dropped.
