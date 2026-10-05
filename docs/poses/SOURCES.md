# Pose sources

The puppet's body moves only through recorded human motion: motion capture or an animator's work. No pose or motion is keyframed in code. Gaze turns the head a little toward the viewer, and the runtime crossfades between clips; both are computed.

The server serves the puppet's idles, chair sit and gestures with its own catalog, which records each clip's source and licence. This directory holds the one pose the page and the VR host load themselves.

## standing.json

The rest pose. The standing idle clips are re-anchored to it. It is the `Standing` preset of the VRM viewer at https://vrm-viewer.ownverse.world/, exported from that viewer as a bone-to-quaternion map. One viewer-specific field was dropped.
