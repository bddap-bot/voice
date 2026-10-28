"""Extract Mesh2Motion's CC0 Walk in the overlay's normalized humanoid format.

Run from the repository root: python3 vr/motions/import-walk.py
Only the rest coordinate frames change; key times and animation rotations remain
authored. The runtime scales hips translation to the avatar's rest hips height.
"""
import hashlib
import json
import math
from pathlib import Path
import struct
import urllib.request

REVISION = "eef2de986dab53283cb636f860d502d64b8073e7"
SOURCE = f"https://raw.githubusercontent.com/mesh2motion/mesh2motion-app/{REVISION}/static/animations/human-base-animations.glb"
RIG = {
    "pelvis": "hips", "spine_01": "spine", "spine_02": "chest",
    "spine_03": "upperChest", "neck_01": "neck", "head": "head",
}
for suffix, side in [("l", "left"), ("r", "right")]:
    for source, target in [("clavicle", "Shoulder"), ("upperarm", "UpperArm"),
                           ("lowerarm", "LowerArm"), ("hand", "Hand"),
                           ("thigh", "UpperLeg"), ("calf", "LowerLeg"),
                           ("foot", "Foot"), ("ball", "Toes")]:
        RIG[f"{source}_{suffix}"] = side + target


def multiply(a, b):
    x, y, z, w = a
    X, Y, Z, W = b
    return [w*X+x*W+y*Z-z*Y, w*Y-x*Z+y*W+z*X,
            w*Z+x*Y-y*X+z*W, w*W-x*X-y*Y-z*Z]


def inverse(q):
    return [-q[0], -q[1], -q[2], q[3]]


def rotate(q, v):
    return multiply(multiply(q, [*v, 0]), inverse(q))[:3]


def convert(data):
    length = struct.unpack_from("<I", data, 12)[0]
    gltf = json.loads(data[20:20+length])
    binary = data[28+length:]
    nodes = gltf["nodes"]
    parents = {child: i for i, node in enumerate(nodes) for child in node.get("children", [])}

    def world(i):
        local = nodes[i].get("rotation", [0, 0, 0, 1])
        return multiply(world(parents[i]), local) if i in parents else local

    def accessor(i):
        entry = gltf["accessors"][i]
        assert entry["componentType"] == 5126 and "sparse" not in entry
        view = gltf["bufferViews"][entry["bufferView"]]
        width = {"SCALAR": 1, "VEC3": 3, "VEC4": 4}[entry["type"]]
        offset = view.get("byteOffset", 0) + entry.get("byteOffset", 0)
        stride = view.get("byteStride", 4 * width)
        return [list(struct.unpack_from("<" + "f" * width, binary, offset + i * stride)) for i in range(entry["count"])]

    animation = next(a for a in gltf["animations"] if a["name"] == "Walk")
    tracks = []
    for channel in animation["channels"]:
        node = channel["target"]["node"]
        bone = RIG.get(nodes[node]["name"])
        prop = channel["target"]["path"]
        if not bone or not (prop == "rotation" or (bone == "hips" and prop == "translation")):
            continue
        sampler = animation["samplers"][channel["sampler"]]
        times = [t[0] for t in accessor(sampler["input"])]
        values = accessor(sampler["output"])
        assert sampler.get("interpolation", "LINEAR") == "LINEAR" or all(v == values[0] for v in values)
        parent = world(parents[node])
        if prop == "rotation":
            rest = inverse(world(node))
            values = [multiply(multiply(parent, q), rest) for q in values]
            values = [[v / math.sqrt(sum(x*x for x in q)) for v in q] for q in values]
            prop = "quaternion"
        else:
            values = [rotate(parent, v) for v in values]
            hips_height = rotate(parent, nodes[node]["translation"])[1]
            prop = "position"
        tracks.append({"name": f"{bone}.{prop}", "times": times, "values": [v for row in values for v in row]})
    return {"name": "Walk", "source": SOURCE, "license": "CC0-1.0",
            "duration": max(t["times"][-1] for t in tracks), "hipsHeight": hips_height, "tracks": tracks}


if __name__ == "__main__":
    with urllib.request.urlopen(SOURCE, timeout=60) as response:
        data = response.read()
    result = convert(data)
    result["sourceSha256"] = hashlib.sha256(data).hexdigest()
    Path(__file__).with_name("walk.json").write_text(json.dumps(result, separators=(",", ":")) + "\n")
