# Objects

An object is a binary glTF 2 file (`.glb`, `model/gltf-binary`) that a VR host draws in 3D, optionally with a program that moves its parts. Any VR host that implements this format can run the same file; a viewer that ignores the program shows the static scene.

## Scene

- The host draws every node's triangle meshes with their base colour factor and optional PNG, JPEG or WebP base colour texture, honouring `alphaMode` and `doubleSided`. It ignores skins, morph targets, glTF animations, cameras and lights.
- Units are metres. The object's size is the box around its vertices under the nodes' own transforms, before any program runs. An object whose box is larger than 25 cm on its longest side is shrunk to that; a smaller one keeps its size.
- A primitive without normals gets smooth normals from its faces.
- The host refuses a file with more than 4096 nodes, a node more than 64 levels deep, more than 600,000 vertices or 200,000 triangles drawn (a mesh is counted once for every node that draws it), more than 8 textures, or a texture over 2048 texels on its longer side.

## Program

The program is a WebAssembly core module stored in one of the file's buffer views and named by a top-level extension. List it in `extensionsUsed`, never `extensionsRequired`, so a viewer without programs still shows the scene:

```json
"extensionsUsed": ["VOICE_program"],
"extensions": { "VOICE_program": { "bufferView": 7, "duration": 4.0 } }
```

`duration` is optional and defaults to 0. The module imports nothing and exports `memory` and

```
tick(time: f32, x: f32, y: f32, z: f32, touching: i32) -> i32
```

- The host calls `tick` about 90 times a second on a thread of its own, never on the thread that draws. The drawing thread uses whatever `tick` last returned, so a slow program makes its object move less smoothly, never the rest of the scene. Until `tick` first returns records, the nodes keep their own transforms.
- `time` is in seconds. When `duration` is above zero it wraps to zero at `duration`.
- `touching` is 1 while the user's pointer is inside the object's bounding sphere, the sphere around the box above. `x`, `y`, `z` are then the pointer in the object's own coordinates, the space its nodes are defined in, before any shrinking; they are 0 when `touching` is 0.
- Returning 0 keeps the previous transforms. Any other value is the address in `memory` of one record per glTF node, in node order, each ten little-endian `f32`: translation x, y, z; rotation quaternion x, y, z, w; scale x, y, z. Each record replaces that node's whole local transform.
- Live data: a module may also export `data(length: i32) -> i32`. When the host receives new data for the object, from a source it defines, it calls `data` with the length in bytes and writes the bytes at the returned address, or drops them if it returns 0. The next `tick` sees them. Only the newest waiting update is delivered.
- This host lets each `tick` and `data` call, and the module's start function, run 2,000,000 instructions as its interpreter counts them, and lets memory grow to 16 MiB. A program that runs past either, traps, returns an address whose records or data do not fit in its memory, or returns a record holding a value that is not finite is stopped, and its object is removed from view.

A program for a file of four nodes that circles nodes 1, 2 and 3 around the object's origin, 3, 2 and 1 times every four seconds, so the 4-second `duration` above loops without a jump. Build it with `rustc --edition 2021 --target wasm32-unknown-unknown --crate-type cdylib -O -C panic=abort orbit.rs`:

```rust
#![no_std]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

static mut NODES: [[f32; 10]; 4] = [[0.0; 10]; 4];

fn sin(x: f32) -> f32 {
    let wrapped = (x + core::f32::consts::PI) % core::f32::consts::TAU;
    let x = wrapped + if wrapped < 0.0 { core::f32::consts::TAU } else { 0.0 } - core::f32::consts::PI;
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0))))
}

#[no_mangle]
pub extern "C" fn tick(time: f32, _x: f32, _y: f32, _z: f32, _touching: i32) -> i32 {
    let orbits = [(0.0, 0.0), (0.05, 3.0), (0.08, 2.0), (0.11, 1.0)];
    unsafe {
        for (node, (radius, turns)) in orbits.into_iter().enumerate() {
            let angle = time / 4.0 * turns * core::f32::consts::TAU;
            NODES[node] = [radius * sin(angle + core::f32::consts::FRAC_PI_2), 0.0, radius * sin(angle), 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        }
        core::ptr::addr_of!(NODES) as i32
    }
}
```

## In this overlay

A hub reply's display item may carry an object. The overlay draws it just right of the puppet, facing the wearer, until the next display item that carries another object, or a card that neither points nor carries data. Its pointer is the right controller's tip, by position alone; its radius below is the bounding sphere's, at least 6 cm.

- Moving the tip around the object's vertical axis while inside turns it. A flick of the tip faster than 1.8 m/s throws it away.
- With the tip closer to the centre than half the radius, the object's parts (its top-level nodes, or a lone root's children) spread away from its centre, fully apart at a fifth of the radius, and close again as the tip withdraws.
- While the tip is inside, `time` stands still, and sliding the tip up or down by the sphere's diameter moves it through one whole `duration`.
- A reply may name a node of the shown object: while the puppet speaks that reply, it points its right arm at the centre of that node and its descendants, wherever the program, the spread and the turn have put it. A reply may also carry live data, which the overlay delivers to the shown object's `data`; neither replaces the object.
- The object is removed when untouched for three minutes, replaced, when the session ends, or when its own rendering averages over 4 ms a frame or takes over 40 ms in one frame after its first 45; removing it closes its overlay, frees its GPU memory and stops and joins its thread.
