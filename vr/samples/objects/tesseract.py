"""A tesseract turning in four dimensions, projected into three: two cubes and the edges along w between them."""
import numpy as np

import shapes
from glb import Node

# The 4D eye's distance along w from the centre, and the drawn size of one unit after projection.
EYE_W = 3.0
UNIT = 0.11
DURATION = 12.0
VERTEX = 0.0055
EDGE = 0.0022


def corner(v: int) -> np.ndarray:
    return np.array([1.0 if v >> bit & 1 else -1.0 for bit in range(4)])


def project(p: np.ndarray) -> tuple[np.ndarray, float]:
    k = 1.0 / (EYE_W - p[3])
    return p[:3] * k * UNIT * (EYE_W - 1.0), k * (EYE_W - 1.0)


def edges() -> list[tuple[int, int]]:
    return [(v, v | 1 << bit) for v in range(16) for bit in range(4) if not v >> bit & 1]


def scene():
    rod = shapes.cylinder(EDGE, 1.0, 10)
    ball = shapes.sphere(VERTEX, 10, 14)
    groups = {"cube_w_minus": [], "cube_w_plus": [], "w_edges": []}
    colours = {"cube_w_minus": (0.3, 0.6, 1.0, 1), "cube_w_plus": (1.0, 0.55, 0.2, 1), "w_edges": (0.75, 0.45, 0.95, 1)}
    for v in range(16):
        at, k = project(corner(v))
        group = "cube_w_plus" if v >> 3 & 1 else "cube_w_minus"
        groups[group].append(Node(f"v{v}", ball, (0.95, 0.95, 0.95, 1), translation=tuple(at), scale=(k, k, k)))
    for a, b in edges():
        (pa, _), (pb, _) = project(corner(a)), project(corner(b))
        direction = pb - pa
        length = float(np.linalg.norm(direction))
        axis = np.cross([0, 1, 0], direction / length)
        angle = float(np.arccos(np.clip(direction[1] / length, -1, 1)))
        rotation = shapes.quaternion(axis, angle) if np.linalg.norm(axis) > 1e-6 else (0.0, 0.0, 0.0, 1.0)
        group = "w_edges" if a ^ b == 8 else ("cube_w_plus" if a >> 3 & 1 else "cube_w_minus")
        groups[group].append(Node(f"e{a}_{b}", rod, colours[group], translation=tuple((pa + pb) / 2), rotation=rotation, scale=(1.0, length, 1.0)))
    view = shapes.quaternion((1, 0, 0), np.radians(22))
    view = tuple(float(x) for x in view)
    roots = [Node("tesseract", rotation=view, children=[Node(name, children=nodes) for name, nodes in groups.items()])]
    return roots, DURATION, {"EYE_W": EYE_W, "UNIT": UNIT}
