"""Offline bootstrap tests; transport is replaced with release fixtures."""
import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import types
import unittest
from unittest.mock import patch

source = Path(__file__).with_name('install.sh').read_text().split("<<'PYTHON'\n", 1)[1].rsplit('\nPYTHON', 1)[0]
installer = types.ModuleType('installer')
exec(compile(source, 'install.sh', 'exec'), installer.__dict__)


def release(tag, complete=True):
    name = 'bytetide-x86_64-unknown-linux-musl.tar.gz'
    return dict(tag_name=tag, draft=False, prerelease=False, assets=[
        dict(name=name, browser_download_url='https://example.com/archive'),
        dict(name=name + '.sha256', browser_download_url='https://example.com/checksum'),
    ] if complete else [])


class InstallerTests(unittest.TestCase):
    def test_selects_highest_complete_stable_release(self):
        releases = [release('v0.10.0'), release('cli-v0.9.0'), release('v0.11.0', False), release('v9.0.0-rc.1')]
        self.assertEqual(installer.select_release(releases, 'bytetide-x86_64-unknown-linux-musl.tar.gz', None)[0]['tag_name'], 'v0.10.0')

    def test_prefers_cli_and_supports_pin(self):
        releases = [release('v1.0.0'), release('cli-v1.0.0'), release('v0.9.0')]
        name = 'bytetide-x86_64-unknown-linux-musl.tar.gz'
        self.assertEqual(installer.select_release(releases, name, None)[0]['tag_name'], 'cli-v1.0.0')
        self.assertEqual(installer.select_release(releases, name, (0, 9, 0))[0]['tag_name'], 'v0.9.0')
        releases[1]['draft'] = True
        self.assertEqual(installer.select_release(releases, name, None)[0]['tag_name'], 'v1.0.0')

    def test_pagination(self):
        pages = [b'[' + b','.join([b'{}'] * 100) + b']', b'[]']
        with patch.object(installer, 'download', side_effect=pages) as fetch:
            self.assertEqual(len(installer.releases()), 100)
            self.assertIn('page=2', fetch.call_args.args[0])

    def test_install_and_bad_checksum_preserve_existing_binary(self):
        buffer = io.BytesIO()
        binary = b'#!/bin/sh\necho "bytetide 1.0.0"\n'
        with tarfile.open(fileobj=buffer, mode='w:gz') as archive:
            info = tarfile.TarInfo('bytetide')
            info.size = len(binary)
            archive.addfile(info, io.BytesIO(binary))
        payload = buffer.getvalue()
        name = 'bytetide-x86_64-unknown-linux-musl.tar.gz'
        sidecar = f'{hashlib.sha256(payload).hexdigest()}  {name}\n'.encode()
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / 'bytetide'
            destination.write_bytes(b'old')
            with patch.object(installer, 'download', side_effect=[sidecar, payload]):
                installer.install(release('v1.0.0'), (1, 0, 0), name, directory)
            self.assertEqual(destination.read_bytes(), binary)
            destination.write_bytes(b'old')
            with patch.object(installer, 'download', side_effect=[sidecar, b'corrupt']):
                with self.assertRaises(ValueError):
                    installer.install(release('v1.0.0'), (1, 0, 0), name, directory)
            self.assertEqual(destination.read_bytes(), b'old')
            self.assertEqual(list(Path(directory).iterdir()), [destination])

    def test_corrupt_gzip_trailer_is_rejected(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode='w:gz') as archive:
            info = tarfile.TarInfo('bytetide')
            info.size = 4
            archive.addfile(info, io.BytesIO(b'test'))
        with self.assertRaises((ValueError, OSError, EOFError)):
            installer.extract_binary(buffer.getvalue()[:-8] + b'XXXXXXXX')

    def test_archive_rejects_symlink(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode='w:gz') as archive:
            info = tarfile.TarInfo('bytetide')
            info.type = tarfile.SYMTYPE
            info.linkname = '/etc/passwd'
            archive.addfile(info)
        with self.assertRaises(ValueError):
            installer.extract_binary(buffer.getvalue())


if __name__ == '__main__':
    unittest.main()
