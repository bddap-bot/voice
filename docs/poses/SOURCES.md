# Pose sources

The puppet's body moves only through recorded human motion: motion capture or an animator's work. No pose or motion is keyframed in code. Gaze turns the head a little toward the viewer, and the runtime crossfades between clips; both are computed.

The server serves the puppet's idles, chair sit and gestures with its own catalog, which records each clip's source and licence. This directory holds the one pose the page and the VR host load themselves, and the list of motion-capture takes the server's catalog adds.

## standing.json

The rest pose. It is used before a clip is available and for bones without a track. Clips retain their authored poses. It is the `Standing` preset of the VRM viewer at https://vrm-viewer.ownverse.world/, exported from that viewer as a bone-to-quaternion map. One viewer-specific field was dropped.

## mocap.json

Clips cut from the CMU Graphics Lab Motion Capture Database, http://mocap.cs.cmu.edu/. Its terms: "This data is free for use in research projects. You may include this data in commercially-sold products, but you may not resell this data directly, even in converted form." It asks for this acknowledgement: "The data used in this project was obtained from mocap.cs.cmu.edu. The database was created with funding from NSF EIA-0196217."

`node scripts/mocap.mjs write <clip dir>` downloads each take's ASF skeleton and AMC motion, checks them against the SHA-256 digests in `mocap.json`, retargets each window onto the VRM humanoid's T-pose, turns it to face +Z, and writes it as `<action>.motion` with its manifest entry, source and licence included. The clips are written outside the repository. `node scripts/mocap.mjs pin` re-records the digests.

`mocap.json` lists each clip: its action, the take it is cut from with the database's description of it, and the window of the take in seconds. A clip's action names what plays it: `idle-<n>` joins the standing idles, `<gesture>-<n>` is another take of that gesture, and a clip named after a mood plays once when the puppet takes on that mood.

