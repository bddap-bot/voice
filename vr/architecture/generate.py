"""Overlay objects from descriptions of model architectures: `python3 generate.py OUT description.json...` writes OUT/<name>.glb for each.

The description format, the node names and the live data the object's program reads are in ../ARCHITECTURE.md.
`glb`, `shapes` and `make` are wunderkammer-objects' tooling, at the rev ../objects.nix pins, on PYTHONPATH.
"""
import json
import math
import random
import re
import sys
import tempfile
from pathlib import Path
from typing import NamedTuple

import glb
import make
import shapes
from glb import Node

HERE = Path(__file__).resolve().parent


def linear(red: float, green: float, blue: float) -> tuple[float, float, float, float]:
    """A base colour factor is linear; these are picked as they should look."""
    return (red ** 2.2, green ** 2.2, blue ** 2.2, 1.0)


PALETTE = [linear(0.96, 0.5, 0.1), linear(0.2, 0.58, 0.96), linear(0.18, 0.74, 0.38), linear(0.97, 0.84, 0.16), linear(0.62, 0.38, 0.95), linear(0.93, 0.26, 0.24), linear(0.1, 0.78, 0.78), linear(0.96, 0.44, 0.72)]
STREAM = linear(0.5, 0.52, 0.58)
BLANK = linear(0.97, 0.97, 0.99)
TOKEN = linear(1.0, 0.97, 0.78)
BASE = linear(0.2, 0.21, 0.25)

SIZE = 0.25
SIDE = 2.4
PITCH = 3.0
BAR = 0.27
POLE = 0.28
RUNG = 0.26
FRAME_X = 0.9 + SIDE / 2
BANK_GAP = 1.3
FIRST = 2.4
FLOOR = 0.6
CELL = 0.875 * SIDE
STACK = 0.35
MODEL_GAP = 1.4
TIE = 0.22
FAINT = 0.3
NODES, VERTICES, TRIANGLES = 4096, 600_000, 200_000
MOST = 256
NARROWEST = 1 / 32
COMMANDS = ("map", "shared", "weigh", "show", "token")
NAME = re.compile(r"[a-z][a-z0-9_]*\Z")
NAMING = f"starts with a lower-case letter, goes on in lower-case letters, digits and _, and is no word live data uses ({', '.join(COMMANDS)}, all, off)"


class Map(NamedTuple):
    name: str
    table: list[int] | None
    passes: list[tuple[int, int]]


class Model(NamedTuple):
    name: str
    width: float
    streams: int
    modules: int
    chunks: int
    maps: list[Map]
    mix: list[list[float]]


def whole(value, least: int, what: str, most: int = MOST) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not least <= value <= most:
        raise ValueError(f"{what} must be a whole number from {least} to {most}, not {value!r}")
    return value


def named(name, what: str) -> str:
    if not isinstance(name, str) or not NAME.match(name) or name in (*COMMANDS, "all", "off"):
        raise ValueError(f"{what} {NAMING}, not {name!r}")
    return name


def read_map(rule, modules: int, chunks: int, what: str) -> Map:
    name = None
    if isinstance(rule, dict):
        rule = dict(rule)
        name = rule.pop("name", None)
        if len(rule) != 1:
            raise ValueError(f"{what} must hold one rule, not {sorted(rule)}")
        (kind, value), = rule.items()
    else:
        kind, value = rule, None
    passes: list[tuple[int, int]] = []
    if kind == "own" and value is None:
        if chunks != modules:
            raise ValueError(f"{what}: own needs a chunk for each of the {modules} modules, not {chunks}")
        table = list(range(modules))
    elif kind == "mod" and value is None:
        table = [module % chunks for module in range(modules)]
    elif kind == "pick" and value is None:
        table = None
    elif kind == "random" and isinstance(value, int) and not isinstance(value, bool):
        dice = random.Random(value)
        table = [dice.randrange(chunks) for _ in range(modules)]
    elif kind == "table" and isinstance(value, list):
        if len(value) != modules or any(isinstance(chunk, bool) or not isinstance(chunk, int) or not 0 <= chunk < chunks for chunk in value):
            raise ValueError(f"{what}: a table names one of the {chunks} chunks for each of the {modules} modules")
        table = value
    elif kind == "loop" and isinstance(value, dict) and set(value) == {"ends", "times"}:
        ends, times = whole(value["ends"], 0, f"{what}: ends"), whole(value["times"], 1, f"{what}: times")
        middle = chunks - 2 * ends
        if middle < 1 or modules != 2 * ends + times * middle:
            raise ValueError(f"{what}: {ends} unshared at each end and the rest of {chunks} chunks run {times} times is {2 * ends + times * max(middle, 0)} modules, not {modules}")
        table = [*range(ends), *(list(range(ends, ends + middle)) * times), *range(ends + middle, chunks)]
        passes = [(ends + turn * middle, ends + (turn + 1) * middle - 1) for turn in range(times)]
    else:
        raise ValueError(f"{what}: {rule!r} is not own, mod, pick, {{random: seed}}, {{table: [...]}} or {{loop: {{ends, times}}}}")
    return Map(named(kind if name is None else name, f"{what}: a map's name"), table, passes)


def read(description) -> list[Model]:
    if not isinstance(description, dict) or set(description) != {"models"} or not isinstance(description["models"], list) or not description["models"]:
        raise ValueError("a description is {\"models\": [...]} with at least one model")
    models = []
    for at, entry in enumerate(description["models"]):
        what = f"model {at}"
        if not isinstance(entry, dict):
            raise ValueError(f"{what} must be an object")
        unknown = set(entry) - {"name", "width", "streams", "modules", "chunks", "maps", "mix"}
        if unknown:
            raise ValueError(f"{what} has unknown {sorted(unknown)}")
        name = named(entry.get("name", "model"), f"{what}: a name")
        width = entry.get("width")
        if isinstance(width, bool) or not isinstance(width, (int, float)) or not 0 < width < 1e30:
            raise ValueError(f"{what}: width must be a positive number, not {width!r}")
        streams = entry.get("streams", 1)
        if isinstance(streams, bool) or not isinstance(streams, int) or streams not in (1, 2):
            raise ValueError(f"{what}: streams must be 1 or 2, not {streams!r}")
        modules = whole(entry.get("modules"), 1, f"{what}: modules")
        chunks = whole(entry.get("chunks", modules), 1, f"{what}: chunks")
        rules = entry.get("maps", ["own" if chunks == modules else "mod"])
        if not isinstance(rules, list) or not rules:
            raise ValueError(f"{what}: maps must list at least one map")
        maps = [read_map(rule, modules, chunks, f"{what}, map {n}") for n, rule in enumerate(rules)]
        if len({m.name for m in maps}) != len(maps):
            raise ValueError(f"{what}: two maps share a name; give one a \"name\"")
        if ("mix" in entry) != (streams == 2):
            raise ValueError(f"{what}: mix belongs to a model of two streams, and such a model needs it")
        mix = entry.get("mix", [[1.0, 0.0, 1.0, 0.0]] * modules)
        if not isinstance(mix, list) or len(mix) != modules or any(not isinstance(row, list) or len(row) != 4 or any(isinstance(v, bool) or not isinstance(v, (int, float)) or not 0 <= v <= 1 for v in row) for row in mix):
            raise ValueError(f"{what}: mix gives each of the {modules} modules four weights from 0 to 1: read from stream 1, read from stream 2, write to stream 1, write to stream 2")
        models.append(Model(name, float(width), streams, modules, chunks, maps, [[float(v) for v in row] for row in mix]))
    widest = max(model.width for model in models)
    for at, model in enumerate(models):
        if model.width < NARROWEST * widest:
            raise ValueError(f"model {at}: width {model.width:g} is under 1/{round(1 / NARROWEST)} of the widest model's {widest:g}; a square that small cannot be drawn beside it")
    if len(models) > 1:
        names = [model.name for model in models]
        if any("name" not in entry for entry in description["models"]) or len(set(names)) != len(names):
            raise ValueError("several models each need their own name")
    return models


def frame(side: float, bar: float, depth: float) -> glb.Mesh:
    return shapes.merge(
        shapes.box(side, bar, depth, at=(0, (side - bar) / 2, 0)),
        shapes.box(side, bar, depth, at=(0, -(side - bar) / 2, 0)),
        shapes.box(bar, side - 2 * bar, depth, at=((side - bar) / 2, 0, 0)),
        shapes.box(bar, side - 2 * bar, depth, at=(-(side - bar) / 2, 0, 0)),
    )


def bracket(height: float, reach: float, bar: float) -> glb.Mesh:
    return shapes.merge(
        shapes.box(bar, height, bar),
        shapes.box(reach, bar, bar, at=(reach / 2, (height - bar) / 2, 0)),
        shapes.box(reach, bar, bar, at=(reach / 2, -(height - bar) / 2, 0)),
    )


def number(value: float) -> str:
    return repr(float(value))


def numbers(values) -> str:
    return "[" + ", ".join(number(v) for v in values) + "]"


def text(value: str) -> str:
    return 'b"' + value + '"'


class Layout:
    """Where one model's parts stand, in centimetres from its stream's foot, before the scene is scaled to the size an object is shown at."""

    def __init__(self, model: Model, share: float):
        self.side = SIDE * share
        self.thin = min(1.0, self.side / (4 * BAR))
        self.pole = max(POLE * share, 0.1)
        self.poles = [0.0, 2 * FRAME_X][: model.streams]
        self.bank = FRAME_X + SIDE + BANK_GAP if model.streams == 1 else 2 * FRAME_X + 1.3 + SIDE / 2
        self.first = FIRST + max(model.chunks - model.modules, 0) * PITCH / 2
        self.top = self.first + (model.modules - 1) * PITCH + PITCH / 2 + 0.9
        middle = self.first + (model.modules - 1) * PITCH / 2
        self.plates = [middle + (chunk - (model.chunks - 1) / 2) * PITCH for chunk in range(model.chunks)]
        self.height = max(self.top, self.plates[-1] + self.side / 2)
        self.left = -1.3 if any(each.passes for each in model.maps) else -0.6
        self.right = self.bank + SIDE / 2 + 0.3
        self.cell = self.side * self.side / CELL

    def stacked(self, place: int, seam: float) -> str:
        return f"super::Cell {{ height: {number((STACK + self.cell * (place + 0.5)) * self.unit)}, wide: {number(CELL / self.side)}, tall: {number(seam * self.cell / self.side)} }}"


class Scene:
    def __init__(self, models: list[Model]):
        widest = max(model.width for model in models)
        layouts = [Layout(model, model.width / widest) for model in models]
        across = sum(layout.right - layout.left for layout in layouts) + MODEL_GAP * (len(models) - 1)
        self.unit = SIZE / max(across, max(layout.height for layout in layouts) + 0.25)
        self.roots: list[Node] = []
        self.emit: list = []
        self.kinds = {"MODELS": "super::Model", "MODULES": "super::Module", "CHUNKS": "super::Chunk", "FRAMES": "super::Frame", "TIES": "super::Tie", "PIPS": "super::Pip", "MAPS": "super::Map", "PASSES": "super::Pass", "QUIET": "usize", "TABLE": "usize"}
        self.tables: dict[str, list[str]] = {name: [] for name in self.kinds}
        self.modules = self.chunks = 0
        self.duration = float(max(model.modules for model in models) + 2)
        self.row = [across / 2 * self.unit, MODEL_GAP * self.unit]
        cursor = 0.0
        for at, (model, layout) in enumerate(zip(models, layouts)):
            layout.unit = self.unit
            self.model(at, model, layout, model.name + "." if len(models) > 1 else "", cursor - layout.left)
            cursor += layout.right - layout.left + MODEL_GAP

    def model(self, model_at: int, model: Model, layout: Layout, prefix: str, home: float) -> None:
        u = self.unit
        tables = self.tables
        side, pole, poles, bank, first = layout.side, layout.pole, layout.poles, layout.bank, layout.first
        bar = BAR * layout.thin
        rung = 0.25 * side
        lift = 0.45 if model.streams == 2 else 0.0
        rest = model.maps[0]
        used = [sorted({each.table[module] for each in model.maps if each.table is not None}) for module in range(model.modules)]
        picks = any(each.table is None for each in model.maps)
        suffix = lambda stream: "" if model.streams == 1 else str(stream + 1)
        colour = lambda chunk: PALETTE[chunk % len(PALETTE)]

        children: list[Node] = [Node(prefix + "base", shapes.box((layout.right - layout.left - 0.2) * u, 0.25 * u, 2.0 * u), BASE, translation=((layout.left + layout.right) / 2 * u, -0.125 * u, 0.0))]
        tokens, beads = [], []
        for stream, x in enumerate(poles):
            children.append(Node(prefix + "stream" + suffix(stream), shapes.cylinder(pole * u, layout.top * u, 20), STREAM, translation=(x * u, layout.top / 2 * u, 0.0)))
            tokens.append(Node(prefix + "token" + suffix(stream), shapes.sphere(0.55 * u, 10, 16), TOKEN, translation=(x * u, FLOOR * u, 0.0)))
            beads.append(Node(prefix + "bead" + suffix(stream), shapes.sphere(0.26 * u, 8, 12), TOKEN, scale=(0.0, 0.0, 0.0)))
        children += tokens + beads

        frame_mesh = frame(side * u, bar * u, 0.3 * u)
        tie_mesh = shapes.box(1.0, TIE * layout.thin * u, TIE * layout.thin * u, at=(0.5, 0, 0))
        pip_mesh = shapes.sphere(0.24 * u, 6, 10)
        modules, frames, ties, pips, quiet = [], [], [], [], []
        for module in range(model.modules):
            y = first + module * PITCH
            parts: list[Node] = []
            for variant in used[module] + ([None] if picks else []):
                shown = rest.table[module] == variant if rest.table is not None else variant is None
                part = Node(f"{prefix}module{module}.{'any' if variant is None else f'chunk{variant}'}", frame_mesh, BLANK if variant is None else colour(variant), scale=(1.0, 1.0, 1.0) if shown else (0.0, 0.0, 0.0))
                parts.append(part)
                frames.append((part, module, variant))
            for stream, x in enumerate(poles):
                sign = -1 if stream == 0 else 1
                inner, outer = sign * side / 2, x - FRAME_X - sign * pole * 0.5
                for kind, weight, height in (("read", model.mix[module][stream], -rung), ("write", model.mix[module][2 + stream], rung)):
                    thick = RUNG * layout.thin * (0.15 + 0.85 * weight) * u
                    parts.append(Node(f"{prefix}module{module}.{kind}{suffix(stream)}", shapes.box(abs(outer - inner) * u, thick, thick), STREAM, translation=((inner + outer) / 2 * u, height * u, 0.0)))
                parts.append(Node(f"{prefix}module{module}.add{suffix(stream)}", shapes.sphere((pole + 0.13) * u, 8, 12), STREAM, translation=((x - FRAME_X) * u, rung * u, 0.0)))
            quiet += [part for part in parts if part.mesh is not frame_mesh]
            modules.append((Node(f"{prefix}module{module}", translation=(FRAME_X * u, y * u, 0.0), children=parts), y))
            for chunk in range(model.chunks) if picks else used[module]:
                start, end = (FRAME_X + side / 2, y), (bank - side / 2, layout.plates[chunk])
                weight = (1.0 if rest.table[module] == chunk else 0.0) if rest.table is not None else FAINT
                tie = Node(f"{prefix}tie{module}.{chunk}", tie_mesh, colour(chunk), translation=(start[0] * u, start[1] * u, lift * u), rotation=shapes.quaternion((0, 0, 1), math.atan2(end[1] - start[1], end[0] - start[0])), scale=(math.dist(start, end) * u, weight, weight))
                ties.append((tie, module, chunk))
                pips += [(Node(f"{prefix}pip{module}.{chunk}{suffix(stream)}", pip_mesh, colour(chunk), scale=(0.0, 0.0, 0.0)), module, chunk, stream) for stream in range(model.streams)]
        children += [node for node, _ in modules]

        hole = (side - 2 * bar) * 0.92
        plates = [Node(f"{prefix}chunk{chunk}", shapes.box(side * u, side * u, 0.5 * u), colour(chunk), translation=(bank * u, y * u, 0.0)) for chunk, y in enumerate(layout.plates)]
        visitors = [Node(f"{prefix}visit{chunk}", shapes.box(hole * u, hole * u, 0.2 * u), colour(chunk), translation=(bank * u, y * u, 0.0), scale=(0.0, 0.0, 0.0)) for chunk, y in enumerate(layout.plates)]
        children.append(Node(prefix + "bank", children=plates))
        children += visitors + [tie for tie, _, _ in ties] + [pip for pip, _, _, _ in pips]

        map_at = len(tables["MAPS"])
        passes = []
        for at, each in enumerate(model.maps):
            table = "None" if each.table is None else f"Some({len(tables['TABLE'])})"
            tables["MAPS"].append(f"super::Map {{ name: {text(each.name)}, table: {table} }}")
            tables["TABLE"] += [str(chunk) for chunk in each.table or []]
            for turn, (low, high) in enumerate(each.passes):
                # A bracket out of use keeps a sliver of size: the host finds where to point from the rest transform, and a scale of nothing has no inverse.
                size = 1.0 if at == 0 else 1e-3
                mark = Node(f"{prefix}pass{turn}" if at == 0 else f"{prefix}{each.name}.pass{turn}", bracket(((high - low) * PITCH + side + 0.3) * u, 0.5 * u, 0.26 * u), BLANK, translation=(-0.95 * u, (first + (low + high) / 2 * PITCH) * u, 0.0), scale=(size, size, size))
                passes.append((mark, map_at + at))
        children += [mark for mark, _ in passes]

        root = Node(prefix.rstrip(".") or "model", translation=(home * u, 0.0, 0.0), children=children)
        self.roots.append(root)
        module_at, chunk_at = self.modules, self.chunks
        self.modules += model.modules
        self.chunks += model.chunks

        def emit(index) -> None:
            pair = lambda nodes: "[" + ", ".join(str(index(node)) for node in (nodes * 2)[:2]) + "]"
            tables["MODELS"].append(
                f"super::Model {{ name: {text(model.name)}, root: {index(root)}, left: {number(layout.left * u)}, span: {number((layout.right - layout.left) * u)}, "
                f"modules: [{module_at}, {module_at + model.modules}], chunks: [{chunk_at}, {chunk_at + model.chunks}], maps: [{map_at}, {map_at + len(model.maps)}], "
                f"streams: {model.streams}, poles: {numbers([x * u for x in (poles * 2)[:2]])}, tokens: {pair(tokens)}, beads: {pair(beads)}, "
                f"side: {number(side * u)}, pitch: {number(PITCH * u)}, rung: {number(rung * u)}, floor: {number(FLOOR * u)}, top: {number((layout.top - 0.5) * u)} }}"
            )
            for module, (node, y) in enumerate(modules):
                tables["MODULES"].append(f"super::Module {{ node: {index(node)}, at: {numbers([FRAME_X * u, y * u])}, cell: {layout.stacked(module, 1.0)}, read: {numbers(model.mix[module][:2])}, write: {numbers(model.mix[module][2:])} }}")
            for chunk, (plate, visitor) in enumerate(zip(plates, visitors)):
                tables["CHUNKS"].append(f"super::Chunk {{ node: {index(plate)}, visitor: {index(visitor)}, at: {numbers([bank * u, layout.plates[chunk] * u])}, cell: {layout.stacked(chunk, 0.94)} }}")
            for part, module, variant in frames:
                tables["FRAMES"].append(f"super::Frame {{ node: {index(part)}, model: {model_at}, module: {module}, chunk: {'None' if variant is None else f'Some({variant})'} }}")
            for tie, module, chunk in ties:
                tables["TIES"].append(f"super::Tie {{ node: {index(tie)}, model: {model_at}, module: {module}, chunk: {chunk} }}")
            for pip, module, chunk, stream in pips:
                tables["PIPS"].append(f"super::Pip {{ node: {index(pip)}, model: {model_at}, module: {module}, chunk: {chunk}, stream: {stream} }}")
            for mark, at in passes:
                tables["PASSES"].append(f"super::Pass {{ node: {index(mark)}, model: {model_at}, map: {at} }}")
            tables["QUIET"] += [str(index(node)) for node in quiet]

        self.emit.append(emit)

    def header(self) -> str:
        order = glb.flatten(self.roots)
        drawn = [node.mesh for node in order if node.mesh is not None]
        size = (len(order), sum(len(mesh.positions) for mesh in drawn), sum(len(mesh.indices) for mesh in drawn))
        if any(count > limit for count, limit in zip(size, (NODES, VERTICES, TRIANGLES))):
            raise ValueError(f"the object would have {size[0]} nodes, {size[1]} vertices and {size[2]} triangles, over the {NODES}, {VERTICES} or {TRIANGLES} a host draws; a pick map costs a tie and a pip for every module and chunk")
        places = {id(node): at for at, node in enumerate(order)}
        for emit in self.emit:
            emit(lambda node: places[id(node)])
        lines = [
            f"pub const NODES: usize = {len(order)};",
            f"pub const DURATION: f32 = {number(self.duration)};",
            f"pub const ROW: [f32; 2] = {numbers(self.row)};",
            f"pub const REACH: f32 = {number(0.75 * SIDE * self.unit)};",
            f"pub const ORBIT: f32 = {number(1.25 * self.unit)};",
            f"pub const FAINT: f32 = {number(FAINT)};",
            f"pub const COMMANDS: [&[u8]; {len(COMMANDS)}] = [{', '.join(text(word) for word in COMMANDS)}];",
        ]
        lines += [f"pub const {name}: [{kind}; {len(self.tables[name])}] = [{', '.join(self.tables[name])}];" for name, kind in self.kinds.items()]
        lines.append("pub const REST: [[f32; 10]; NODES] = [" + ", ".join(numbers(node.rest()) for node in order) + "];")
        return "\n".join(lines) + "\n"


def generate(description) -> bytes:
    scene = Scene(read(description))
    header = scene.header()
    with tempfile.TemporaryDirectory() as scratch:
        wasm = make.program(HERE / "program.rs", header, Path(scratch))
    return glb.build(scene.roots, wasm, scene.duration)


def main() -> None:
    if len(sys.argv) < 3:
        sys.exit("usage: generate.py OUT description.json...")
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for source in map(Path, sys.argv[2:]):
        try:
            data = generate(json.loads(source.read_text()))
        except (ValueError, OSError) as error:
            sys.exit(f"{source}: {error}")
        path = out / f"{source.stem}.glb"
        path.write_bytes(data)
        print(f"{path}: {len(data)} bytes")


if __name__ == "__main__":
    main()
