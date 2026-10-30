"""Install a converted puppet over the catalog file it replaces, refusing one that carries less.

usage: publish-avatar.py NEW.vrm CATALOG.vrm [--drop PREFIX ...]

Every node, morph target, material, spring joint, VRM expression and avatar expression part (parameter, layer, menu
control, material target, curve, shown node, mask) the catalog file carries must survive into the new one, or match a
--drop prefix naming it as an intended removal. The replaced file stays beside the catalog in .previous/."""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import tempfile


def document(data):
    size = struct.unpack_from('<I', data, 12)[0]
    return json.loads(data[20:20 + size])


def inventory(doc):
    nodes = doc.get('nodes', [])
    name = lambda index: nodes[index].get('name', str(index)) if isinstance(index, int) and 0 <= index < len(nodes) else str(index)
    out = Counter()
    out.update('extension:' + e for e in doc.get('extensionsUsed', []))
    out.update('node:' + n.get('name', '') for n in nodes)
    out.update('material:' + m.get('name', '') for m in doc.get('materials', []))
    for n in nodes:
        if 'mesh' not in n:
            continue
        mesh = doc['meshes'][n['mesh']]
        names = mesh.get('extras', {}).get('targetNames') or [str(i) for i in range(max((len(p.get('targets', [])) for p in mesh['primitives']), default=0))]
        out.update(f"morph:{n.get('name', '')}:{t}" for t in names)
    extensions = doc.get('extensions', {})
    springs = extensions.get('VRMC_springBone', {})
    out.update('spring-joint:' + name(j['node']) for s in springs.get('springs', []) for j in s['joints'])
    out.update('collider:' + name(c['node']) for c in springs.get('colliders', []))
    for group, entries in extensions.get('VRMC_vrm', {}).get('expressions', {}).items():
        out.update(f'vrm-expression:{group}:{e}' for e in entries)
    x = extensions.get('VRCHAT_expressions')
    if not x:
        return out
    out.update(f"parameter:{p['name']}:{p['type']}" for p in x.get('parameters', []))
    out.update('layer:' + '|'.join(s.get('name', '') for s in layer.get('states', [])) for layer in x.get('layers', []))

    def menu(controls, path):
        for c in controls:
            here = f"{path}/{c.get('name', '')}"
            out[f"menu:{here}:{c.get('type')}:{c.get('parameter', '')}"] += 1
            menu(c.get('controls', []), here)
    menu(x.get('menu', []), '')

    def target(t):
        effect = t['effect']
        if isinstance(effect, str):
            return effect
        kind, value = next(iter(effect.items()))
        parts = value if isinstance(value, list) else [value]
        return kind + ':' + ','.join(sorted({name(p['node']) for p in parts if isinstance(p, dict) and 'node' in p}))
    targets = [target(t) for t in x.get('targets', [])]
    out.update('target:' + t for t in targets)
    out.update('curve:' + (targets[c['target']] if c['target'] < len(targets) else str(c['target'])) for clip in x.get('clips', []) for c in clip.get('curves', []))
    out.update('shown:' + name(s['node']) for s in x.get('shown', []))
    out.update(f'icon:{i}' for i, _ in enumerate(x.get('icons', [])))
    out.update('expressions:' + k for k, v in x.items() if isinstance(v, str) and v)
    return out


def lost(old, new, drops):
    missing = old - new
    return sorted(k for k in missing.elements() if not any(k.startswith(d) for d in drops))


def publish(new_path, catalog_path, drops=()):
    data = Path(new_path).read_bytes()
    catalog = Path(catalog_path)
    if catalog.exists():
        gone = lost(inventory(document(catalog.read_bytes())), inventory(document(data)), drops)
        if gone:
            raise SystemExit(f'{new_path} would lose {len(gone)} part(s) {catalog} carries; name intended ones with --drop:\n' + '\n'.join('  ' + k for k in gone))
        previous = catalog.parent / '.previous'
        previous.mkdir(exist_ok=True)
        shutil.copy2(catalog, previous / catalog.name)
    with tempfile.NamedTemporaryFile(dir=catalog.parent, prefix='.' + catalog.name, delete=False) as out:
        out.write(data)
    os.replace(out.name, catalog)
    os.chmod(catalog, 0o644)
    return hashlib.sha256(data).hexdigest()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('new')
    parser.add_argument('catalog')
    parser.add_argument('--drop', action='append', default=[])
    args = parser.parse_args()
    print(publish(args.new, args.catalog, args.drop), args.catalog)
