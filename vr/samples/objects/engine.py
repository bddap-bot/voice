"""A single-cylinder four-stroke engine, cut away: slider-crank piston, overhead cams at half crank speed, one 720° cycle every four seconds."""
import numpy as np

import shapes
from glb import Mesh, Node

DURATION = 4.0
CRANK = 0.024
ROD = 0.070
BORE = 0.0255
DECK = 0.108
HEAD_TOP = 0.132
CROWN = 0.010
TILT = np.radians(12)
SEAT_X = 0.011
TAPPET_TOP = 0.040
BASE_CIRCLE = 0.0065
LIFT = 0.006
SPRING_FROM = 0.025
SPRING_LENGTH = 0.010
BELT_Z = -0.026
PLUG_TILT = np.radians(22)
PORT_INNER = 0.012
PORT_OUTER = 0.042

STEEL = (0.62, 0.64, 0.68, 1)
DARK = (0.2, 0.21, 0.23, 1)


def along_z(mesh: Mesh) -> Mesh:
    return shapes.rotate(mesh, (1, 0, 0), np.pi / 2)


def sleeve(radius: float, low: float, high: float, sections: int = 32) -> Mesh:
    """A cylinder's side alone: a cap would lay one more translucent sheet over the chamber."""
    walls = shapes.cylinder(radius, high - low, sections, at=(0, (high + low) / 2, 0))
    return Mesh(walls.positions, walls.normals, walls.indices[: 2 * sections])


def lift(theta: float, valve: str) -> float:
    theta %= 4 * np.pi
    open_from = 0.0 if valve == "intake" else 3 * np.pi
    if open_from <= theta < open_from + np.pi:
        return LIFT * np.sin(theta - open_from) ** 2
    return 0.0


def axis(side: float) -> np.ndarray:
    """The valve's up direction: each valve leans outward, so its cam clears the plug."""
    return np.array([side * np.sin(TILT), np.cos(TILT), 0.0])


def cam(valve: str, side: float) -> Mesh:
    """A radial lobe whose radius toward the tappet equals base circle plus the valve's lift, turning clockwise at half crank speed."""
    u = axis(side)
    contact = np.arctan2(-u[1], -u[0])
    outline = []
    for a in np.linspace(0, 2 * np.pi, 72, endpoint=False):
        radius = BASE_CIRCLE + lift(2 * (a - contact), valve)
        outline.append([radius * np.cos(a), -radius * np.sin(a)])
    lobe = along_z(shapes.extrude(outline, 0.008))
    shaft = along_z(shapes.cylinder(0.0028, 0.044, 12, at=(0, -0.008, 0)))
    return shapes.merge(lobe, shaft)


def cam_sprocket() -> Mesh:
    return shapes.merge(
        shapes.translate(along_z(shapes.gear(0.0148, 0.016, 20, 0.003)), (0, 0, BELT_Z)),
        shapes.translate(along_z(shapes.cylinder(0.005, 0.0045, 12)), (0, 0, BELT_Z)),
    )


def valve(side: float) -> tuple[Mesh, np.ndarray]:
    u = axis(side)
    seat = np.array([side * SEAT_X, DECK, 0.0])
    local = shapes.merge(
        shapes.cylinder(0.0075, 0.0015, 20, at=(0, 0.00075, 0)),
        shapes.cylinder(0.0035, 0.003, 12, at=(0, 0.003, 0)),
        shapes.cylinder(0.0011, TAPPET_TOP, 8, at=(0, TAPPET_TOP / 2, 0)),
        shapes.cylinder(0.0045, 0.005, 16, at=(0, TAPPET_TOP - 0.0025, 0)),
    )
    m = shapes.rotation((0, 0, 1), -side * TILT)
    m[:3, 3] = seat
    return shapes.transform(local, m), seat


def spring() -> Mesh:
    turns = 4.5
    t = np.linspace(0, 1, 160)
    path = np.stack([0.0042 * np.cos(t * turns * 2 * np.pi), t * SPRING_LENGTH, -0.0042 * np.sin(t * turns * 2 * np.pi)], 1)
    return shapes.tube(path, 0.0007, 6)


def crank_web() -> Mesh:
    outline = []
    for a in np.linspace(0, 2 * np.pi, 64, endpoint=False):
        d = np.array([np.cos(a), np.sin(a)])
        b = d @ np.array([0, CRANK])
        pin_reach = b + np.sqrt(max(b * b - CRANK**2 + 0.0095**2, 0.0))
        down = abs(((a + np.pi / 2 + np.pi) % (2 * np.pi)) - np.pi) < np.radians(62)
        radius = max(0.011, pin_reach, 0.021 if down else 0.0)
        outline.append([radius * d[0], -radius * d[1]])
    return along_z(shapes.extrude(outline, 0.005))


def crankshaft() -> Mesh:
    return shapes.merge(
        shapes.translate(along_z(shapes.cylinder(0.0052, 0.016, 16)), (0, CRANK, 0)),
        along_z(shapes.cylinder(0.006, 0.056, 16, at=(0, -0.006, 0))),
        shapes.translate(along_z(shapes.gear(0.0068, 0.008, 10, 0.004)), (0, 0, BELT_Z)),
    )


def connecting_rod() -> Mesh:
    return shapes.merge(
        along_z(shapes.cylinder(0.0095, 0.0055, 20)),
        shapes.box(0.0075, ROD - 0.012, 0.0045, at=(0, ROD / 2, 0)),
        shapes.translate(along_z(shapes.cylinder(0.0055, 0.0055, 16)), (0, ROD, 0)),
    )


def belt() -> Mesh:
    cams_y = DECK + (TAPPET_TOP + BASE_CIRCLE) * np.cos(TILT)
    cams_x = SEAT_X + (TAPPET_TOP + BASE_CIRCLE) * np.sin(TILT)
    circles = [((0.0, 0.0), 0.0085), ((-cams_x, cams_y), 0.0165), ((cams_x, cams_y), 0.0165)]
    points = sorted({(round(cx + r * np.cos(a), 6), round(cy + r * np.sin(a), 6)) for (cx, cy), r in circles for a in np.linspace(0, 2 * np.pi, 90, endpoint=False)})

    def half(ps):
        hull = []
        for p in ps:
            while len(hull) >= 2 and np.cross(np.subtract(hull[-1], hull[-2]), np.subtract(p, hull[-2])) <= 0:
                hull.pop()
            hull.append(p)
        return hull

    loop = half(points)[:-1] + half(points[::-1])[:-1]
    loop.append(loop[0])
    dense = []
    for p, q in zip(loop, loop[1:]):
        steps = max(1, int(np.hypot(q[0] - p[0], q[1] - p[1]) / 0.003))
        dense += [(p[0] + (q[0] - p[0]) * s / steps, p[1] + (q[1] - p[1]) * s / steps) for s in range(steps)]
    dense.append(dense[0])
    return shapes.tube([(x, y, BELT_Z) for x, y in dense], 0.0013, 6)


def spark_star() -> Mesh:
    """Flat, because at ignition the chamber is a disc a few millimetres high."""
    rays = [shapes.rotate(shapes.box(0.044 - 0.014 * (k % 2), 0.0025, 0.0025), (0, 1, 0), k * np.pi / 5) for k in range(5)]
    return shapes.merge(shapes.transform(shapes.sphere(0.009, 8, 12), np.diag([1.0, 0.5, 1.0, 1.0])), *rays)


def scene():
    wrist = CRANK + ROD
    intake_mesh, intake_seat = valve(-1)
    exhaust_mesh, exhaust_seat = valve(1)
    cam_centre = {side: np.array([side * SEAT_X, DECK, 0]) + axis(side) * (TAPPET_TOP + BASE_CIRCLE) for side in (-1, 1)}
    spring_base = {side: np.array([side * SEAT_X, DECK, 0]) + axis(side) * SPRING_FROM for side in (-1, 1)}
    port = {side: (np.array([side * PORT_INNER, DECK + 0.004, 0]), np.array([side * PORT_OUTER, DECK + 0.017, 0])) for side in (-1, 1)}

    def port_mesh(side):
        inner, outer = port[side]
        bend = (inner + outer) / 2 + [0, 0.004, 0]
        path = [inner + (bend - inner) * s for s in np.linspace(0, 1, 5)] + [bend + (outer - bend) * s for s in np.linspace(0.25, 1, 4)]
        return shapes.tube(path, 0.0055, 12)

    gas = shapes.merge(sleeve(BORE - 0.0004, -1.0, 0.0, 28), shapes.cylinder(BORE - 0.0004, 0.0, 28, at=(0, -1.0, 0)))
    chamber = DECK - (wrist + CROWN)

    def charge(name, colour, shown):
        return Node(name, gas, colour, translation=(0, DECK, 0), scale=(1.0, chamber, 1.0) if shown else (0.0, 0.0, 0.0))

    puff = shapes.sphere(0.003, 6, 10)
    head = Node("cylinder_head", shapes.merge(sleeve(0.037, DECK, HEAD_TOP), shapes.cylinder(0.037, 0.001, 32, at=(0, HEAD_TOP - 0.0005, 0))), (0.62, 0.66, 0.74, 0.2), double_sided=True, children=[
        Node("intake_port", port_mesh(-1), (0.35, 0.55, 1.0, 0.35)),
        Node("exhaust_port", port_mesh(1), (0.95, 0.45, 0.3, 0.35)),
        Node("intake_valve", intake_mesh, (0.45, 0.65, 1.0, 1)),
        Node("exhaust_valve", exhaust_mesh, (1.0, 0.5, 0.38, 1)),
        Node("intake_spring", spring(), (0.35, 0.8, 0.45, 1), translation=tuple(spring_base[-1]), rotation=shapes.quaternion((0, 0, 1), TILT)),
        Node("exhaust_spring", spring(), (0.35, 0.8, 0.45, 1), translation=tuple(spring_base[1]), rotation=shapes.quaternion((0, 0, 1), -TILT)),
        Node("camshaft", children=[
            Node("intake_cam", cam("intake", -1), (0.95, 0.75, 0.25, 1), translation=tuple(cam_centre[-1]), children=[Node("intake_sprocket", cam_sprocket(), DARK)]),
            Node("exhaust_cam", cam("exhaust", 1), (0.95, 0.75, 0.25, 1), translation=tuple(cam_centre[1]), children=[Node("exhaust_sprocket", cam_sprocket(), DARK)]),
        ]),
        *[Node(f"intake_puff_{k}", puff, (0.35, 0.6, 1.0, 0.8), translation=tuple(port[-1][1]), scale=(0, 0, 0)) for k in range(3)],
        *[Node(f"exhaust_puff_{k}", puff, (0.55, 0.55, 0.55, 0.8), translation=tuple(port[1][1]), scale=(0, 0, 0)) for k in range(3)],
    ])
    plug_axis = np.array([0, np.cos(PLUG_TILT), np.sin(PLUG_TILT)])
    plug_local = shapes.merge(
        shapes.cylinder(0.0006, 0.004, 6, at=(0, 0.0, 0)),
        shapes.box(0.003, 0.0008, 0.0008, at=(0.0009, -0.0018, 0)),
        shapes.cylinder(0.0028, 0.024, 12, at=(0, 0.014, 0)),
    )
    plug_top = shapes.merge(
        shapes.cylinder(0.0055, 0.006, 6, at=(0, 0.029, 0)),
        shapes.cylinder(0.0038, 0.022, 14, at=(0, 0.043, 0)),
        shapes.cylinder(0.002, 0.005, 8, at=(0, 0.056, 0)),
    )
    plug_matrix = shapes.rotation((1, 0, 0), PLUG_TILT)
    tip = np.array([0, DECK - 0.0012, 0])
    plug_matrix[:3, 3] = tip
    plug = Node("spark_plug", shapes.transform(plug_local, plug_matrix), (0.55, 0.56, 0.6, 1), children=[
        Node("plug_insulator", shapes.transform(plug_top, plug_matrix), (0.95, 0.94, 0.9, 1)),
        Node("spark", spark_star(), (1.0, 0.92, 0.0, 1), translation=tuple(tip + plug_axis * -0.001), scale=(0, 0, 0)),
    ])
    block = Node("cylinder_block", sleeve(0.031, 0.030, DECK), (0.6, 0.68, 0.78, 0.15), double_sided=True, children=[
        Node("crankcase", along_z(shapes.merge(sleeve(0.033, -0.018, 0.018), shapes.cylinder(0.033, 0.0, 32, at=(0, -0.018, 0)))), (0.55, 0.6, 0.66, 0.1), double_sided=True),
        Node("timing_belt", belt(), (0.12, 0.12, 0.14, 1)),
        charge("fresh_charge", (0.25, 0.5, 1.0, 0.4), True),
        charge("combustion", (1.0, 0.5, 0.08, 0.8), False),
        charge("exhaust_gas", (0.3, 0.27, 0.25, 0.8), False),
    ])
    piston_crown = wrist + CROWN
    piston = Node("piston", shapes.merge(
        shapes.cylinder(BORE - 0.0003, 0.009, 32, at=(0, piston_crown - 0.0045, 0)),
        shapes.translate(shapes.torus(BORE - 0.0006, 0.0006, 32, 4), (0, piston_crown - 0.0062, 0)),
        shapes.translate(shapes.torus(BORE - 0.0006, 0.0006, 32, 4), (0, piston_crown - 0.0080, 0)),
    ), (0.8, 0.8, 0.83, 1), children=[
        Node("piston_skirt", shapes.cylinder(BORE - 0.0004, 0.021, 32, at=(0, piston_crown - 0.0195, 0)), (0.8, 0.8, 0.85, 0.35), double_sided=True),
        Node("wrist_pin", shapes.translate(along_z(shapes.cylinder(0.003, 0.044, 12)), (0, wrist, 0)), DARK),
    ])
    rod = Node("connecting_rod", connecting_rod(), (0.3, 0.5, 0.85, 1), translation=(0, CRANK, 0))
    crank = Node("crankshaft", crankshaft(), (0.72, 0.73, 0.78, 1), children=[
        Node("counterweight", shapes.merge(shapes.translate(crank_web(), (0, 0, 0.0075)), shapes.translate(crank_web(), (0, 0, -0.0075))), (0.16, 0.16, 0.2, 1)),
    ])
    roots = [head, plug, block, piston, rod, crank]
    constants = {
        "CRANK_RADIUS": CRANK, "ROD_LENGTH": ROD, "DECK_Y": DECK, "CROWN_ABOVE_PIN": CROWN, "VALVE_LIFT": LIFT,
        "VALVE_UX": np.sin(TILT), "VALVE_UY": np.cos(TILT), "SPRING_LENGTH": SPRING_LENGTH,
        "PORT_INNER_X": PORT_INNER, "PORT_INNER_Y": DECK + 0.004, "PORT_OUTER_X": PORT_OUTER, "PORT_OUTER_Y": DECK + 0.017,
    }
    return roots, DURATION, constants
