"""The Sun and the planets out to Saturn on their true periods and eccentric orbits, ten seconds to a year; live data sets the date."""
import datetime

import numpy as np

import shapes
from glb import Node

# Name, colour, semi-major axis in AU, eccentricity, longitude of perihelion and mean longitude at J2000 in degrees, sidereal period in years, drawn radius in metres.
PLANETS = [
    ("mercury", (0.62, 0.6, 0.58, 1), 0.387, 0.2056, 77.46, 252.25, 0.2408467, 0.0026),
    ("venus", (0.95, 0.82, 0.5, 1), 0.723, 0.0068, 131.53, 181.98, 0.61519726, 0.0040),
    ("earth", (0.25, 0.55, 0.95, 1), 1.000, 0.0167, 102.94, 100.46, 1.0000174, 0.0042),
    ("mars", (0.9, 0.36, 0.2, 1), 1.524, 0.0934, 336.04, 355.45, 1.8808476, 0.0034),
    ("jupiter", (0.86, 0.66, 0.48, 1), 5.203, 0.0484, 14.75, 34.40, 11.862615, 0.0085),
    ("saturn", (0.92, 0.84, 0.6, 1), 9.537, 0.0539, 92.43, 49.94, 29.447498, 0.0072),
]
OUTER = 0.105
SECONDS_PER_YEAR = 10.0
DURATION = 120.0
# Toward the wearer, so the ecliptic reads as a plane rather than a line.
TILT = 20.0


def distance(au: float) -> float:
    """Distances drawn at their square root, so Mercury and Saturn fit one object."""
    return OUTER * np.sqrt(au / PLANETS[-1][2])


def orbit(a: float, e: float, perihelion: float):
    true = np.linspace(0, 2 * np.pi, 160)
    r = a * (1 - e * e) / (1 + e * np.cos(true))
    angle = true + np.radians(perihelion)
    return np.stack([distance(r) * np.cos(angle), np.zeros_like(r), -distance(r) * np.sin(angle)], 1)


def position(a: float, e: float, perihelion: float, longitude: float, period: float, years: float) -> tuple[float, float, float]:
    """Where the program puts a planet `years` after J2000, so the file at rest shows its build date."""
    mean = np.radians(longitude) + years / period * 2 * np.pi - np.radians(perihelion)
    true = mean + (2 * e - e**3 / 4) * np.sin(mean) + 1.25 * e * e * np.sin(2 * mean) + 13 / 12 * e**3 * np.sin(3 * mean)
    r = distance(a * (1 - e * e) / (1 + e * np.cos(true)))
    angle = true + np.radians(perihelion)
    return (float(r * np.cos(angle)), 0.0, float(-r * np.sin(angle)))


def scene():
    years = ((datetime.date.today() - datetime.date(2000, 1, 1)).days - 0.5) / 365.25
    rings = shapes.merge(*[shapes.tube(orbit(a, e, w), 0.0004, 4) for _, _, a, e, w, *_ in PLANETS])
    bodies = [Node("sun", shapes.sphere(0.009, 16, 24), (1.0, 0.78, 0.25, 1)), Node("orbits", rings, (0.5, 0.53, 0.6, 1))]
    for name, colour, a, e, perihelion, longitude, period, radius in PLANETS:
        planet = Node(name, shapes.sphere(radius, 12, 18), colour, translation=position(a, e, perihelion, longitude, period, years))
        if name == "earth":
            planet.children.append(Node("moon", shapes.sphere(0.0015, 8, 12), (0.8, 0.8, 0.78, 1), translation=(0.008, 0.0, 0.0)))
        if name == "saturn":
            ring = shapes.transform(shapes.torus(0.0118, 0.0032, 48, 8), np.diag([1.0, 0.08, 1.0, 1.0]))
            planet.children.append(Node("saturn_rings", shapes.rotate(ring, (1, 0, 0), np.radians(27)), (0.85, 0.78, 0.6, 0.7)))
        bodies.append(planet)
    roots = [Node("orrery", children=bodies, rotation=shapes.quaternion((1, 0, 0), np.radians(TILT)))]
    constants = {"BUILT_YEARS": years, "SECONDS_PER_YEAR": SECONDS_PER_YEAR, "OUTER": OUTER, "OUTER_AU": PLANETS[-1][2], "MOON_DISTANCE": 0.008, "MOON_PERIOD": 27.321661 / 365.25, "MOON_J2000": np.radians(218.32)}
    for name, _, a, e, perihelion, longitude, period, _ in PLANETS:
        prefix = name.upper()
        constants |= {f"{prefix}_AU": a, f"{prefix}_E": e, f"{prefix}_PERIHELION": np.radians(perihelion), f"{prefix}_J2000": np.radians(longitude), f"{prefix}_PERIOD": period}
    return roots, DURATION, constants
