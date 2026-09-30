import importlib.util
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('packer', Path(__file__).resolve().parents[1] / 'scripts/pack-puppets.py')
packer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packer)


def glb(doc, binary):
    doc['buffers'] = [{'byteLength': len(binary)}]
    text = json.dumps(doc).encode()
    text += b' ' * (-len(text) % 4)
    binary += bytes(-len(binary) % 4)
    return struct.pack('<4sII', b'glTF', 2, 28 + len(text) + len(binary)) + struct.pack('<I4s', len(text), b'JSON') + text + struct.pack('<I4s', len(binary), b'BIN\0') + binary


class Packing(unittest.TestCase):
    def test_untextured_is_untouched(self):
        source = glb({'asset': {'version': '2.0'}}, b'')
        self.assertEqual(packer.pack(source, Path('.')), source)

    def test_exact_pixels_sparse_morphs_and_extension(self):
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
            directory = Path(directory)
            png = directory / 'input.png'
            subprocess.run(['magick', '-size', '4x4', 'xc:rgba(100,50,200,0)', 'PNG32:' + str(png)], check=True)
            image = png.read_bytes()
            morph = struct.pack('<9f', 0, 0, 0, 1, -2, 3, -0.0, 0, 0)
            doc = {'asset': {'version': '2.0'}, 'bufferViews': [{'buffer': 0, 'byteOffset': 0, 'byteLength': len(image)}, {'buffer': 0, 'byteOffset': len(image), 'byteLength': len(morph)}], 'images': [{'bufferView': 0, 'mimeType': 'image/png'}], 'textures': [{'source': 0}], 'accessors': [{'bufferView': 1, 'componentType': 5126, 'count': 3, 'type': 'VEC3'}], 'meshes': [{'primitives': [{'targets': [{'POSITION': 0}]}]}]}
            packed = packer.pack(glb(doc, image + morph), directory)
            size = struct.unpack_from('<I', packed, 12)[0]
            result = json.loads(packed[20:20 + size])
            binary = packed[28 + size:]
            def view(index):
                v = result['bufferViews'][index]
                return binary[v['byteOffset']:v['byteOffset'] + v['byteLength']]
            self.assertEqual(result['textures'], [{'extensions': {'EXT_texture_webp': {'source': 0}}}])
            self.assertIn('EXT_texture_webp', result['extensionsRequired'])
            webp = view(result['images'][0]['bufferView'])
            self.assertIn(b'VP8L', webp)
            output = directory / 'output.webp'
            output.write_bytes(webp)
            decode = lambda path: subprocess.check_output(['magick', str(path), '-depth', '8', 'rgba:-'])
            self.assertEqual(decode(png), decode(output))
            accessor = result['accessors'][0]
            self.assertNotIn('bufferView', accessor)
            sparse = accessor['sparse']
            self.assertEqual(sparse['count'], 2)
            restored = bytearray(36)
            for j, index in enumerate(struct.unpack('<2H', view(sparse['indices']['bufferView']))):
                restored[index * 12:(index + 1) * 12] = view(sparse['values']['bufferView'])[j * 12:(j + 1) * 12]
            self.assertEqual(restored, morph)


unittest.main()
