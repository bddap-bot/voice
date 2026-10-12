# Objects

An object is a binary glTF 2 file (`.glb`, `model/gltf-binary`) that a VR host draws in 3D. It may carry a program that moves its parts. Nothing in an object depends on this overlay, so any host that implements this page can run the same file.

## Scene

- Triangle meshes with a base colour factor and an optional PNG, JPEG or WebP base colour texture. `alphaMode` and `doubleSided` are honoured.
- Units are metres. An object larger than 25 cm on its largest side is shrunk to that; a smaller one keeps its size. Its size is taken from the nodes' own transforms, before any program runs.
- A primitive without normals gets smooth normals from its faces.
- At most 200,000 triangles, and 2048 texels on a texture's longer side.

## Program

The program is a WebAssembly core module stored in one of the file's buffer views and named by a top-level extension:

```json
"extensionsUsed": ["VOICE_program"],
"extensions": { "VOICE_program": { "bufferView": 7, "duration": 4.0 } }
```

The module imports nothing and exports `memory` and

```
tick(time: f32, x: f32, y: f32, z: f32, touching: i32) -> i32
```

- The host calls `tick` about 90 times a second on a thread of its own, never on the thread that draws. The drawing thread uses whatever `tick` last returned, so a slow program makes the object move less smoothly, never the scene.
- `time` is seconds since the object appeared, wrapping to zero at `duration` when `duration` is above zero.
- `touching` is 1 while the controller's touch point is inside the object; `x`, `y`, `z` are then that point in the object's own coordinates, the space its nodes are defined in.
- Returning 0 keeps the previous transforms. Any other value is the address in `memory` of one record per glTF node, in node order, each ten little-endian `f32`: translation x, y, z; rotation quaternion x, y, z, w; scale x, y, z. Each record replaces that node's local transform.
- Each `tick`, and the module's start function, may run 2,000,000 instructions; memory may grow to 16 MiB. A program that runs past either, traps, returns an address whose records do not fit in its memory, or returns a value that is not finite is stopped, and its object closes.

A program for a file of four nodes that sends nodes 1 to 3 around node 0 every four seconds, built with `rustc --edition 2021 --target wasm32-unknown-unknown --crate-type cdylib -O -C panic=abort orbit.rs`:

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
    let orbits = [(0.0, 0.0), (0.05, 1.6), (0.08, 1.0), (0.11, 0.6)];
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

The object appears just right of the puppet, facing the wearer. With the right controller's tip inside it, moving the tip around its vertical axis turns it, and a flick faster than 1.8 m/s throws it away. Untouched for three minutes, replaced by the next display item, over 8 ms of GPU time for 90 frames in a row, or when the session ends, it closes: its overlay, its GPU memory and its thread are all released.
