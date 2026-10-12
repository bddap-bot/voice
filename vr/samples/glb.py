"""One binary glTF 2 file from a tree of nodes, optionally carrying a VOICE_program (vr/OBJECTS.md)."""
import json
import struct
from dataclasses import dataclass, field

import numpy as np


@dataclass
class Mesh:
    positions: np.ndarray
    normals: np.ndarray
    indices: np.ndarray


@dataclass
class Node:
    name: str
    mesh: Mesh | None = None
    color: tuple[float, float, float, float] = (0.8, 0.8, 0.8, 1.0)
    translation: tuple[float, float, float] = (0.0, 0.0, 0.0)
    rotation: tuple[float, float, float, float] = (0.0, 0.0, 0.0, 1.0)
    scale: tuple[float, float, float] = (1.0, 1.0, 1.0)
    children: list["Node"] = field(default_factory=list)
    double_sided: bool = False

    def rest(self) -> list[float]:
        return [*self.translation, *self.rotation, *self.scale]


def flatten(roots: list[Node]) -> list[Node]:
    """Every node, depth first, in the order the program's records follow."""
    order: list[Node] = []

    def visit(node: Node) -> None:
        order.append(node)
        for child in node.children:
            visit(child)

    for root in roots:
        visit(root)
    return order


def build(roots: list[Node], program: bytes | None = None, duration: float = 0.0) -> bytes:
    blob = bytearray()
    views: list[dict] = []
    accessors: list[dict] = []
    meshes: list[dict] = []
    materials: list[dict] = []
    order = flatten(roots)
    index = {id(node): at for at, node in enumerate(order)}

    def view(data: bytes, target: int | None = None) -> int:
        while len(blob) % 4:
            blob.append(0)
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(data), **({"target": target} if target else {})})
        blob.extend(data)
        return len(views) - 1

    def accessor(data: np.ndarray, kind: str, target: int, bounds: bool = False) -> int:
        entry = {"bufferView": view(data.tobytes(), target), "componentType": 5126 if data.dtype == np.float32 else 5125, "count": len(data), "type": kind}
        if bounds:
            entry |= {"min": data.min(0).tolist(), "max": data.max(0).tolist()}
        accessors.append(entry)
        return len(accessors) - 1

    nodes = []
    for node in order:
        entry: dict = {"name": node.name}
        if node.mesh is not None:
            materials.append({"pbrMetallicRoughness": {"baseColorFactor": list(node.color), "metallicFactor": 0.0}, **({"alphaMode": "BLEND"} if node.color[3] < 1 else {}), **({"doubleSided": True} if node.double_sided else {})})
            attributes = {"POSITION": accessor(node.mesh.positions.astype(np.float32), "VEC3", 34962, True), "NORMAL": accessor(node.mesh.normals.astype(np.float32), "VEC3", 34962)}
            meshes.append({"primitives": [{"attributes": attributes, "indices": accessor(node.mesh.indices.astype(np.uint32).ravel(), "SCALAR", 34963), "material": len(materials) - 1}]})
            entry["mesh"] = len(meshes) - 1
        if node.translation != (0.0, 0.0, 0.0):
            entry["translation"] = list(node.translation)
        if node.rotation != (0.0, 0.0, 0.0, 1.0):
            entry["rotation"] = list(node.rotation)
        if node.scale != (1.0, 1.0, 1.0):
            entry["scale"] = list(node.scale)
        if node.children:
            entry["children"] = [index[id(child)] for child in node.children]
        nodes.append(entry)
    gltf: dict = {"asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [index[id(root)] for root in roots]}], "nodes": nodes, "meshes": meshes, "materials": materials, "accessors": accessors}
    if program is not None:
        gltf["extensionsUsed"] = ["VOICE_program"]
        gltf["extensions"] = {"VOICE_program": {"bufferView": view(program), "duration": duration}}
    while len(blob) % 4:
        blob.append(0)
    gltf["bufferViews"] = views
    gltf["buffers"] = [{"byteLength": len(blob)}]
    text = json.dumps(gltf, separators=(",", ":")).encode()
    text += b" " * (-len(text) % 4)
    return struct.pack("<III", 0x46546C67, 2, 28 + len(text) + len(blob)) + struct.pack("<II", len(text), 0x4E4F534A) + text + struct.pack("<II", len(blob), 0x004E4942) + bytes(blob)


def rust_header(roots: list[Node], duration: float, constants: dict[str, float]) -> str:
    """The node indices, rest transforms and the scene's own numbers, so the program and the geometry share one source."""
    order = flatten(roots)
    lines = [f"pub const NODES: usize = {len(order)};", f"pub const DURATION: f32 = {duration!r};"]
    for at, node in enumerate(order):
        lines.append(f"#[allow(dead_code)] pub const {node.name.upper()}: usize = {at};")
    for name, value in constants.items():
        lines.append(f"#[allow(dead_code)] pub const {name}: f32 = {float(value)!r};")
    lines.append(f"pub const NAMES: [&str; NODES] = [{', '.join(repr(node.name).replace(chr(39), chr(34)) for node in order)}];")
    rest = ", ".join("[" + ", ".join(f"{float(v)!r}" for v in node.rest()) + "]" for node in order)
    lines.append(f"pub const REST: [[f32; 10]; NODES] = [{rest}];")
    return "\n".join(lines) + "\n"
