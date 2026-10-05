"""Do not present partial, stale or altered evidence as a verified demo."""
import hashlib
import json
import pathlib
import tempfile
import unittest

import investor_report as report


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.bundle = pathlib.Path(self.tmp.name)
        (self.bundle / 'Cargo.lock').write_bytes(b'lock')
        self.data = dict(schema_version=2, complete=True, passed=True,
                         lockfile_sha256=hashlib.sha256(b'lock').hexdigest(),
                         metadata={key: dict(exit_code=0, stdout='', stderr='') for key in
                                   ('commit', 'working_tree', 'working_tree_end', 'cargo', 'rustc')}, checks=[])
        self.data['metadata']['commit']['stdout'] = 'a' * 40
        for name, command in report.COMMANDS:
            check = dict(name=name, command=command, exit_code=0, passed=True,
                         missing_expected_lines=[], logs={})
            for stream in ('stdout', 'stderr'):
                (self.bundle / f'{name}.{stream}.log').write_bytes(b'log')
                check['logs'][stream] = hashlib.sha256(b'log').hexdigest()
            self.data['checks'].append(check)

    def validate(self):
        (self.bundle / 'manifest.json').write_text(json.dumps(self.data))
        return report.validate(self.bundle)[1]

    def test_complete_consistent_bundle(self):
        self.assertEqual([], self.validate())

    def test_incomplete_and_failed_are_not_verified(self):
        for flag in ('complete', 'passed'):
            with self.subTest(flag=flag):
                self.data[flag] = False
                self.assertTrue(self.validate())
                self.data[flag] = True

    def test_missing_or_duplicate_check(self):
        check = self.data['checks'].pop()
        self.assertTrue(self.validate())
        self.data['checks'].extend([check, check])
        self.assertTrue(self.validate())

    def test_altered_or_missing_log(self):
        path = self.bundle / 'red-team.stdout.log'
        path.write_text('forged')
        self.assertTrue(self.validate())
        path.unlink()
        self.assertTrue(self.validate())

    def test_dirty_start_or_end(self):
        for key in ('working_tree', 'working_tree_end'):
            self.data['metadata'][key]['stdout'] = ' M core/src/lib.rs'
            self.assertTrue(self.validate())
            self.data['metadata'][key]['stdout'] = ''

    def test_nonzero_cannot_be_overridden_by_passed_flag(self):
        self.data['checks'][0]['exit_code'] = 1
        self.assertTrue(self.validate())

    def test_unvalidated_redteam_command_rejected(self):
        self.data['checks'][2]['command'] = ['true']
        self.assertTrue(self.validate())

    def test_lockfile_integrity(self):
        (self.bundle / 'Cargo.lock').write_text('changed')
        self.assertTrue(self.validate())

    def test_untrusted_names_do_not_become_paths(self):
        self.data['checks'][0]['name'] = '../outside'
        self.assertTrue(self.validate())

    def test_html_escapes_metadata(self):
        self.data['captured_at_utc'] = '<script>alert(1)</script>'
        rendered = report.render(self.data, ['<img src=x>'])
        self.assertNotIn('<script>', rendered)
        self.assertIn('&lt;script&gt;', rendered)
        self.assertIn('&lt;img src=x&gt;', rendered)


if __name__ == '__main__':
    unittest.main()
