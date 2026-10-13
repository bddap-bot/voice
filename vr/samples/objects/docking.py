"""Caffeine docking into the human adenosine A2A receptor, from the crystal structure RCSB PDB 5MZP (`5mzp.pdb`, an extract).

The binding pocket is drawn as ribbon, one node per helix segment, with licorice side chains of the residues touching caffeine; caffeine is ball-and-stick in CPK colours.
Its hydrogen bond to Asn253 and its stacking on Phe168 are found from the coordinates, not typed in.
Rest pose is the bound pose; the program brings caffeine in from the extracellular side and grows the interactions as it settles.
"""
import math
from collections import defaultdict
from pathlib import Path

import numpy as np

from glb import Mesh, Node

PDB = Path(__file__).with_name("5mzp.pdb")
SIZE = 0.18
POCKET = 8.5
CONTACT = 4.5
HBOND = 3.3
START = 20.0
DURATION, APPROACH, GROW_FROM, GROW_TO = 8.0, 5.0, 4.6, 5.4

CPK = {"C": (0.42, 0.42, 0.45, 1), "N": (0.2, 0.35, 0.95, 1), "O": (0.92, 0.15, 0.12, 1), "S": (0.95, 0.8, 0.15, 1)}
SIDECHAIN_CARBON = (0.45, 0.8, 0.55, 1)
BOND = (0.85, 0.85, 0.87, 1)
HYDROGEN_BOND = (1.0, 0.85, 0.1, 1)
STACKING = (0.15, 0.85, 0.95, 1)
SEGMENTS = {"TM1": (1, 34, (0.6, 0.6, 0.7, 1)), "TM2": (35, 70, (0.4, 0.72, 0.85, 1)), "TM3": (71, 112, (0.45, 0.5, 0.9, 1)), "TM4": (113, 145, (0.6, 0.6, 0.6, 1)),
            "ECL2": (146, 185, (0.9, 0.78, 0.45, 1)), "TM5": (186, 215, (0.92, 0.55, 0.4, 1)), "TM6": (216, 262, (0.5, 0.75, 0.45, 1)), "TM7": (263, 310, (0.78, 0.5, 0.82, 1))}
# Each transmembrane helix's first and last residue, and +1 where it runs from the cytoplasm out: summed, they point out of the cell.
TRANSMEMBRANE = [(1, 34, -1), (39, 58, 1), (74, 108, -1), (117, 137, 1), (186, 208, -1), (220, 259, 1), (266, 292, -1)]
RESIDUE = {"ALA": "Ala", "ARG": "Arg", "ASN": "Asn", "ASP": "Asp", "CYS": "Cys", "GLN": "Gln", "GLU": "Glu", "HIS": "His", "ILE": "Ile",
           "LEU": "Leu", "LYS": "Lys", "MET": "Met", "PHE": "Phe", "SER": "Ser", "THR": "Thr", "TRP": "Trp", "TYR": "Tyr", "VAL": "Val"}
PHENYL = {"CG", "CD1", "CD2", "CE1", "CE2", "CZ"}
PYRIMIDINE = {"N1", "C2", "N3", "C4", "C5", "C6"}
IMIDAZOLE = {"C4", "C5", "N7", "C8", "N9"}


def unit(v):
    return v / (np.linalg.norm(v) or 1)


def parse():
    protein, ligand, helices = [], [], set()
    for line in PDB.read_text().splitlines():
        if line.startswith("HELIX"):
            helices.update(range(int(line[21:25]), int(line[33:37]) + 1))
        elif line[:6] in ("ATOM  ", "HETATM"):
            atom = dict(name=line[12:16].strip(), res=line[17:20].strip(), seq=int(line[22:26]), xyz=np.array([float(line[30:38]), float(line[38:46]), float(line[46:54])]), el=line[76:78].strip())
            (ligand if atom["res"] == "CFF" else protein).append(atom)
    return protein, ligand, helices


class Parts:
    """Triangles of one part grouped by colour; glTF draws one colour per node, so each colour after the first is a child."""

    def __init__(self):
        self.groups = defaultdict(lambda: ([], [], []))

    def add(self, label, color, positions, normals, triangles):
        p, n, t = self.groups[(label, color)]
        base = len(p)
        p.extend(positions)
        n.extend(normals)
        t.extend([a + base, b + base, c + base] for a, b, c in triangles)

    def sphere(self, label, color, centre, radius, rings=6, sections=8):
        theta, phi = np.meshgrid(np.linspace(0, np.pi, rings + 1), np.linspace(0, 2 * np.pi, sections + 1), indexing="ij")
        normals = np.stack([np.sin(theta) * np.cos(phi), np.cos(theta), -np.sin(theta) * np.sin(phi)], -1).reshape(-1, 3)
        triangles = [[r * (sections + 1) + s, (r + 1) * (sections + 1) + s, (r + 1) * (sections + 1) + s + 1] for r in range(rings) for s in range(sections)]
        triangles += [[r * (sections + 1) + s, (r + 1) * (sections + 1) + s + 1, r * (sections + 1) + s + 1] for r in range(rings) for s in range(sections)]
        self.add(label, color, centre + normals * radius, normals, triangles)

    def cylinder(self, label, color, a, b, radius, sides=8):
        u, v = frame(unit(b - a))
        ring = [math.cos(2 * math.pi * i / sides) * u + math.sin(2 * math.pi * i / sides) * v for i in range(sides)]
        triangles = []
        for i in range(sides):
            j = (i + 1) % sides
            triangles += [[i, j, sides + j], [i, sides + j, sides + i]]
        self.add(label, color, [a + r * radius for r in ring] + [b + r * radius for r in ring], ring + ring, triangles)

    def dashes(self, label, color, a, b, radius, count):
        for k in range(count):
            self.cylinder(label, color, a + (b - a) * (2 * k + 0.5) / (2 * count), a + (b - a) * (2 * k + 1.5) / (2 * count), radius)

    def ring(self, label, color, centre, axis, radius, tube, segments=24, sides=6):
        u, v = frame(axis)
        positions, normals, triangles = [], [], []
        for i in range(segments):
            out = math.cos(2 * math.pi * i / segments) * u + math.sin(2 * math.pi * i / segments) * v
            for k in range(sides):
                n = math.cos(2 * math.pi * k / sides) * out + math.sin(2 * math.pi * k / sides) * axis
                positions.append(centre + out * radius + n * tube)
                normals.append(n)
        for i in range(segments):
            for k in range(sides):
                a, b = i * sides + k, (i + 1) % segments * sides + k
                c, d = (i + 1) % segments * sides + (k + 1) % sides, i * sides + (k + 1) % sides
                triangles += [[a, b, c], [a, c, d]]
        self.add(label, color, positions, normals, triangles)

    def ribbon(self, label, color, cas, helix, steps=4, sides=8):
        """A flattened tube through a CA trace, wide on helices; `cas` repeats its first and last point for the end tangents."""
        sideways = []
        for i in range(1, len(cas) - 1):
            s = unit(cas[i - 1] + cas[i + 1] - 2 * cas[i])
            sideways.append(-s if sideways and np.dot(s, sideways[-1]) < 0 else s)
        sideways = [sideways[0]] + sideways + [sideways[-1]]
        points, binormals = [], []
        for i in range(1, len(cas) - 2):
            for s in range(steps + (i == len(cas) - 3)):
                t = s / steps
                p0, p1, p2, p3 = cas[i - 1:i + 3]
                points.append(0.5 * (2 * p1 + (p2 - p0) * t + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t * t + (3 * p1 - p0 - 3 * p2 + p3) * t ** 3))
                binormals.append(unit(sideways[i] * (1 - t) + sideways[i + 1] * t))
        positions, normals, triangles = [], [], []
        tangents = [unit(points[min(k + 1, len(points) - 1)] - points[max(k - 1, 0)]) for k in range(len(points))]
        for k, p in enumerate(points):
            across = unit(np.cross(tangents[k], binormals[k]))
            face = np.cross(across, tangents[k])
            width, thick = (0.9, 0.22) if helix[k // steps] else (0.35, 0.3)
            for j in range(sides):
                c, s = math.cos(2 * math.pi * j / sides), math.sin(2 * math.pi * j / sides)
                positions.append(p + across * width * c + face * thick * s)
                normals.append(unit(across * c / width + face * s / thick))
        for k in range(len(points) - 1):
            for j in range(sides):
                a, b = k * sides + j, k * sides + (j + 1) % sides
                triangles += [[a, b, b + sides], [a, b + sides, a + sides]]
        for k, sign in ((0, -1), (len(points) - 1, 1)):
            base = len(positions)
            positions += positions[k * sides:(k + 1) * sides]
            normals += [tangents[k] * sign] * sides
            triangles += [[base, base + j + 1, base + j][::sign] for j in range(1, sides - 1)]
        self.add(label, color, positions, normals, triangles)

    def node(self, name, at, scale):
        made = [Node(name if not k else f"{name}_{label}", Mesh(np.array(p) * scale, np.array(n), np.array(t)), color) for k, ((label, color), (p, n, t)) in enumerate(self.groups.items())]
        made[0].translation = tuple(float(v) for v in at * scale)
        made[0].children = made[1:]
        return made[0]


def frame(axis):
    other = np.array([1.0, 0, 0]) if abs(axis[0]) < 0.9 else np.array([0, 1.0, 0])
    u = unit(np.cross(axis, other))
    return u, np.cross(axis, u)


def bonds(atoms, cutoff):
    return [(i, j) for i in range(len(atoms)) for j in range(i + 1, len(atoms)) if np.linalg.norm(atoms[i]["xyz"] - atoms[j]["xyz"]) < cutoff]


def plane(atoms, names):
    ring = [a["xyz"] for a in atoms if a["name"] in names]
    return np.mean(ring, 0), unit(np.cross(ring[1] - ring[0], ring[2] - ring[0]))


def directions(count, keep):
    for i in range(count):
        z = 1 - (i + 0.5) / count * 2
        phi = i * math.pi * (3 - 5 ** 0.5)
        d = np.array([math.sqrt(1 - z * z) * math.cos(phi), z, math.sqrt(1 - z * z) * math.sin(phi)])
        if keep(d):
            yield d


def scene():
    protein, ligand, helices = parse()
    origin = np.mean([a["xyz"] for a in ligand], 0)
    distance = lambda xyz: min(np.linalg.norm(xyz - l["xyz"]) for l in ligand)
    ca = {a["seq"]: a["xyz"] for a in protein if a["name"] == "CA"}
    up = unit(sum(sign * (ca[last] - ca[first]) for first, last, sign in TRANSMEMBRANE))

    def openness(d):
        return sum(min(np.linalg.norm(origin + d * r - a["xyz"]) for a in protein) for r in (4, 6, 8, 10, 12))
    entry = max(directions(400, lambda d: np.dot(d, up) > 0.5), key=openness)

    pocket = sorted({a["seq"] for a in protein if distance(a["xyz"]) < POCKET})
    residues = defaultdict(list)
    for a in protein:
        residues[(a["seq"], a["res"])].append(a)
    contacts = [(f"{RESIDUE[res]}{seq}", side) for (seq, res), atoms in sorted(residues.items())
                for side in [[a for a in atoms if a["name"] not in ("N", "C", "O")]]
                if res in RESIDUE and any(distance(a["xyz"]) < CONTACT for a in side if a["name"] != "CA")]

    drawn = [ca[s] for s in pocket if s in ca] + [a["xyz"] for _, side in contacts for a in side]

    def blocked(d):
        hits = 0
        for p in drawn:
            along = np.dot(p - origin, d)
            hits += along > 1.5 and np.linalg.norm(p - origin - d * along) < 3.5
        return hits + 4 * (1 - np.dot(d, up))
    # Looking down into the pocket from where the fewest atoms stand between the wearer and caffeine.
    toward = min(directions(600, lambda d: 0.0 <= np.dot(d, up) <= 0.7), key=blocked)
    x = unit(np.cross(up, toward))
    axes = np.stack([x, np.cross(toward, x), toward])
    place = lambda xyz: axes @ (xyz - origin)

    runs, run = [], []
    for s in pocket:
        if run and s - run[-1] > 3:
            runs.append(run)
            run = []
        run += list(range(run[-1] + 1, s)) if run else []
        run.append(s)
    runs = [r for r in runs + [run] if len(r) >= 3]

    parts = []
    for r in runs:
        name = max(SEGMENTS, key=lambda segment: sum(SEGMENTS[segment][0] <= s <= SEGMENTS[segment][1] for s in r))
        span = [s for s in range(r[0] - 1, r[-1] + 2) if s in ca]
        trace = [place(ca[s]) for s in span]
        centre = np.mean(trace, 0)
        part = Parts()
        part.ribbon("ribbon", SEGMENTS[name][2], [trace[0] - centre] + [p - centre for p in trace] + [trace[-1] - centre], [s in helices for s in span])
        parts.append((name, centre, part))
        if sum(n == name for n, _, _ in parts) > 1:
            raise SystemExit(f"two pocket segments would be named {name}")

    for name, side in contacts:
        points = [place(a["xyz"]) for a in side]
        centre = np.mean(points, 0)
        local = [p - centre for p in points]
        part = Parts()
        tint = lambda atom: ("C", SIDECHAIN_CARBON) if atom["el"] == "C" else (atom["el"], CPK[atom["el"]])
        for i, j in bonds(side, 1.9):
            middle = (local[i] + local[j]) / 2
            for k in (i, j):
                part.cylinder(*tint(side[k]), local[k], middle, 0.17)
        for k, atom in enumerate(side):
            part.sphere(*tint(atom), local[k], 0.17)
        parts.append((name, centre, part))

    lig = [place(a["xyz"]) for a in ligand]
    caffeine = Parts()
    for i, j in bonds(ligand, 1.75):
        caffeine.cylinder("bonds", BOND, lig[i], lig[j], 0.17, 12)
    for k, atom in enumerate(ligand):
        caffeine.sphere(atom["el"], CPK[atom["el"]], lig[k], 0.5 if atom["el"] == "C" else 0.55, 8, 12)
    parts.append(("caffeine", np.zeros(3), caffeine))

    interactions = []
    for name, side in contacts:
        for a in side:
            for l in ligand:
                if a["el"] in "NO" and l["el"] in "NO" and np.linalg.norm(a["xyz"] - l["xyz"]) < HBOND:
                    p, q = place(a["xyz"]), place(l["xyz"])
                    bond = Parts()
                    bond.dashes("dashes", HYDROGEN_BOND, p - (p + q) / 2, q - (p + q) / 2, 0.16, 4)
                    interactions.append((f"hbond_{name}", (p + q) / 2, bond))
    phenyl, phenyl_normal = plane(dict(contacts)["Phe168"], PHENYL)
    purine = (plane(ligand, PYRIMIDINE)[0] + plane(ligand, IMIDAZOLE)[0]) / 2
    purine_normal = plane(ligand, PYRIMIDINE)[1]
    p, q = place(phenyl), place(purine)
    stack = Parts()
    stack.dashes("dashes", STACKING, p - (p + q) / 2, q - (p + q) / 2, 0.16, 3)
    stack.ring("dashes", STACKING, p - (p + q) / 2, axes @ phenyl_normal, 1.4, 0.14)
    stack.ring("dashes", STACKING, q - (p + q) / 2, axes @ purine_normal, 2.0, 0.14)
    interactions.append(("pi_stack_Phe168", (p + q) / 2, stack))

    points = np.concatenate([np.array(pts) + centre for _, centre, part in parts + interactions for pts, _, _ in part.groups.values()])
    scale = SIZE / (points.max(0) - points.min(0)).max()
    roots = [part.node(name, centre, scale) for name, centre, part in parts]
    group = np.mean([centre for _, centre, _ in interactions], 0)
    roots.append(Node("interactions", translation=tuple(float(v) for v in group * scale), children=[part.node(name, centre - group, scale) for name, centre, part in interactions]))
    start = axes @ entry * START * scale
    constants = {"START_X": start[0], "START_Y": start[1], "START_Z": start[2], "APPROACH": APPROACH, "GROW_FROM": GROW_FROM, "GROW_TO": GROW_TO}
    return roots, DURATION, constants
