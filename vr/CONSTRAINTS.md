# Transform constraints

`UNITY_constraints` carries an ordered `constraints` array alongside the VRM
skeleton. A constraint has a `kind` (`Parent`, `Position`, or `LookAt`), a target
`node`, a component `weight`, three-axis boolean `translation` and `rotation`
masks, local `rest` and `offset` matrices, and weighted `sources`.

Each source names either a glTF `node` or an immutable `world` matrix, with its
own `weight` and `offset` matrix. Matrices are column-major, use glTF coordinates,
and have 16 finite numbers. External matrices use the tracking world's origin;
they are not children of the avatar or its controller.

Constraints run in exported hierarchy order before spring integration. A self
source retains the previous evaluated world transform. Spring rotations from
the preceding frame remain available to constraints whose sources are spring
bones. Replacing or repositioning an avatar resets both simulation states.

The exporter resolves source assets and excludes disabled components and inactive
hierarchies. It refuses missing sources and LookAt constraints using an up object
or roll. Synthetic tests cover world anchoring under controller translation,
rotation and scale, weighted position feedback, and invalid node references.

The `wear` capture writes `spring-motion.json` beside its video frames. Each
simulated joint records its node index and name, maximum local rotation change
from the first captured frame, world-position bounds, and sample count. This
separates actual joint motion from mere inclusion in the imported rig.
The companion `constraint-motion.json` reports world rotation change and position
bounds for constraint targets, so an external anchor moving with the controller
can be detected directly on the capture device.
