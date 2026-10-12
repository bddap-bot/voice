"""A machine's workload at a glance, driven by live data: a GPU dial and fan, a memory column, a job queue and the jobs running."""
import numpy as np

import shapes
from glb import Node

QUEUE_SLOTS = 20
QUEUE_COLUMNS = 2
BLOCK = (0.026, 0.0075, 0.026)
WORKERS = 8
DIAL = 0.045
FAN = 0.03
COLUMN = 0.09
DARK = (0.16, 0.17, 0.2, 1)


def dial_face():
    angles = np.linspace(0, np.pi, 33)
    outline = [[0.0, 0.0]] + [[DIAL * np.cos(a), -DIAL * np.sin(a)] for a in angles]
    face = shapes.rotate(shapes.extrude(outline[1:] + [[-DIAL, 0.004], [DIAL, 0.004]], 0.004), (1, 0, 0), np.pi / 2)
    return face


def ticks():
    marks = []
    for i in range(11):
        a = np.pi * i / 10
        length = 0.008 if i % 5 == 0 else 0.004
        mark = shapes.box(0.0012, length, 0.001)
        mark = shapes.rotate(shapes.translate(mark, (0, DIAL - 0.003 - length / 2, 0)), (0, 0, 1), np.pi / 2 - a)
        marks.append(shapes.translate(mark, (0, 0, 0.0025)))
    return shapes.merge(*marks)


def bands():
    """Green to red along the dial's rim, so the needle's angle reads as load without numbers."""
    pieces = []
    for i, colour in enumerate([(0.25, 0.8, 0.4, 1), (0.95, 0.75, 0.2, 1), (0.95, 0.3, 0.25, 1)]):
        start, end = np.pi * (1 - (i + 1) / 3), np.pi * (1 - i / 3)
        a = np.linspace(start, end, 12)
        outline = [[(DIAL - 0.002) * np.cos(x), -(DIAL - 0.002) * np.sin(x)] for x in a] + [[(DIAL - 0.007) * np.cos(x), -(DIAL - 0.007) * np.sin(x)] for x in a[::-1]]
        pieces.append(Node(f"band{i}", shapes.translate(shapes.rotate(shapes.extrude(outline, 0.001), (1, 0, 0), np.pi / 2), (0, 0, 0.0028)), colour))
    return pieces


def scene():
    base = Node("base", shapes.box(0.25, 0.008, 0.075), DARK, translation=(0.0, -0.05, 0.0))
    needle = Node("gpu_needle", shapes.merge(shapes.box(0.0035, DIAL - 0.006, 0.002, at=(0, (DIAL - 0.006) / 2, 0)), shapes.rotate(shapes.cylinder(0.005, 0.005, 16), (1, 0, 0), np.pi / 2)), (1.0, 0.35, 0.25, 1), translation=(0.0, 0.0, 0.005))
    dial = Node("gpu_dial", dial_face(), (0.1, 0.11, 0.13, 1), translation=(-0.075, -0.035, 0.0), children=[*bands(), Node("ticks", ticks(), (0.9, 0.9, 0.9, 1)), needle])
    blades = []
    for i in range(5):
        blade = shapes.rotate(shapes.box(0.013, FAN - 0.007, 0.0015, at=(0, (FAN - 0.007) / 2 + 0.005, 0)), (0, 1, 0), 0.6)
        blades.append(shapes.rotate(blade, (0, 0, 1), 2 * np.pi * i / 5))
    rotor = Node("fan_rotor", shapes.merge(*blades, shapes.rotate(shapes.cylinder(0.008, 0.007, 20), (1, 0, 0), np.pi / 2)), (0.35, 0.75, 0.55, 1))
    workers = [Node(f"worker{i}", shapes.sphere(0.0045, 8, 12), (0.3, 0.85, 1.0, 1), translation=(FAN + 0.008, 0.0, 0.0), scale=(0.0, 0.0, 0.0)) for i in range(WORKERS)]
    fan = Node("fan", shapes.rotate(shapes.torus(FAN, 0.003, 40, 8), (1, 0, 0), np.pi / 2), DARK, translation=(0.015, -0.005, 0.0), children=[rotor, Node("workers", children=workers)])
    column = Node("vram", shapes.cylinder(0.011, COLUMN, 20, at=(0, COLUMN / 2, 0)), (0.7, 0.8, 0.95, 0.25), translation=(0.07, -0.046, 0.0), children=[Node("vram_fill", shapes.cylinder(0.009, 1.0, 20, at=(0, 0.5, 0)), (0.55, 0.4, 0.95, 1), scale=(1.0, 0.5 * COLUMN, 1.0))])
    blocks = [Node(f"job{i}", shapes.box(*BLOCK), (1.0, 0.6, 0.15, 1), translation=slot(i), scale=(0.0, 0.0, 0.0)) for i in range(QUEUE_SLOTS)]
    queue = Node("queue", shapes.box(0.06, 0.003, 0.032), DARK, translation=(0.11, -0.044, 0.0), children=blocks)
    constants = {"COLUMN": COLUMN, "FAN_ORBIT": FAN + 0.008, "BLOCK_HEIGHT": BLOCK[1] + 0.001, "BLOCK_PITCH": BLOCK[0] + 0.003, "QUEUE_SLOTS": QUEUE_SLOTS, "QUEUE_COLUMNS": QUEUE_COLUMNS, "WORKER_COUNT": WORKERS}
    return [base, dial, fan, column, queue], 0.0, constants


def slot(i: int) -> tuple[float, float, float]:
    column, row = i % QUEUE_COLUMNS, i // QUEUE_COLUMNS
    return ((column - (QUEUE_COLUMNS - 1) / 2) * (BLOCK[0] + 0.003), 0.0055 + row * (BLOCK[1] + 0.001), 0.0)
