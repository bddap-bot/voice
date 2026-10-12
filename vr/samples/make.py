"""Builds every sample object, or those named, into OUT: `python3 make.py OUT [name...]`.

Each object is `objects/<name>.py`, whose `scene()` returns its root nodes, its program's duration and the numbers the program shares with the geometry, beside an optional `objects/<name>.rs` program.
The program is compiled as one no_std crate: the scene's generated constants as `mod scene`, then `common.rs`, then the object's own source.
"""
import importlib.util
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import glb

HERE = Path(__file__).resolve().parent
PRELUDE = "#![no_std]\n#[panic_handler]\nfn panic(_: &core::panic::PanicInfo) -> ! {\n    loop {}\n}\n"


def program(name: str, header: str, scratch: Path) -> bytes:
    source = scratch / f"{name}.rs"
    source.write_text(PRELUDE + "#[allow(dead_code)]\nmod scene {\n" + header + "}\n" + (HERE / "common.rs").read_text() + (HERE / "objects" / f"{name}.rs").read_text())
    wasm = scratch / f"{name}.wasm"
    subprocess.run(["rustc", "--edition", "2021", "--target", "wasm32-unknown-unknown", "--crate-type", "cdylib", "-O", "-C", "panic=abort", "-D", "warnings", "-o", str(wasm), str(source)], check=True)
    return wasm.read_bytes()


def make(name: str, out: Path) -> Path:
    spec = importlib.util.spec_from_file_location(name, HERE / "objects" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    roots, duration, constants = module.scene()
    wasm = None
    if (HERE / "objects" / f"{name}.rs").exists():
        with tempfile.TemporaryDirectory() as scratch:
            wasm = program(name, glb.rust_header(roots, duration, constants), Path(scratch))
    path = out / f"{name}.glb"
    path.write_bytes(glb.build(roots, wasm, duration))
    return path


def main() -> None:
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    names = sys.argv[2:] or sorted(path.stem for path in (HERE / "objects").glob("*.py"))
    for name in names:
        path = make(name, out)
        print(f"{path}: {os.path.getsize(path)} bytes")


if __name__ == "__main__":
    main()
