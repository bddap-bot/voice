import copy
import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('publisher', Path(__file__).resolve().parents[1] / 'scripts/publish-avatar.py')
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


CONVERTER = tempfile.TemporaryDirectory()
REPO = Path(CONVERTER.name) / 'converter'
os.environ.update(GIT_CONFIG_COUNT='1', GIT_CONFIG_KEY_0='core.hooksPath', GIT_CONFIG_VALUE_0='/dev/null', GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t', GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t')


def git(*args):
    return subprocess.run(['git', '-C', str(REPO), *args], check=True, stdout=subprocess.PIPE, text=True).stdout.strip()


subprocess.run(['git', 'init', '--quiet', '--bare', '--initial-branch=main', str(REPO.with_name('origin'))], check=True)
subprocess.run(['git', 'clone', '--quiet', str(REPO.with_name('origin')), str(REPO)], check=True, stderr=subprocess.DEVNULL)
for _ in range(3):
    git('commit', '--quiet', '--allow-empty', '-m', 'c')
    git('push', '--quiet', 'origin', 'HEAD:main')
OLDER, LANDED = git('rev-parse', 'HEAD~1'), git('rev-parse', 'HEAD')
git('commit', '--quiet', '--allow-empty', '-m', 'unlanded')
UNLANDED = git('rev-parse', 'HEAD')
git('reset', '--quiet', '--hard', LANDED)


def publish(new, catalog, drops=(), **flags):
    return publisher.publish(new, catalog, REPO, drops, **flags)


def stamped(commit, dirty=False):
    return lambda doc: doc['asset'].update(extras={'converter': {'commit': commit, 'dirty': dirty}})


def glb(doc, spelled=lambda text: text):
    text = spelled(json.dumps(doc)).encode()
    text += b' ' * (-len(text) % 4)
    return struct.pack('<4sII', b'glTF', 2, 20 + len(text)) + struct.pack('<I4s', len(text), b'JSON') + text


AVATAR = {
    'asset': {'version': '2.0', 'extras': {'converter': {'commit': LANDED, 'dirty': False}}},
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
        'hueMask': 0,
        'springs': [{'joints': [2], 'when': [[0, 1]]}],
        'colliders': [{'collider': 0, 'when': [[0, 1]]}],
    }},
}


SPREAD = {'angle': 0.1, 'body': 'b', 'clips': 'c', 'revision': 1}
measured = []
publisher.measure = lambda name, data, clips=None: measured.append(data) or SPREAD


def without_arms(data):
    doc = publisher.document(data)
    assert doc['extensions'].pop(publisher.ARMS) == SPREAD
    doc['extensionsUsed'].remove(publisher.ARMS)
    return doc


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
            publish(self.candidate(strip), self.catalog)
        for part in ['target:hue:Body', 'curve:hue:Body', 'layer:Hue', 'expressions:hueMask']:
            self.assertIn(part, str(refused.exception))
        self.assertEqual(self.catalog.read_bytes(), before)

    def test_untinted_colours_moved_from_a_whole_atlas_to_their_cells_are_kept(self):
        self.catalog.write_bytes(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions'].update(colorAtlas='iVBOR')).read_bytes())
        publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions'].update(untinted={'image': 0, 'cells': [[0, 0, 1, 1, 0, 0, 1, 1]]})), self.catalog)
        for old in [{'colorAtlas': 'iVBOR'}, {'untinted': {'image': 0, 'cells': [[0, 0, 1, 1, 0, 0, 1, 1]]}}]:
            self.catalog.write_bytes(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions'].update(old)).read_bytes())
            with self.assertRaises(SystemExit) as refused:
                publish(self.candidate(lambda doc: None), self.catalog)
            self.assertIn('expressions:untinted', str(refused.exception))

    def test_a_decal_effect_carried_as_a_look_is_kept(self):
        def as_decal(doc):
            doc['extensions']['VRCHAT_expressions']['targets'][0]['effect'] = {'decal': [{'node': 1, 'rect': [0, 0, 1, 1], 'slot': 0}]}
        self.catalog.write_bytes(self.candidate(as_decal).read_bytes())

        def as_look(doc):
            doc['extensions']['VRCHAT_expressions']['targets'][0]['effect'] = {'look': [{'field': 'decal', 'node': 1, 'rect': [0, 0, 1, 1], 'slot': 0}]}
        publish(self.candidate(as_look), self.catalog)

    def test_an_effect_or_look_field_the_overlay_does_not_read_is_refused(self):
        look = lambda field: {'node': 1, 'rect': [0, 0, 1, 1], 'field': field, 'slot': 0}
        for effect, part in [({'glow': [{'node': 1}]}, 'effect:glow'), ('blink', 'effect:blink'), ({'look': [look('decal'), look('sparkle')]}, 'look:sparkle')]:
            before = self.catalog.read_bytes()
            with self.assertRaises(SystemExit) as refused:
                publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions']['targets'].append({'default': 0, 'effect': effect})), self.catalog)
            self.assertIn(part, str(refused.exception))
            self.assertEqual(self.catalog.read_bytes(), before)

    def test_a_look_without_its_lighting_is_refused(self):
        look = {'decals': [None] * 4, 'matcaps': [None] * 4, 'emission': None}
        before = self.catalog.read_bytes()
        with self.assertRaises(SystemExit) as refused:
            publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions'].update(looks={'index': 0, 'materials': [look]})), self.catalog)
        self.assertIn('lighting', str(refused.exception))
        self.assertEqual(self.catalog.read_bytes(), before)
        publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions'].update(looks={'index': 0, 'materials': [look | {'lighting': {'model': 'flat'}}]})), self.catalog)

    def test_every_look_field_and_effect_the_overlay_lists_publishes(self):
        targets = [{'effect': kind if kind == 'switch' else {kind: []}} for kind in publisher.READ['effects']]
        targets.append({'effect': {'look': [{'node': 1, 'rect': [0, 0, 1, 1], 'field': field, 'slot': 0} for field in publisher.READ['looks']]}})
        publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions']['targets'].extend(targets)), self.catalog)

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
                publish(self.candidate(change), self.catalog)
            self.assertIn(part, str(refused.exception))

    def test_a_target_s_extra_curves_may_go_but_its_last_may_not(self):
        x = AVATAR['extensions']['VRCHAT_expressions']
        self.catalog.write_bytes(glb({**AVATAR, 'extensions': {**AVATAR['extensions'], 'VRCHAT_expressions': {**x, 'clips': x['clips'] + [{'curves': [{'target': 0, 'keys': [[0, 1]]}]}]}}}))
        publish(self.candidate(lambda doc: None), self.catalog)
        with self.assertRaises(SystemExit) as refused:
            publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions'].update(clips=[{'curves': []}])), self.catalog)
        self.assertIn('curve:hue:Body', str(refused.exception))
        twin = lambda doc: doc['extensions']['VRCHAT_expressions'].update(targets=x['targets'] * 2, clips=[{'curves': [{'target': 0, 'keys': [[0, 1]]}, {'target': 1, 'keys': [[0, 1]]}]}])
        self.catalog.write_bytes(self.candidate(twin).read_bytes())
        with self.assertRaises(SystemExit) as refused:
            publish(self.candidate(lambda doc: doc['extensions']['VRCHAT_expressions'].update(targets=x['targets'] * 2)), self.catalog)
        self.assertIn('curve:hue:Body', str(refused.exception))

    def test_a_superset_installs_and_keeps_the_replaced_file(self):
        before = self.catalog.read_bytes()
        new = self.candidate(lambda doc: doc['extensionsUsed'].append('VRMC_vrm'))
        publish(new, self.catalog)
        self.assertEqual(without_arms(self.catalog.read_bytes()), publisher.document(new.read_bytes()))
        self.assertEqual((self.root / '.previous' / 'avatar.vrm').read_bytes(), before)

    def test_republishing_in_place_re_measures_it_and_keeps_the_replaced_file(self):
        before = self.catalog.read_bytes()
        publish(self.candidate(lambda doc: doc['extensionsUsed'].append('VRMC_vrm')), self.catalog)
        count = len(measured)
        publish(self.catalog, self.catalog)
        self.assertEqual(len(measured), count + 1)
        self.assertEqual((self.root / '.previous' / 'avatar.vrm').read_bytes(), before)
        self.catalog.write_bytes(self.candidate(lambda doc: doc['asset'].pop('extras')).read_bytes())
        publisher.publish(self.catalog, self.catalog)
        self.assertEqual(len(measured), count + 2)

    def test_a_fresh_conversion_installs_over_a_catalog_file_carrying_its_spread(self):
        publish(self.candidate(lambda doc: None), self.catalog)
        new = self.candidate(lambda doc: None)
        publish(new, self.catalog)
        self.assertEqual(without_arms(self.catalog.read_bytes()), publisher.document(new.read_bytes()))

    def test_a_named_removal_installs(self):
        new = self.candidate(lambda doc: doc['meshes'][0]['extras'].update(targetNames=['smile']))
        publish(new, self.catalog, ['morph:Body:blink'])
        self.assertEqual(without_arms(self.catalog.read_bytes()), publisher.document(new.read_bytes()))

    def test_a_new_catalog_entry_installs(self):
        new = self.candidate(lambda doc: None)
        target = self.root / 'fresh.vrm'
        publish(new, target)
        self.assertEqual(without_arms(target.read_bytes()), publisher.document(new.read_bytes()))

    def test_a_figure_whose_spread_cannot_be_measured_leaves_the_catalog_and_its_backup_untouched(self):
        before = self.catalog.read_bytes()
        new = self.candidate(lambda doc: doc['extensionsUsed'].append('VRMC_vrm'))
        def refuse(name, data, clips=None):
            raise RuntimeError('no drawn left thigh skin to measure')
        measure, publisher.measure = publisher.measure, refuse
        try:
            with self.assertRaises(RuntimeError):
                publish(new, self.catalog)
        finally:
            publisher.measure = measure
        self.assertEqual(self.catalog.read_bytes(), before)
        self.assertFalse((self.root / '.previous').exists())

    def test_a_file_no_landed_converter_wrote_is_refused_unless_named(self):
        self.catalog.write_bytes(self.candidate(lambda doc: doc['asset'].pop('extras')).read_bytes())
        for change, part in [
            (lambda doc: doc['asset'].pop('extras'), 'names no converter commit'),
            (lambda doc: doc['asset'].update(extras={'converter': 'main'}), 'names no converter commit'),
            (stamped('main'), 'names no converter commit'),
            (stamped(LANDED, dirty=True), 'differs from'),
            (lambda doc: doc['asset']['extras']['converter'].pop('dirty'), 'differs from'),
            (stamped(UNLANDED), 'does not contain'),
            (stamped('0' * 40), 'does not contain'),
        ]:
            before = self.catalog.read_bytes()
            with self.assertRaises(SystemExit) as refused:
                publish(self.candidate(change), self.catalog)
            self.assertIn(part, str(refused.exception))
            self.assertEqual(self.catalog.read_bytes(), before)
            publish(self.candidate(change), self.catalog, unlanded_ok=True)
            self.catalog.write_bytes(before)
        with self.assertRaises(SystemExit) as refused:
            publisher.publish(self.candidate(lambda doc: None), self.catalog, unlanded_ok=True)
        self.assertIn('--converter', str(refused.exception))

    def test_a_file_from_an_older_converter_commit_is_refused_over_a_newer_one_unless_a_rollback(self):
        before = self.catalog.read_bytes()
        with self.assertRaises(SystemExit) as refused:
            publish(self.candidate(stamped(OLDER)), self.catalog)
        self.assertIn(f'{OLDER} is older than {LANDED}', str(refused.exception))
        self.assertNotIn('cannot tell', str(refused.exception))
        self.assertEqual(self.catalog.read_bytes(), before)
        publish(self.candidate(stamped(OLDER)), self.catalog, rollback_ok=True)
        publish(self.candidate(stamped(LANDED)), self.catalog)

    def test_a_commit_landed_since_the_converter_checkout_last_fetched_publishes(self):
        elsewhere = REPO.with_name('elsewhere')
        subprocess.run(['git', 'clone', '--quiet', str(REPO.with_name('origin')), str(elsewhere)], check=True)
        subprocess.run(['git', '-C', str(elsewhere), 'commit', '--quiet', '--allow-empty', '-m', 'c'], check=True)
        subprocess.run(['git', '-C', str(elsewhere), 'push', '--quiet', 'origin', 'HEAD:main'], check=True)
        commit = subprocess.run(['git', '-C', str(elsewhere), 'rev-parse', 'HEAD'], check=True, stdout=subprocess.PIPE, text=True).stdout.strip()
        publish(self.candidate(stamped(commit)), self.catalog)

    def test_a_catalog_file_from_a_commit_the_converter_does_not_know_is_replaced_only_as_a_rollback(self):
        self.catalog.write_bytes(self.candidate(stamped('1' * 40)).read_bytes())
        with self.assertRaises(SystemExit) as refused:
            publish(self.candidate(lambda doc: None), self.catalog)
        self.assertIn('cannot tell', str(refused.exception))
        publish(self.candidate(lambda doc: None), self.catalog, rollback_ok=True)

    def test_an_in_place_republish_measured_against_a_clip_directory_writes_where_named_and_leaves_the_catalog(self):
        against = []
        before = self.catalog.read_bytes()
        staged = self.root / '.staged' / self.catalog.name
        staged.parent.mkdir()
        measure, publisher.measure = publisher.measure, lambda name, data, clips=None: against.append(clips) or SPREAD
        try:
            publisher.publish(self.catalog, self.catalog, clips='next-clips', out_path=staged)
        finally:
            publisher.measure = measure
        self.assertEqual(against, ['next-clips'])
        self.assertEqual(self.catalog.read_bytes(), before)
        self.assertEqual(staged.read_bytes(), publisher.with_arms(before, SPREAD))
        self.assertFalse((self.root / '.previous').exists())

    def test_the_installed_file_carries_the_spread_measured_on_it_with_its_binary_untouched(self):
        body = b'BODY'
        data = glb(AVATAR)
        new = self.root / 'new.vrm'
        new.write_bytes(data[:8] + struct.pack('<I', len(data) + 8 + len(body)) + data[12:] + struct.pack('<I4s', len(body), b'BIN\0') + body)
        measured.clear()
        publish(new, self.catalog)
        installed = self.catalog.read_bytes()
        self.assertEqual(len(measured), 1)
        self.assertEqual(installed, publisher.with_arms(measured[0], SPREAD))
        self.assertEqual(without_arms(installed), AVATAR)
        self.assertEqual(installed[-len(body) - 8:], struct.pack('<I4s', len(body), b'BIN\0') + body)
        self.assertEqual(struct.unpack_from('<I', installed, 8)[0], len(installed))

    def test_the_spread_is_measured_on_the_number_spellings_it_installs(self):
        long, short = '2.6941299438476563e-05', '2.6941299438476562e-05'
        self.assertEqual(float(long), float(short))
        doc = copy.deepcopy(AVATAR)
        doc['accessors'] = [{'max': [0.125]}]
        new = self.root / 'new.vrm'
        new.write_bytes(glb(doc, lambda text: text.replace('0.125', long)))

        def body(data):
            doc = json.loads(data[20:20 + struct.unpack_from('<I', data, 12)[0]], parse_float=str)
            doc['extensions'].pop(publisher.ARMS, None)
            return json.dumps([doc[k] for k in ('nodes', 'meshes', 'accessors')])
        measure, publisher.measure = publisher.measure, lambda name, data, clips=None: {**SPREAD, 'body': body(data)}
        try:
            publish(new, self.catalog)
        finally:
            publisher.measure = measure
        installed = self.catalog.read_bytes()
        self.assertEqual(publisher.document(installed)['extensions'][publisher.ARMS]['body'], body(installed))


if __name__ == '__main__':
    unittest.main()
