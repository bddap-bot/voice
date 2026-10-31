import copy
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('publisher', Path(__file__).resolve().parents[1] / 'scripts/publish-avatar.py')
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


def glb(doc):
    text = json.dumps(doc).encode()
    text += b' ' * (-len(text) % 4)
    return struct.pack('<4sII', b'glTF', 2, 20 + len(text)) + struct.pack('<I4s', len(text), b'JSON') + text


AVATAR = {
    'asset': {'version': '2.0'},
    'extensionsUsed': ['VRCHAT_expressions'],
    'nodes': [{'name': 'Root', 'children': [1, 2, 3]}, {'name': 'Body', 'mesh': 0}, {'name': 'Hair'}, {'name': 'Head'}],
    'images': [{'name': 'atlas', 'mimeType': 'image/webp', 'bufferView': 0}],
    'meshes': [{'primitives': [{'attributes': {}, 'targets': [{}, {}]}], 'extras': {'targetNames': ['smile', 'blink']}}],
    'extensions': {'VRMC_springBone': {
        'colliders': [{'node': 3, 'shape': {'sphere': {'radius': 0.1}}}],
        'colliderGroups': [{'colliders': [0]}],
        'springs': [{'joints': [{'node': 2}], 'colliderGroups': [0]}],
    }, 'VRCHAT_expressions': {
        'parameters': [{'name': 'Hue', 'type': 'float', 'default': 0, 'saved': True}],
        'layers': [{'states': [{'name': 'Hue'}]}],
        'menu': [{'name': 'Hue', 'type': 'radial', 'parameter': 'Hue', 'controls': []}],
        'targets': [{'default': 0, 'effect': {'hue': [{'node': 1, 'rect': [0, 0, 1, 1]}]}}],
        'clips': [{'curves': [{'target': 0, 'keys': [[0, 0], [1, 1]]}]}],
        'hueMask': 'iVBOR',
        'springs': [{'joints': [2], 'when': [[0, 1]]}],
        'colliders': [{'collider': 0, 'when': [[0, 1]]}],
    }},
}


class Publishing(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=Path.cwd())
        self.root = Path(self.directory.name)
        self.catalog = self.root / 'avatar.vrm'
        self.catalog.write_bytes(glb(AVATAR))

    def tearDown(self):
        self.directory.cleanup()

    def candidate(self, change):
        doc = copy.deepcopy(AVATAR)
        change(doc)
        path = self.root / 'new.vrm'
        path.write_bytes(glb(doc))
        return path

    def test_a_file_losing_a_material_target_is_refused(self):
        def strip(doc):
            x = doc['extensions']['VRCHAT_expressions']
            x['targets'] = []
            x['clips'] = [{'curves': []}]
            x['layers'] = []
            del x['hueMask']
        before = self.catalog.read_bytes()
        with self.assertRaises(SystemExit) as refused:
            publisher.publish(self.candidate(strip), self.catalog)
        for part in ['target:hue:Body', 'curve:hue:Body', 'layer:Hue', 'expressions:hueMask']:
            self.assertIn(part, str(refused.exception))
        self.assertEqual(self.catalog.read_bytes(), before)

    def test_a_decal_effect_carried_as_a_look_is_kept(self):
        def as_decal(doc):
            doc['extensions']['VRCHAT_expressions']['targets'][0]['effect'] = {'decal': [{'node': 1, 'rect': [0, 0, 1, 1], 'slot': 0}]}
        self.catalog.write_bytes(self.candidate(as_decal).read_bytes())

        def as_look(doc):
            doc['extensions']['VRCHAT_expressions']['targets'][0]['effect'] = {'look': [{'field': 'decal', 'node': 1, 'rect': [0, 0, 1, 1], 'slot': 0}]}
        publisher.publish(self.candidate(as_look), self.catalog)

    def test_an_effect_or_look_field_the_overlay_does_not_read_is_refused(self):
        look = lambda field: {'node': 1, 'rect': [0, 0, 1, 1], 'field': field, 'slot': 0}
        for effect, part in [({'glow': [{'node': 1}]}, 'effect:glow'), ('blink', 'effect:blink'), ({'look': [look('decal'), look('sparkle')]}, 'look:sparkle')]:
            before = self.catalog.read_bytes()
            with self.assertRaises(SystemExit) as refused:
                publisher.publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions']['targets'].append({'default': 0, 'effect': effect})), self.catalog)
            self.assertIn(part, str(refused.exception))
            self.assertEqual(self.catalog.read_bytes(), before)

    def test_every_look_field_and_effect_the_overlay_lists_publishes(self):
        targets = [{'effect': kind if kind == 'switch' else {kind: []}} for kind in publisher.READ['effects']]
        targets.append({'effect': {'look': [{'node': 1, 'rect': [0, 0, 1, 1], 'field': field, 'slot': 0} for field in publisher.READ['looks']]}})
        publisher.publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions']['targets'].extend(targets)), self.catalog)

    def test_a_lost_parameter_menu_control_morph_image_or_collider_is_refused(self):
        for change, part in [
            (lambda doc: doc['extensions']['VRCHAT_expressions'].update(parameters=[]), 'parameter:Hue:float'),
            (lambda doc: doc['extensions']['VRCHAT_expressions'].update(menu=[]), 'menu:/Hue:radial:Hue'),
            (lambda doc: doc['meshes'][0]['extras'].update(targetNames=['smile']), 'morph:Body:blink'),
            (lambda doc: doc.update(images=[]), 'image'),
            (lambda doc: doc['extensions']['VRMC_springBone'].update(colliders=[], colliderGroups=[{'colliders': []}]), 'collider:Head'),
            (lambda doc: doc['extensions']['VRMC_springBone']['springs'][0].update(colliderGroups=[]), 'spring-collider:Hair:Head'),
            (lambda doc: doc['extensions']['VRCHAT_expressions'].update(springs=[]), 'switched-spring:Hair'),
            (lambda doc: doc['extensions']['VRCHAT_expressions'].update(colliders=[]), 'switched-collider:Head'),
        ]:
            with self.assertRaises(SystemExit) as refused:
                publisher.publish(self.candidate(change), self.catalog)
            self.assertIn(part, str(refused.exception))

    def test_a_superset_installs_and_keeps_the_replaced_file(self):
        before = self.catalog.read_bytes()
        new = self.candidate(lambda doc: doc['extensionsUsed'].append('VRMC_vrm'))
        publisher.publish(new, self.catalog)
        self.assertEqual(self.catalog.read_bytes(), new.read_bytes())
        self.assertEqual((self.root / '.previous' / 'avatar.vrm').read_bytes(), before)

    def test_a_named_removal_installs(self):
        new = self.candidate(lambda doc: doc['meshes'][0]['extras'].update(targetNames=['smile']))
        publisher.publish(new, self.catalog, ['morph:Body:blink'])
        self.assertEqual(self.catalog.read_bytes(), new.read_bytes())

    def test_a_new_catalog_entry_installs(self):
        new = self.candidate(lambda doc: None)
        target = self.root / 'fresh.vrm'
        publisher.publish(new, target)
        self.assertEqual(target.read_bytes(), new.read_bytes())


if __name__ == '__main__':
    unittest.main()
