"""A eukaryotic replication fork running down 24 bp of B-DNA: helicase unwinds, the leading strand grows toward the fork, the lagging strand away from it in primed Okazaki fragments."""
import numpy as np

import shapes
from glb import Node

SEQUENCE = "ATGCGTACCGATTGCAGTCAAGCT"
PAIR = {"A": "T", "T": "A", "G": "C", "C": "G"}
BASE_COLOURS = {"A": (0.2, 0.78, 0.3, 1), "T": (0.92, 0.22, 0.22, 1), "U": (1.0, 0.55, 0.72, 1), "G": (1.0, 0.72, 0.1, 1), "C": (0.22, 0.45, 1.0, 1)}
PARENTAL = (0.3, 0.36, 0.56, 1)
NEW = (0.96, 0.96, 0.93, 1)
RNA = (0.92, 0.2, 0.85, 1)

BP = len(SEQUENCE)
RADIUS = 0.025
RISE = 0.0085
TWIST = 2 * np.pi / 10.5
GROOVE = 2.5
PHASE = 0.4
TOP = (BP - 1) * RISE / 2
OFFSET = 0.037
LIFT = 0.012
FLARE = 2.5
BEAD = 0.0034
STRAND = 0.0017
LINK = float(np.hypot(2 * RADIUS * np.sin(TWIST / 2), RISE))
BASE = RADIUS * np.sin(GROOVE / 2)
FRAGMENT = 6
PRIMER = 2
LAG = 2.0
DURATION = 12.0
SIDES = (-1.0, 1.0)


def helix(angle: int, i: float, fork: float, side: int) -> np.ndarray:
    w = np.clip((fork - i) / FLARE, 0, 1)
    w = w * w * (3 - 2 * w)
    theta = PHASE - i * TWIST + angle * GROOVE
    return np.array([SIDES[side] * OFFSET * w + RADIUS * np.cos(theta), TOP - i * RISE + LIFT * w, -RADIUS * np.sin(theta)])


def frame(x: np.ndarray, y: np.ndarray) -> tuple[float, float, float, float]:
    y = y / np.linalg.norm(y)
    x = x - np.dot(x, y) * y
    x = x / np.linalg.norm(x)
    m = np.stack([x, y, np.cross(x, y)], 1)
    w = np.sqrt(max(1e-9, 1 + m[0, 0] + m[1, 1] + m[2, 2])) / 2
    return (float((m[2, 1] - m[1, 2]) / (4 * w)), float((m[0, 2] - m[2, 0]) / (4 * w)), float((m[1, 0] - m[0, 1]) / (4 * w)), float(w))


def nucleotide(name: str, letter: str, backbone, side: int, angle: int, slot: int, link: int, bonded: bool) -> Node:
    """Rest pose is the finished replication, both daughters fully apart."""
    end = BP - 1 + FLARE
    here = helix(angle, slot, end, side)
    partner = helix(1 - angle, slot, end, side)
    toward = helix(angle, slot + link, end, side) - here if bonded else here - helix(angle, slot - link, end, side)
    bead = shapes.sphere(BEAD, 6, 8)
    mesh = shapes.merge(bead, shapes.cylinder(STRAND, LINK, 6, at=(0, LINK / 2, 0))) if bonded else bead
    base = Node(f"{name}_base", shapes.box(BASE - BEAD * 0.6 - 0.0004, 0.0018, 0.0045, at=((BASE + BEAD * 0.6 - 0.0004) / 2, 0, 0)), BASE_COLOURS[letter])
    return Node(name, mesh, backbone, translation=tuple(float(v) for v in here), rotation=frame((here + partner) / 2 - here, toward), children=[base])


def rna(letter: str) -> str:
    return "U" if letter == "T" else letter


def scene():
    leading_template = Node("leading_template", children=[nucleotide(f"leading_template_{i:02}", PAIR[SEQUENCE[i]], PARENTAL, 0, 1, i, 1, i < BP - 1) for i in range(BP)])
    leading_primer = Node("leading_primer", children=[nucleotide(f"leading_primer_{i:02}", rna(SEQUENCE[i]), RNA, 0, 0, i, -1, i > 0) for i in range(PRIMER)])
    leading_strand = Node("leading_strand", children=[leading_primer] + [nucleotide(f"leading_strand_{i:02}", SEQUENCE[i], NEW, 0, 0, i, -1, True) for i in range(PRIMER, BP)])
    lagging_template = Node("lagging_template", children=[nucleotide(f"lagging_template_{i:02}", SEQUENCE[i], PARENTAL, 1, 0, i, 1, i < BP - 1) for i in range(BP)])
    fragments = []
    for k in range(BP // FRAGMENT):
        start = k * FRAGMENT
        primer_slots = [start + FRAGMENT - 1 - j for j in range(PRIMER)]
        dna_slots = [start + FRAGMENT - 1 - PRIMER - m for m in range(FRAGMENT - PRIMER)]
        refill_slots = [start - 1 - m for m in range(PRIMER)] if k else []
        primer = Node(f"primer_{k + 1}", children=[nucleotide(f"primer_{k + 1}_{s:02}", rna(PAIR[SEQUENCE[s]]), RNA, 1, 1, s, 1, j > 0) for j, s in enumerate(primer_slots)])
        dna = [nucleotide(f"okazaki_{k + 1}_{s:02}", PAIR[SEQUENCE[s]], NEW, 1, 1, s, 1, True) for s in dna_slots + refill_slots]
        fragments.append(Node(f"okazaki_fragment_{k + 1}", children=[primer] + dna))
    lagging_strand = Node("lagging_strand", children=fragments)

    end = BP - 1 + FLARE

    def outward(side: int, angle: int, i: float, by: float) -> tuple[float, float, float]:
        p = helix(angle, i, end, side)
        centre = np.array([SIDES[side] * OFFSET, p[1], 0.0])
        return tuple(float(v) for v in p + (p - centre) / RADIUS * by)

    lobes = [shapes.sphere(0.0042, 6, 10, at=(0.0088 * np.cos(a), 0, 0.0088 * np.sin(a))) for a in np.arange(6) * np.pi / 3]
    helicase = Node("helicase", shapes.merge(shapes.torus(0.0088, 0.0024, 30, 8), *lobes), (0.1, 0.72, 0.68, 1), translation=tuple(float(v) for v in helix(1, BP + 0.5, 0, 0)))
    polymerase = shapes.transform(shapes.sphere(0.012, 12, 18), np.diag([1.0, 0.85, 1.0, 1.0]))
    clamp = shapes.torus(RADIUS + 0.0055, 0.0018, 40, 6)
    arc = 2 * np.pi * np.linspace(0.08, 0.92, 32)
    topo_ring = shapes.tube(np.stack([(RADIUS + 0.006) * np.cos(arc), np.zeros_like(arc), (RADIUS + 0.006) * np.sin(arc)], 1), 0.003, 8)
    topo_ends = [shapes.sphere(0.0042, 6, 10, at=((RADIUS + 0.006) * np.cos(a), 0, (RADIUS + 0.006) * np.sin(a))) for a in arc[[0, -1]]]
    enzymes = [
        helicase,
        Node("primase", shapes.sphere(0.0065, 10, 14), (0.95, 0.35, 0.9, 0.55), translation=outward(1, 1, BP - 0.5, 0.009)),
        Node("leading_polymerase", polymerase, (0.62, 0.5, 1.0, 0.6), translation=outward(0, 0, BP - 1, 0.008)),
        Node("leading_clamp", clamp, (1.0, 0.85, 0.45, 1), translation=(-OFFSET, TOP - (BP - 1.6) * RISE + LIFT, 0.0)),
        Node("lagging_polymerase", polymerase, (0.62, 0.5, 1.0, 0.6), translation=outward(1, 1, 2 * FRAGMENT - PRIMER, 0.008)),
        Node("lagging_clamp", clamp, (1.0, 0.85, 0.45, 1), translation=(OFFSET, TOP - (2 * FRAGMENT - PRIMER + 0.6) * RISE + LIFT, 0.0)),
        Node("topoisomerase", shapes.merge(topo_ring, *topo_ends), (0.55, 0.8, 0.35, 1), translation=(0.0, TOP - (BP + 1) * RISE, 0.0)),
    ]
    roots = [Node("leading_daughter", children=[leading_template, leading_strand]), Node("lagging_daughter", children=[lagging_template, lagging_strand])] + enzymes
    last_event = BP - 1 + LAG + 0.2 + 0.8 * FRAGMENT + 0.4
    constants = {
        "BP": BP, "RADIUS": RADIUS, "RISE": RISE, "TWIST": TWIST, "GROOVE": GROOVE, "PHASE": PHASE, "TOP": TOP, "OFFSET": OFFSET, "LIFT": LIFT, "FLARE": FLARE,
        "LINK": LINK, "FRAGMENT": FRAGMENT, "PRIMER": PRIMER, "LAG": LAG, "FORK_END": BP - 1 + FLARE, "PROGRESS_END": last_event, "START": 0.5, "FINISH": DURATION - 0.6,
    }
    return roots, DURATION, constants
