"""Install a converted puppet over the catalog file it replaces, refusing one that carries less.

usage: publish-avatar.py NEW.vrm CATALOG.vrm [--converter DIR] [--drop PREFIX ...] [--unlanded] [--rollback] [--clips DIR] [--out PATH]

Every node, image, morph target, material, spring joint, collider a spring collides with, VRM expression and avatar
expression part (parameter, layer, menu control, material target, curve, shown node, switched spring or collider,
mask) the catalog file carries must survive into the new one, or match a --drop prefix naming it as an intended
removal. A target effect or look field the overlay's vr/src/vrm/effects.json does not list is refused: the overlay
would leave it still. So is a look without its material's lighting, which the overlay refuses to load. The installed
file carries VOICE_arms, the arm spread `voice-vr arms` (the binary $VOICE_VR names, else voice-vr on PATH) measures on
its body against the clips the server plays; the overlay refuses a figure without it.
The replaced file stays beside the catalog in .previous/, unless NEW is CATALOG: republishing a file in place re-measures it.
Otherwise NEW must name in its asset extras the clean converter commit that wrote it, one origin/main of DIR, the
converter's repository, contains once fetched, unless --unlanded names it an intended exception; and that commit may
not be a strict ancestor of the one the catalog file names, or one DIR cannot compare with it, unless --rollback names
it an intended rollback.
--clips measures the spread against the clips the server would serve from that clip directory instead, and --out writes
the result there instead of over CATALOG, so a clip write can re-measure the catalog before it installs its clips."""
import argparse
from collections import Counter
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import struct
import subprocess
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
    out['image'] += len(doc.get('images', []))
    for n in nodes:
        if 'mesh' not in n:
            continue
        mesh = doc['meshes'][n['mesh']]
        names = mesh.get('extras', {}).get('targetNames') or [str(i) for i in range(max((len(p.get('targets', [])) for p in mesh['primitives']), default=0))]
        out.update(f"morph:{n.get('name', '')}:{t}" for t in names)
    extensions = doc.get('extensions', {})
    springs = extensions.get('VRMC_springBone', {})
    out.update('spring-joint:' + name(j['node']) for s in springs.get('springs', []) for j in s['joints'])
    colliders = springs.get('colliders', [])
    groups = springs.get('colliderGroups', [])
    out.update('collider:' + name(c['node']) for c in colliders)
    out.update(f"spring-collider:{name(s['joints'][0]['node'])}:{name(colliders[c]['node'])}"
               for s in springs.get('springs', []) if s.get('joints')
               for g in s.get('colliderGroups', []) for c in groups[g].get('colliders', []))
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
        # The format carries a decal as a look of field "decal", which earlier files called a decal effect.
        kind = 'look' if kind == 'decal' else kind
        parts = value if isinstance(value, list) else [value]
        return kind + ':' + ','.join(sorted({name(p['node']) for p in parts if isinstance(p, dict) and 'node' in p}))
    targets = [target(t) for t in x.get('targets', [])]
    out.update('target:' + t for t in targets)
    out.update('curve:' + (targets[c['target']] if c['target'] < len(targets) else str(c['target'])) for clip in x.get('clips', []) for c in clip.get('curves', []))
    out.update('shown:' + name(s['node']) for s in x.get('shown', []))
    out.update('switched-spring:' + ','.join(name(j) for j in s['joints']) for s in x.get('springs', []))
    out.update('switched-collider:' + (name(colliders[s['collider']]['node']) if s['collider'] < len(colliders) else str(s['collider'])) for s in x.get('colliders', []))
    out.update(f'icon:{i}' for i, _ in enumerate(x.get('icons', [])))
    out.update('expressions:' + k for k, v in x.items() if isinstance(v, str) and v)
    return out


READ = json.loads((Path(__file__).resolve().parents[1] / 'vr/src/vrm/effects.json').read_text())


ARMS = 'VOICE_arms'


def measure(path, clips=None):
    run = subprocess.run([os.environ.get('VOICE_VR', 'voice-vr'), 'arms', *(['--clips', str(clips)] if clips else []), str(path)], check=True, stdout=subprocess.PIPE, text=True)
    return json.loads(run.stdout)


def with_arms(data, spread):
    size = struct.unpack_from('<I', data, 12)[0]
    doc = document(data)
    doc.setdefault('extensions', {})[ARMS] = spread
    doc['extensionsUsed'] = sorted(set(doc.get('extensionsUsed', [])) | {ARMS})
    text = json.dumps(doc, separators=(',', ':')).encode()
    text += b' ' * (-len(text) % 4)
    rest = data[20 + size:]
    return struct.pack('<4sII', b'glTF', 2, 20 + len(text) + len(rest)) + struct.pack('<I4s', len(text), b'JSON') + text + rest


def unread(doc):
    out = set()
    for t in doc.get('extensions', {}).get('VRCHAT_expressions', {}).get('targets', []):
        effect = t['effect']
        kind = effect if isinstance(effect, str) else next(iter(effect))
        if kind not in READ['effects']:
            out.add('effect:' + kind)
        elif kind == 'look':
            out.update('look:' + r.get('field', '') for r in effect[kind] if r.get('field') not in READ['looks'])
    return sorted(out)


def stamp_of(doc):
    extras = doc.get('asset', {}).get('extras')
    stamp = extras.get('converter') if isinstance(extras, dict) else None
    return stamp if isinstance(stamp, dict) else {}


def git(repo, *args):
    return subprocess.run(['git', '-C', str(repo), *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode


def unlanded(stamp, repo):
    commit = stamp.get('commit')
    if not isinstance(commit, str) or not re.fullmatch('[0-9a-f]{40}|[0-9a-f]{64}', commit):
        return 'names no converter commit'
    if stamp.get('dirty') is not False:
        return f'was converted from a tree that differs from {commit}'
    if git(repo, 'merge-base', '--is-ancestor', commit, 'origin/main'):
        return f"names converter commit {commit}, which {repo}'s origin/main does not contain"


def lost(old, new, drops):
    missing = old - new
    return sorted(k for k in missing.elements() if not any(k.startswith(d) for d in drops))


def provenance(new_path, catalog, repo, unlanded_ok, rollback_ok):
    if repo is None:
        raise SystemExit(f'name the repository of the converter that wrote {new_path} with --converter')
    if git(repo, 'fetch', '--quiet', 'origin'):
        raise SystemExit(f'cannot fetch origin in {repo}')
    stamp = stamp_of(document(Path(new_path).read_bytes()))
    commit = stamp.get('commit')
    why = unlanded(stamp, repo)
    if why and not unlanded_ok:
        raise SystemExit(f'{new_path} {why}; convert it from the converter\'s main, or name the exception with --unlanded')
    replaced = stamp_of(document(catalog.read_bytes())).get('commit') if catalog.exists() else None
    if rollback_ok or not replaced or commit == replaced:
        return
    older = git(repo, 'merge-base', '--is-ancestor', str(commit), str(replaced))
    if older == 0:
        raise SystemExit(f"{new_path}'s converter commit {commit} is older than {replaced}, which wrote {catalog}; convert it from the converter's main, or name the rollback with --rollback")
    if older != 1:
        raise SystemExit(f"{repo} cannot tell whether {new_path}'s converter commit {commit} is older than {replaced}, which wrote {catalog}; name an intended rollback with --rollback")


def publish(new_path, catalog_path, repo=None, drops=(), unlanded_ok=False, rollback_ok=False, clips=None, out_path=None):
    data = Path(new_path).read_bytes()
    catalog = Path(catalog_path)
    in_place = catalog.exists() and catalog.samefile(new_path)
    if not in_place:
        provenance(new_path, catalog, repo, unlanded_ok, rollback_ok)
    unknown = unread(document(data))
    if unknown:
        raise SystemExit(f'{new_path} carries expression parts the overlay does not read; teach voice-vr them and list them in vr/src/vrm/effects.json:\n' + '\n'.join('  ' + k for k in unknown))
    looks = document(data).get('extensions', {}).get('VRCHAT_expressions', {}).get('looks', {}).get('materials', [])
    unlit = [i for i, look in enumerate(looks) if 'lighting' not in look]
    if unlit:
        raise SystemExit(f'{new_path} has looks without the lighting the overlay reads (materials {unlit}); reconvert it with the current converter')
    data = with_arms(data, measure(new_path, clips))
    if catalog.exists():
        gone = lost(inventory(document(catalog.read_bytes())), inventory(document(data)), drops)
        if gone:
            raise SystemExit(f'{new_path} would lose {len(gone)} part(s) {catalog} carries; name intended ones with --drop:\n' + '\n'.join('  ' + k for k in gone))
        if not in_place:
            previous = catalog.parent / '.previous'
            previous.mkdir(exist_ok=True)
            shutil.copy2(catalog, previous / catalog.name)
    target = Path(out_path) if out_path else catalog
    with tempfile.NamedTemporaryFile(dir=target.parent, prefix='.' + target.name, delete=False) as out:
        out.write(data)
    os.replace(out.name, target)
    os.chmod(target, 0o644)
    return hashlib.sha256(data).hexdigest()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('new')
    parser.add_argument('catalog')
    parser.add_argument('--converter')
    parser.add_argument('--drop', action='append', default=[])
    parser.add_argument('--unlanded', action='store_true')
    parser.add_argument('--rollback', action='store_true')
    parser.add_argument('--clips')
    parser.add_argument('--out')
    args = parser.parse_args()
    print(publish(args.new, args.catalog, args.converter, args.drop, args.unlanded, args.rollback, args.clips, args.out), args.out or args.catalog)
