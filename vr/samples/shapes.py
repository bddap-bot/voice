"""Meshes in metres, built with numpy alone; hard edges carry their own vertices so they stay flat-shaded."""
import numpy as np

from glb import Mesh


def outward(mesh: Mesh) -> Mesh:
    """Each triangle wound counter-clockwise as seen from the side its normals face, glTF's front."""
    p, f = mesh.positions, mesh.indices.copy()
    facing = np.cross(p[f[:, 1]] - p[f[:, 0]], p[f[:, 2]] - p[f[:, 0]])
    flip = (facing * mesh.normals[f].sum(1)).sum(1) < 0
    f[flip] = f[flip][:, ::-1]
    return Mesh(p, mesh.normals, f)


def merge(*meshes: Mesh) -> Mesh:
    offsets = np.cumsum([0] + [len(m.positions) for m in meshes])[:-1]
    return Mesh(np.concatenate([m.positions for m in meshes]), np.concatenate([m.normals for m in meshes]), np.concatenate([m.indices + o for m, o in zip(meshes, offsets)]))


def transform(mesh: Mesh, matrix: np.ndarray) -> Mesh:
    linear = matrix[:3, :3]
    normals = mesh.normals @ np.linalg.inv(linear)
    normals /= np.linalg.norm(normals, axis=1, keepdims=True)
    return Mesh(mesh.positions @ linear.T + matrix[:3, 3], normals, mesh.indices)


def translate(mesh: Mesh, offset) -> Mesh:
    return Mesh(mesh.positions + np.asarray(offset, float), mesh.normals, mesh.indices)


def rotation(axis, angle: float) -> np.ndarray:
    axis = np.asarray(axis, float) / np.linalg.norm(axis)
    x, y, z = axis
    c, s = np.cos(angle), np.sin(angle)
    m = np.eye(4)
    m[:3, :3] = c * np.eye(3) + s * np.array([[0, -z, y], [z, 0, -x], [-y, x, 0]]) + (1 - c) * np.outer(axis, axis)
    return m


def rotate(mesh: Mesh, axis, angle: float) -> Mesh:
    return transform(mesh, rotation(axis, angle))


def quaternion(axis, angle: float) -> tuple[float, float, float, float]:
    axis = np.asarray(axis, float) / np.linalg.norm(axis)
    s = np.sin(angle / 2)
    return (float(axis[0] * s), float(axis[1] * s), float(axis[2] * s), float(np.cos(angle / 2)))


def box(sx: float, sy: float, sz: float, at=(0, 0, 0)) -> Mesh:
    positions, normals, indices = [], [], []
    for axis in range(3):
        for sign in (-1, 1):
            n = np.zeros(3)
            n[axis] = sign
            u, v = np.eye(3)[(axis + 1) % 3], np.eye(3)[(axis + 2) % 3]
            if sign < 0:
                u, v = v, u
            base = len(positions)
            for a, b in ((-1, -1), (1, -1), (1, 1), (-1, 1)):
                positions.append((n + a * u + b * v) * 0.5 * np.array([sx, sy, sz]))
                normals.append(n)
            indices += [[base, base + 1, base + 2], [base, base + 2, base + 3]]
    return translate(outward(Mesh(np.array(positions), np.array(normals), np.array(indices))), at)


def extrude(outline, depth: float) -> Mesh:
    """A flat polygon in the xz plane, counter-clockwise seen from +y, as a slab from y = -depth/2 to +depth/2; it may be concave if it is star-shaped about its first point's centroid."""
    outline = np.asarray(outline, float)
    n = len(outline)
    centre = outline.mean(0)
    positions, normals, indices = [], [], []
    for sign in (1, -1):
        base = len(positions)
        positions.append([centre[0], sign * depth / 2, centre[1]])
        normals.append([0, sign, 0])
        for x, z in outline:
            positions.append([x, sign * depth / 2, z])
            normals.append([0, sign, 0])
        for i in range(n):
            a, b = base + 1 + i, base + 1 + (i + 1) % n
            indices.append([base, b, a] if sign > 0 else [base, a, b])
    for i in range(n):
        (x0, z0), (x1, z1) = outline[i], outline[(i + 1) % n]
        normal = np.array([z1 - z0, 0, -(x1 - x0)])
        normal /= np.linalg.norm(normal) or 1
        base = len(positions)
        for x, z, y in ((x0, z0, -1), (x1, z1, -1), (x1, z1, 1), (x0, z0, 1)):
            positions.append([x, y * depth / 2, z])
            normals.append(normal)
        indices += [[base, base + 2, base + 1], [base, base + 3, base + 2]]
    return outward(Mesh(np.array(positions), np.array(normals), np.array(indices)))


def cylinder(radius: float, height: float, sections: int = 24, at=(0, 0, 0)) -> Mesh:
    """Along y, centred on `at`, with flat caps."""
    angles = np.linspace(0, 2 * np.pi, sections, endpoint=False)
    ring = np.stack([np.cos(angles), np.zeros(sections), -np.sin(angles)], 1)
    side_positions = np.concatenate([ring * radius + [0, -height / 2, 0], ring * radius + [0, height / 2, 0]])
    side_normals = np.concatenate([ring, ring])
    side = []
    for i in range(sections):
        j = (i + 1) % sections
        side += [[i, j, sections + j], [i, sections + j, sections + i]]
    tube = Mesh(side_positions, side_normals, np.array(side))
    cap = extrude(np.stack([np.cos(angles), -np.sin(angles)], 1) * radius, height)
    caps = Mesh(cap.positions[: 2 * (sections + 1)], cap.normals[: 2 * (sections + 1)], np.array([f for f in cap.indices if max(f) < 2 * (sections + 1)]))
    return translate(outward(merge(tube, caps)), at)


def sphere(radius: float, rings: int = 12, sections: int = 20, at=(0, 0, 0)) -> Mesh:
    theta = np.linspace(0, np.pi, rings + 1)
    phi = np.linspace(0, 2 * np.pi, sections + 1)
    t, p = np.meshgrid(theta, phi, indexing="ij")
    normals = np.stack([np.sin(t) * np.cos(p), np.cos(t), -np.sin(t) * np.sin(p)], -1).reshape(-1, 3)
    indices = []
    for r in range(rings):
        for s in range(sections):
            a, b = r * (sections + 1) + s, (r + 1) * (sections + 1) + s
            indices += [[a, b, b + 1], [a, b + 1, a + 1]]
    return translate(Mesh(normals * radius, normals, np.array(indices)), at)


def torus(major: float, minor: float, sections: int = 48, around: int = 10) -> Mesh:
    """In the xz plane, about y."""
    u = np.linspace(0, 2 * np.pi, sections + 1)
    v = np.linspace(0, 2 * np.pi, around + 1)
    U, V = np.meshgrid(u, v, indexing="ij")
    centre = np.stack([np.cos(U), np.zeros_like(U), -np.sin(U)], -1)
    normals = centre * np.cos(V)[..., None] + np.array([0, 1, 0]) * np.sin(V)[..., None]
    positions = centre * major + normals * minor
    indices = []
    for i in range(sections):
        for j in range(around):
            a, b = i * (around + 1) + j, (i + 1) * (around + 1) + j
            indices += [[a, b, b + 1], [a, b + 1, a + 1]]
    return Mesh(positions.reshape(-1, 3), normals.reshape(-1, 3), np.array(indices))


def tube(path, radius: float, around: int = 8) -> Mesh:
    """A smooth tube through the points of `path`."""
    path = np.asarray(path, float)
    tangents = np.gradient(path, axis=0)
    tangents /= np.linalg.norm(tangents, axis=1, keepdims=True)
    reference = np.array([0, 1, 0]) if abs(tangents[0][1]) < 0.9 else np.array([1, 0, 0])
    positions, normals = [], []
    side = np.cross(tangents[0], reference)
    side /= np.linalg.norm(side)
    for point, tangent in zip(path, tangents):
        side = side - np.dot(side, tangent) * tangent
        side /= np.linalg.norm(side)
        up = np.cross(tangent, side)
        for a in np.linspace(0, 2 * np.pi, around, endpoint=False):
            n = np.cos(a) * side + np.sin(a) * up
            positions.append(point + n * radius)
            normals.append(n)
    indices = []
    for i in range(len(path) - 1):
        for j in range(around):
            a, b = i * around + j, i * around + (j + 1) % around
            indices += [[a, b, b + around], [a, b + around, a + around]]
    return Mesh(np.array(positions), np.array(normals), np.array(indices))


def gear(root: float, tip: float, teeth: int, depth: float, tooth: float = 0.5) -> Mesh:
    """A spur gear about y: `tooth` is the fraction of each pitch the tooth's top covers."""
    outline = []
    for i in range(teeth):
        a = 2 * np.pi * i / teeth
        pitch = 2 * np.pi / teeth
        for frac, r in ((0.0, root), (0.5 - tooth / 2 - 0.08, root), (0.5 - tooth / 2, tip), (0.5 + tooth / 2, tip), (0.5 + tooth / 2 + 0.08, root)):
            outline.append([r * np.cos(a + frac * pitch), -r * np.sin(a + frac * pitch)])
    return extrude(outline, depth)
