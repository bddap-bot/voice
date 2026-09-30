import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile


def pack(data, scratch):
    magic, version, length = struct.unpack_from('<4sII', data)
    if (magic, version, length) != (b'glTF', 2, len(data)):
        raise ValueError('expected a binary glTF 2 file')
    size, kind = struct.unpack_from('<I4s', data, 12)
    if kind != b'JSON':
        raise ValueError('missing JSON chunk')
    doc = json.loads(data[20:20 + size])
    if not doc.get('images'):
        return data
    binary_size, kind = struct.unpack_from('<I4s', data, 20 + size)
    binary = data[28 + size:]
    if kind != b'BIN\0' or binary_size != len(binary) or len(doc['buffers']) != 1:
        raise ValueError('expected one embedded binary buffer')
    if any('uri' in image or 'bufferView' not in image for image in doc['images']):
        raise ValueError('all images must be embedded')
    views = [binary[v.get('byteOffset', 0):v.get('byteOffset', 0) + v['byteLength']] for v in doc['bufferViews']]
    meta = doc.get('extensions', {}).get('VRM', {}).get('meta', {})
    texture = meta.get('texture', -1)
    thumbnail = doc['textures'][texture].get('source') if texture >= 0 else doc.get('extensions', {}).get('VRMC_vrm', {}).get('meta', {}).get('thumbnailImage')
    image_views = [image['bufferView'] for image in doc['images']]
    if len(set(image_views)) != len(image_views):
        raise ValueError('images sharing a bufferView must be separated before packing')
    def material_textures(value):
        if isinstance(value, dict):
            for key, child in value.items():
                if key == 'index' and isinstance(child, int):
                    yield child
                else:
                    yield from material_textures(child)
        elif isinstance(value, list):
            for child in value:
                yield from material_textures(child)
    rendered = set(material_textures(doc.get('materials', [])))
    for material in doc.get('extensions', {}).get('VRM', {}).get('materialProperties', []):
        rendered.update(material.get('textureProperties', {}).values())
    for index in rendered:
        if index < 0:
            continue
        texture = doc['textures'][index]
        source = texture.get('source', texture.get('extensions', {}).get('EXT_texture_webp', {}).get('source'))
        if source is not None and source == thumbnail:
            raise ValueError('thumbnail is also rendered; export a separate thumbnail first')
    for index, entry in enumerate(doc['images']):
        payload = views[entry['bufferView']]
        thumb = index == thumbnail
        key = hashlib.sha256(payload + bytes([thumb])).hexdigest()
        source = scratch / (key + '.source')
        target = scratch / (key + ('.png' if thumb else '.webp'))
        if not target.exists():
            source.write_bytes(payload)
            if thumb:
                subprocess.run(['magick', str(source), '-filter', 'Lanczos', '-resize', '256x256>', 'PNG32:' + str(target)], check=True, capture_output=True)
                subprocess.run(['oxipng', '-o', '4', '--strip', 'safe', '-q', str(target)], check=True, capture_output=True)
            else:
                subprocess.run(['cwebp', '-quiet', '-exact', '-mt', '-lossless', '-z', '9', str(source), '-o', str(target)], check=True, capture_output=True)
        views[entry['bufferView']] = target.read_bytes()
        entry['mimeType'] = 'image/png' if thumb else 'image/webp'
    for texture in doc.get('textures', []):
        source = texture.get('source', texture.get('extensions', {}).get('EXT_texture_webp', {}).get('source'))
        if source is not None and doc['images'][source]['mimeType'] == 'image/webp':
            texture.pop('source', None)
            texture.setdefault('extensions', {})['EXT_texture_webp'] = {'source': source}
    if any(image['mimeType'] == 'image/webp' for image in doc['images']):
        for key in ('extensionsUsed', 'extensionsRequired'):
            doc[key] = sorted(set(doc.get(key, [])) | {'EXT_texture_webp'})
    removed = set()
    targets = {a for mesh in doc.get('meshes', []) for p in mesh['primitives'] for t in p.get('targets', []) for a in t.values()}
    for index in sorted(targets):
        accessor = doc['accessors'][index]
        if 'sparse' in accessor or 'bufferView' not in accessor:
            continue
        if accessor['componentType'] != 5126 or accessor['type'] != 'VEC3':
            raise ValueError('expected float VEC3 morph target')
        old = accessor['bufferView']
        view = doc['bufferViews'][old]
        offset, stride = accessor.get('byteOffset', 0), view.get('byteStride', 12)
        values = [views[old][offset + i * stride:offset + i * stride + 12] for i in range(accessor['count'])]
        moved = [i for i, value in enumerate(values) if value != bytes(12)]
        del accessor['bufferView']
        accessor.pop('byteOffset', None)
        removed.add(old)
        if moved:
            component, fmt = (5123, 'H') if max(moved) < 65536 else (5125, 'I')
            start = len(views)
            for payload in (struct.pack(f'<{len(moved)}{fmt}', *moved), b''.join(values[i] for i in moved)):
                views.append(payload)
                doc['bufferViews'].append({'buffer': 0, 'byteLength': len(payload)})
            accessor['sparse'] = {'count': len(moved), 'indices': {'bufferView': start, 'componentType': component}, 'values': {'bufferView': start + 1}}
    referenced = set()
    def references(value):
        if isinstance(value, dict):
            for key, child in value.items():
                if key == 'bufferView':
                    referenced.add(child)
                else:
                    references(child)
        elif isinstance(value, list):
            for child in value:
                references(child)
    references(doc)
    remap, layout, body = {}, [], bytearray()
    for index, payload in enumerate(views):
        if index in removed and index not in referenced:
            continue
        body += bytes(-len(body) % 4)
        remap[index] = len(layout)
        layout.append(dict(doc['bufferViews'][index], byteOffset=len(body), byteLength=len(payload)))
        body += payload
    def rewrite(value):
        if isinstance(value, dict):
            for key, child in value.items():
                if key == 'bufferView':
                    value[key] = remap[child]
                else:
                    rewrite(child)
        elif isinstance(value, list):
            for child in value:
                rewrite(child)
    rewrite(doc)
    body += bytes(-len(body) % 4)
    doc['bufferViews'] = layout
    doc['buffers'] = [{'byteLength': len(body)}]
    text = json.dumps(doc, separators=(',', ':')).encode()
    text += b' ' * (-len(text) % 4)
    return struct.pack('<4sII', b'glTF', 2, 28 + len(text) + len(body)) + struct.pack('<I4s', len(text), b'JSON') + text + struct.pack('<I4s', len(body), b'BIN\0') + body


def main():
    parser = argparse.ArgumentParser(description='Pack embedded VRM textures as exact lossless WebP, shrink the metadata thumbnail, and sparsify morphs. Replaces each input atomically.')
    parser.add_argument('models', nargs='+', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='.pack-puppets-', dir=Path.cwd()) as directory:
        for model in args.models:
            original = model.read_bytes()
            packed = pack(original, Path(directory))
            if packed != original:
                with tempfile.NamedTemporaryFile(dir=model.parent, prefix='.packing-', delete=False) as output:
                    pending = Path(output.name)
                    output.write(packed)
                try:
                    pending.chmod(model.stat().st_mode & 0o777)
                    pending.replace(model)
                finally:
                    pending.unlink(missing_ok=True)
            print(json.dumps({'file': str(model), 'before': len(original), 'after': len(packed), 'sha256': hashlib.sha256(packed).hexdigest()}), flush=True)


if __name__ == '__main__':
    main()
