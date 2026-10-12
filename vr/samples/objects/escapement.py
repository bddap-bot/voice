"""A seconds-beating clock movement opened up: a Graham deadbeat escapement whose 30-tooth wheel carries the seconds hand, its anchor tied by the crutch to a pendulum of one-second half-period; the program keeps the time of day."""
import numpy as np

import shapes
from glb import Mesh, Node

TEETH = 30
PITCH = 360.0 / TEETH
TIP = 0.042
TOOTH_HEIGHT = 0.195 * TIP
LEAD = 4.0
BACK = 8.5
LAND = 0.6
CENTRES = TIP * np.sqrt(2)
DROP = 1.2
# Wheel angles of the tooth tip resting on each pallet's inner and outer arc, from the wheel's line of centres: half a pitch per beat, 7½ teeth embraced.
INNER_AT = (7 * PITCH + DROP) / 2
OUTER_AT = INNER_AT + PITCH / 2 - DROP
AMPLITUDE = 6.0
LIFT = 3.0
UNLOCK = 1.2
DROP_TIME = 0.035
PALLET_LENGTH = 17.0

WHEEL_Y = CENTRES
FORK_Y = -0.05
BOB_Y = -0.118
BOB_RADIUS = 0.018
DIAL_AT = (0.052, WHEEL_Y + 0.026)
DIAL_RADIUS = 0.0135
RING = (0.0232, 0.0292)

DIAL_Z = 0.011
CRUTCH_Z = -0.012
PENDULUM_Z = -0.019
PLATE_Z = -0.03

BRASS = (0.95, 0.74, 0.3, 1)
STEEL = (0.78, 0.8, 0.84, 1)
BLUED = (0.3, 0.48, 0.88, 1)
RUBY = (0.95, 0.1, 0.25, 1)
CREAM = (0.96, 0.93, 0.84, 1)
INK = (0.1, 0.11, 0.16, 1)
HAND = (0.1, 0.14, 0.32, 1)
PLATE = (0.3, 0.23, 0.18, 1)
BOB = (0.86, 0.6, 0.26, 1)


def slab(outline, depth: float, z: float = 0.0) -> Mesh:
    """A star-shaped polygon in the xy plane, `depth` thick about z."""
    outline = np.asarray(outline, float)
    flat = np.stack([outline[:, 0], -outline[:, 1]], 1)
    if np.sum(flat[:, 0] * np.roll(flat[:, 1], -1) - np.roll(flat[:, 0], -1) * flat[:, 1]) < 0:
        flat = flat[::-1]
    return shapes.translate(shapes.rotate(shapes.extrude(flat, depth), (1, 0, 0), np.pi / 2), (0, 0, z))


def disc(radius: float, depth: float, at=(0, 0, 0), sections: int = 32) -> Mesh:
    return shapes.translate(shapes.rotate(shapes.cylinder(radius, depth, sections), (1, 0, 0), np.pi / 2), at)


def annulus(outer: float, inner: float, depth: float, sections: int = 64, at=(0, 0, 0)) -> Mesh:
    square = [(outer, depth / 2), (inner, depth / 2), (inner, -depth / 2), (outer, -depth / 2)]
    positions, normals, indices = [], [], []
    for side in range(4):
        (r0, z0), (r1, z1) = square[side], square[(side + 1) % 4]
        normal = np.array([z1 - z0, -(r1 - r0)])
        normal /= np.linalg.norm(normal)
        base = len(positions)
        for angle in np.linspace(0, 2 * np.pi, sections + 1):
            c, s = np.cos(angle), np.sin(angle)
            positions += [[r0 * c, r0 * s, z0], [r1 * c, r1 * s, z1]]
            normals += [[normal[0] * c, normal[0] * s, normal[1]]] * 2
        for i in range(sections):
            p = base + 2 * i
            indices += [[p, p + 2, p + 3], [p, p + 3, p + 1]]
    return shapes.translate(shapes.outward(Mesh(np.array(positions), np.array(normals), np.array(indices))), at)


def polar(radius: float, from_top: float, centre=(0.0, 0.0)) -> np.ndarray:
    """A point `from_top` degrees clockwise from straight up."""
    a = np.radians(from_top)
    return np.array([centre[0] + radius * np.sin(a), centre[1] + radius * np.cos(a)])


def turned(point, degrees: float) -> np.ndarray:
    """Counter-clockwise about the origin, as the anchor turns for a positive angle."""
    a = np.radians(degrees)
    return np.array([point[0] * np.cos(a) - point[1] * np.sin(a), point[0] * np.sin(a) + point[1] * np.cos(a)])


def bar(a, b, width: float, depth: float, z: float = 0.0) -> Mesh:
    a, b = np.asarray(a, float), np.asarray(b, float)
    side = np.array([-(b - a)[1], (b - a)[0]])
    side *= width / 2 / np.linalg.norm(side)
    return slab([a - side, b - side, b + side, a + side], depth, z)


def wheel() -> Mesh:
    root = TIP - TOOTH_HEIGHT
    rim = root - 0.0042
    teeth = []
    for k in range(TEETH):
        tip = k * PITCH + 180 - OUTER_AT
        teeth.append(slab([polar(TIP, tip), polar(root - 0.001, tip - LEAD), polar(root - 0.001, tip - BACK), polar(TIP, tip - LAND)], 0.003))
    spokes = [bar((0, 0), polar(rim + 0.001, 72 * k), 0.0036, 0.0024) for k in range(5)]
    return shapes.merge(*teeth, annulus(root, rim, 0.003, 90), *spokes, disc(0.006, 0.004))


def pallet_outlines() -> tuple[np.ndarray, np.ndarray]:
    """Both pallets about the anchor's pivot, the wheel straight above: each pallet's two sides are circles about the pivot, so a tooth resting on one holds the wheel dead still however the anchor turns."""
    wheel_centre = np.array([0.0, CENTRES])

    def at(from_bottom: float) -> np.ndarray:
        return polar(TIP, 180 + from_bottom, wheel_centre)

    def body(corner, nib, withdraw: float) -> np.ndarray:
        corner_angle, nib_angle = np.arctan2(corner[1], corner[0]), np.arctan2(nib[1], nib[0])
        end = corner_angle + withdraw * np.radians(PALLET_LENGTH)
        near = [np.linalg.norm(corner) * np.array([np.cos(a), np.sin(a)]) for a in np.linspace(corner_angle, end, 6)]
        far = [np.linalg.norm(nib) * np.array([np.cos(a), np.sin(a)]) for a in np.linspace(end, nib_angle, 6)]
        return np.array(near + far)

    entry = body(turned(at(-OUTER_AT), -UNLOCK), turned(at(-INNER_AT), LIFT - UNLOCK), -1)
    exit = body(turned(at(INNER_AT), UNLOCK), turned(at(OUTER_AT), UNLOCK - LIFT), 1)
    return entry, exit


def anchor() -> Node:
    entry, exit = pallet_outlines()
    arms = []
    for outline in (entry, exit):
        root = (outline[5] + outline[6]) / 2
        along = root / np.linalg.norm(root)
        arms.append(bar(along * 0.004, root - along * 0.003, 0.0065, 0.003))
        arms.append(bar(root - along * 0.003, root + along * 0.003, 0.0075, 0.002, -0.0012))
    crown = bar((entry[5] + entry[6]) * 0.27, (exit[5] + exit[6]) * 0.27, 0.0045, 0.003)
    body = shapes.merge(*arms, crown, disc(0.0065, 0.004))
    arbor = disc(0.0016, 0.02, at=(0, 0, -0.007), sections=12)
    length = -FORK_Y
    prongs = [disc(0.0009, 0.011, at=(dx, -length, PENDULUM_Z), sections=8) for dx in (-0.0023, 0.0023)]
    crutch = shapes.merge(bar((0, 0.003), (0, -length - 0.002), 0.003, 0.002, CRUTCH_Z), bar((-0.0035, -length), (0.0035, -length), 0.0028, 0.002, CRUTCH_Z), *prongs)
    return Node(
        "anchor",
        body,
        BLUED,
        children=[
            Node("entry_pallet", slab(entry, 0.005), RUBY),
            Node("exit_pallet", slab(exit, 0.005), RUBY),
            Node("anchor_arbor", arbor, STEEL),
            Node("crutch", crutch, BLUED),
        ],
    )


def pendulum() -> Node:
    length = -BOB_Y
    spring = shapes.box(0.004, 0.01, 0.0006, at=(0, 0.002, 0))
    chops = shapes.box(0.009, 0.005, 0.004, at=(0, 0.008, 0))
    rod = shapes.translate(shapes.cylinder(0.0012, length - 0.003, 10), (0, -length / 2 - 0.0015, 0))
    lens = shapes.transform(disc(BOB_RADIUS, 1.0, sections=40), np.diag([1.0, 1.0, 0.007, 1.0]))
    face = shapes.transform(shapes.sphere(BOB_RADIUS, 10, 40), np.diag([1.0, 1.0, 0.3, 1.0]))
    nut = shapes.translate(shapes.cylinder(0.0028, 0.005, 12), (0, -length - BOB_RADIUS - 0.003, 0))
    return Node(
        "pendulum",
        translation=(0.0, 0.0, PENDULUM_Z),
        children=[
            Node("suspension", shapes.merge(spring, chops), INK),
            Node("pendulum_rod", shapes.merge(rod, nut), STEEL),
            Node("pendulum_bob", shapes.translate(shapes.merge(lens, face), (0, -length, 0)), BOB),
        ],
    )


def ticks(count: int, radius: float, centre, every: int, long: float, short: float) -> list[Mesh]:
    marks = []
    for k in range(count):
        major = k % every == 0
        a = np.radians(k * 360 / count)
        height = long if major else short
        r = radius - height / 2
        mark = shapes.rotate(shapes.box(0.0011 if major else 0.0006, height, 0.0004), (0, 0, 1), -a)
        marks.append(shapes.translate(mark, (centre[0] + r * np.sin(a), centre[1] + r * np.cos(a), DIAL_Z + 0.0008)))
    return marks


def hand(length: float, width: float, tail: float) -> Mesh:
    return shapes.merge(slab([(-width / 2, -tail), (width / 2, -tail), (width * 0.3, length), (-width * 0.3, length)], 0.0006), disc(width * 0.9, 0.001, sections=12))


def dial() -> Node:
    face = shapes.merge(
        annulus(RING[1], RING[0], 0.0012, 96, at=(0, WHEEL_Y, DIAL_Z)),
        disc(DIAL_RADIUS, 0.0012, at=(*DIAL_AT, DIAL_Z), sections=40),
        annulus(DIAL_RADIUS + 0.0012, DIAL_RADIUS, 0.0018, 40, at=(*DIAL_AT, DIAL_Z)),
    )
    marks = ticks(60, RING[1] - 0.0006, (0, WHEEL_Y), 5, 0.003, 0.0018) + ticks(12, DIAL_RADIUS - 0.001, DIAL_AT, 3, 0.003, 0.002)
    second = shapes.merge(hand(RING[1] - 0.001, 0.0014, 0.009), disc(0.0026, 0.0008, at=(0, -0.009, 0), sections=16))
    return Node(
        "dial",
        face,
        CREAM,
        children=[
            Node("dial_marks", shapes.merge(*marks), INK),
            Node("second_hand", second, (0.85, 0.16, 0.1, 1), translation=(0.0, WHEEL_Y, DIAL_Z + 0.0022)),
            Node("hour_hand", hand(DIAL_RADIUS * 0.58, 0.0024, 0.002), HAND, translation=(*DIAL_AT, DIAL_Z + 0.0016)),
            Node("minute_hand", hand(DIAL_RADIUS * 0.9, 0.0016, 0.0025), HAND, translation=(*DIAL_AT, DIAL_Z + 0.0026)),
        ],
    )


def frame() -> Node:
    top, bottom, half, corner = WHEEL_Y + TIP + 0.008, FORK_Y - 0.008, 0.07, 0.012
    outline = []
    for cx, cy, start in ((half - corner, top - corner, 0), (-half + corner, top - corner, 90), (-half + corner, bottom + corner, 180), (half - corner, bottom + corner, 270)):
        for a in np.radians(np.linspace(start, start + 90, 6)):
            outline.append((cx + corner * np.cos(a), cy + corner * np.sin(a)))
    pillars = [disc(0.0026, -PLATE_Z + 0.002, at=(x, y, (PLATE_Z + 0.002) / 2), sections=12) for x in (-half + 0.007, half - 0.007) for y in (top - 0.007, bottom + 0.007)]
    cock = shapes.box(0.01, 0.012, -PLATE_Z + PENDULUM_Z - 0.0015, at=(0, 0.011, (PLATE_Z + PENDULUM_Z) / 2))
    return Node("frame", shapes.merge(slab(outline, 0.003, PLATE_Z), cock), PLATE, children=[Node("pillars", shapes.merge(*pillars), BRASS)])


def scene():
    arbor = Node("escape_arbor", disc(0.0016, 0.021, at=(0, 0, 0.0015), sections=12), STEEL)
    roots = [dial(), Node("escape_wheel", wheel(), BRASS, translation=(0.0, WHEEL_Y, 0.0), children=[arbor]), anchor(), pendulum(), frame()]
    constants = {
        "AMPLITUDE": np.radians(AMPLITUDE),
        "LIFT": np.radians(LIFT),
        "UNLOCK": np.radians(UNLOCK),
        "IMPULSE_SHARE": (PITCH / 2 - DROP) / (PITCH / 2),
        # Where in the beat the swing carries the nib clear of the tooth and the wheel falls free.
        "IMPULSE_END": np.arccos((UNLOCK - LIFT) / AMPLITUDE) / np.pi,
        "DROP_TIME": DROP_TIME,
    }
    return roots, 2.0, constants
