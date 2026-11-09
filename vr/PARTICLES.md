# Particle systems

`UNITY_particles` carries an avatar's Unity ParticleSystems as `{"systems": [...],
"meshes": [...]}`. The exporter samples every curve and gradient and mirrors every
vector into the avatar's glTF axes, so the overlay needs none of Unity's types.

A curve is `{"min": [...], "max": [...], "scale": [a, b]}`: samples evenly spaced
from 0 to 1, read linearly between neighbours, a lone sample holding everywhere. Its
value at `t` is `a * min(t)` blended toward `b * max(t)` by the particle's own random
draw for that property. A gradient is `{"min": [[r, g, b, a]...], "max": [...]}`,
read the same way. Colours in the file, gradients and materials alike, are sRGB,
possibly above 1, with linear alpha; the overlay makes them linear once as it reads
them.

A system names its glTF `node`, whose world position and rotation are its emitter
(scale and simulation speed are baked by the exporter). It has its cycle
(`duration`, `loop`, `delay`), `awake` (plays from its start whenever it becomes
active), `world` (particles keep their world position rather than moving with the
emitter) and `max` live particles. Curves over the cycle, read
when a particle is born, give its `lifetime`, `speed`, `gravity` (a share of 9.81 m/s²
down), `size` and `rotation` (one curve or three axes; a lone rotation turns about the
view axis), `color` (a gradient), and the emission `rate` per second and `distance`
per metre the emitter moves. `bursts` lists `{"time", "count", "cycles", "interval",
"probability"}` within each cycle. `shape` (`point`, `sphere`, `hemisphere`, `cone`,
`box`, `circle`, `edge`, all facing +Z, placed in the emitter's axes by its `matrix`)
gives where particles start and which way they leave. Over a particle's life,
`velocity` adds curves in world or emitter axes, `limit` dampens speed past its curve
(read midway between its ends) each thirtieth of a second, `noise` wanders it, `sizeOverLife`, `spin` and
`colorOverLife` scale, turn and tint it, and `sizeBySpeed` and `colorBySpeed` read a
curve or gradient over its speed. `inherit` gives it a share of the emitter's
velocity. `subEmitters` lists up to sixteen `{"system", "on": "birth" or "death", "inherit"}`. A
system another names emits only as its sub-emitter, and a sub-emitter that leads back
to the system naming it, itself included, is dropped. At a
particle's birth the child system runs its own rate and bursts from wherever that
particle goes until it dies; at its death the child bursts there once. The child
takes the parent particle's colour, size, rotation or lifetime where `inherit` names
them.

The `renderer` has `draw` (false runs the system unseen, still driving
sub-emitters), `mode` (`billboard`, `stretched` along the velocity by `length` sizes
plus `velocityScale` times the speed, `horizontal` facing up, `vertical` upright
facing the eye, or `mesh`, drawing `meshes[mesh]` shifted by `pivot` sizes), `align`,
the frame a billboard or mesh turns in (`view`, the camera's, turning a billboard
about its view axis alone; `world`; `local`, the emitter's; `facing`, toward the eye;
or `velocity`, along it, each of the last four turning by all three axes of the
rotation and spin), `maxSize`, the share of the view's height a quad may span at the
particle's distance, and `materials`, the default first. A material has `color` and
`emission` (sRGB, multiplied and added), `blend` (`opaque`, `cutout` at `cutoff`,
`alpha`, `add`, `premultiplied` or `multiply`), `uv` (`[scale x, scale y, offset x,
offset y]`) and `image`, a glTF image no texture names, packed as lossless WebP. A
mesh is `{"positions", "uvs", "indices"}`, flat arrays in glTF axes.

Files come from other players, so every field is checked for its type and range and a
missing one takes Unity's default. A number past 1e9 in magnitude counts as
missing. A file with more than 64 systems, a system naming no node of the file, a
material that is not an object or names an image the file lacks, or a mesh with more
than 4096 vertices, more than 12288 indices or an index past its vertices, draws no
particles; the avatar still loads. A curve holds at most 64 samples and a system 8
bursts. An avatar's systems hold at most 10000 slots together, and 375000 slots times
the indices each slot draws (six for a quad); past either, every system's `max` shrinks
by one common share. A particle lives at most 1e4 s, starts at most 1e4 m/s, and a
linear colour channel is at most 1e4; a negative rate or distance emits nothing, and a
burst gives at most its system's `max`.

## Menu

In `VRCHAT_expressions`, `particles` lists `{"system", "when"}` for each system a
switch controls: it runs while every `[target, value]` in `when` holds, as a switched
node shows, and stops with its particles cleared while any fails, as a GameObject
switched off hides them. A target whose effect is `{"particle": {"system", "field",
"end", "channel"}}` sets one value, its default being the file's own: `rate`,
`distance`, `speed`, `size`, `lifetime` or `gravity` set their curve's scale at the
`min` end, the `max` end or `both`; `color` sets a `channel` of the start colour's
`end`; `material` chooses `renderer.materials[round(value)]`; `tint` and `emission`
set a `channel` of every material's colour or emission (`emission` has no alpha).
A value for a colour's red, green or blue is sRGB and made linear; one for alpha is
linear. A `size` effect sets each of its axes. An effect naming a system the
file lacks does nothing.

## Running and drawing

The particles live in a world fixed in the tracking space at the avatar's own
scale. Each step, after constraints and springs settle the pose, the overlay places
every emitter and starts a system that became active or stops one that became
inactive. Sub-emitters then follow their parents' particles by the closed form below,
speed limit included but without the velocity curves and noise, so a death burst
takes its slots before new births do. Last, each running system owes the births of its
rate over time, of its rate over the distance its emitter moved and of the bursts its
cycle reached, a wrapping loop first firing what its old cycle left; what it owes is
capped at its `max`. Births take at most 500 µs of the step for the whole avatar, and
those past it wait for the next step. A birth takes the first free slot of the 16 past
the last one taken; where none is free the system's remaining births that step are
dropped, as a full system's are. A death burst gives at most its child's `max`. Each
system keeps its own clock, which starts again from zero once it passes an hour, its
living particles' birth times moved with it and every other slot cleared, so the birth
times the GPU reads keep their precision.

An emitter takes its node's position and its rotation from the node's normalised axes,
and none where an axis is scaled to nothing. A particle is written once, at birth, into its system's instance buffer: its start
position and velocity, size, rotation, birth time, lifetime, random seed, gravity and
colour. The vertex shader works out where it is at any age in closed form, so the CPU
spends nothing on a living particle. The life curves reach the shader as sixteen samples per row, the
low and high end of each range, with the velocity's and spin's integrals over life.

Each system with a living particle is one instanced draw in the avatar's render
pass, after the figure, unlit: opaque and cutout ones first, writing depth, then the
blended ones, testing depth without writing it, into the premultiplied image the
compositor takes. A multiply particle scales what is behind it toward its colour by
its alpha. A stretched one moving straight toward or away from the eye draws as a
billboard. The frame budget's density scaling counts them with the figure.

Synthetic tests cover the parser's defaults, clamps, refusals, colour conversion,
sub-emitter cycles and slot and index caps; burst, rate and distance emission counts,
a negative rate owing nothing, and huge values staying finite; births waiting on the
time budget; a loop's wrap; birth and death sub-emitters, a death burst held to its
child's `max`, and a followed parent's speed limit; an emitter scaled to nothing; a
system's clock starting again under a steady emitter; a switch clearing and replaying
a system; menu effects on rate, material, emission and start colour; the life-curve
rows; and an offscreen render of an opaque billboard, an additive billboard, a cutout
mesh, a multiply billboard over an opaque one and a billboard held to its maximum size.
