#!/bin/sh
# Bootstrap uses only Python 3's standard library; no Rust toolchain is required.
set -eu
if ! command -v python3 >/dev/null 2>&1; then
    echo 'ByteTide installer requires Python 3. Install python3 and retry.' >&2
    exit 1
fi
exec python3 - "$@" <<'PYTHON'
"""Install a verified ByteTide CLI release without modifying shell profiles."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

API = 'https://api.github.com/repos/RtuQ/bytetide/releases'
MAX_ARCHIVE = 128 * 1024 * 1024
MAX_BINARY = 256 * 1024 * 1024


class HTTPSRedirect(urllib.request.HTTPRedirectHandler):
    """Disallow redirects that downgrade the transport."""
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        if not newurl.startswith('https://'):
            raise ValueError('Refusing non-HTTPS redirect')
        return super().redirect_request(request, fp, code, msg, headers, newurl)


def download(url, limit):
    """Download a bounded HTTPS response with a socket timeout."""
    if not url.startswith('https://'):
        raise ValueError('Refusing non-HTTPS URL')
    request = urllib.request.Request(url, headers={'User-Agent': 'bytetide-installer', 'Accept': 'application/vnd.github+json'})
    with urllib.request.build_opener(HTTPSRedirect()).open(request, timeout=30) as response:
        data = response.read(limit + 1)
    if len(data) > limit:
        raise ValueError('Download exceeds size limit')
    return data


def parse_version(text):
    """Accept stable release versions with no leading zeros or suffixes."""
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', text):
        raise ValueError('Expected stable version such as 0.8.0')
    return tuple(map(int, text.split('.')))


def releases():
    """Read every release page so ordering does not decide eligibility."""
    result = []
    for page in range(1, 101):
        batch = json.loads(download(f'{API}?per_page=100&page={page}', 16 * 1024 * 1024))
        if not isinstance(batch, list):
            raise ValueError('Invalid release API response')
        result.extend(batch)
        if len(batch) < 100:
            return result
    raise ValueError('Release list exceeds pagination limit')


def select_release(items, archive, pinned):
    """Select a complete stable release, preferring cli-v on version ties."""
    eligible = []
    for item in items:
        if item.get('draft') or item.get('prerelease'):
            continue
        tag = item.get('tag_name', '')
        match = re.fullmatch(r'(cli-v|v)(.+)', tag)
        if not match:
            continue
        try:
            version = parse_version(match[2])
        except ValueError:
            continue
        names = {asset['name'] for asset in item.get('assets', [])}
        if (pinned is None or version == pinned) and {archive, archive + '.sha256'} <= names:
            eligible.append((version, tag.startswith('cli-v'), item))
    if not eligible:
        raise ValueError('No stable release has this platform archive and SHA-256 sidecar')
    version, _, item = max(eligible, key=lambda row: row[:2])
    return item, version


def extract_binary(payload):
    """Read only the root regular binary; never extract archive paths."""
    limit = MAX_BINARY + 1024 * 1024
    with gzip.GzipFile(fileobj=io.BytesIO(payload)) as compressed:
        unpacked = compressed.read(limit + 1)
    if len(unpacked) > limit:
        raise ValueError('Uncompressed archive exceeds size limit')
    with tarfile.open(fileobj=io.BytesIO(unpacked), mode='r:') as archive:
        matches = [entry for entry in archive if entry.name == 'bytetide']
        if len(matches) != 1 or not matches[0].isfile() or not 0 < matches[0].size <= MAX_BINARY:
            raise ValueError('Archive must contain one regular root bytetide binary')
        with archive.extractfile(matches[0]) as binary:
            data = binary.read(MAX_BINARY + 1)
        if len(data) != matches[0].size:
            raise ValueError('Invalid binary size')
        return data


def install(release, version, archive, directory):
    """Verify and stage beside the destination, then replace atomically."""
    urls = {asset['name']: asset['browser_download_url'] for asset in release['assets']}
    sidecar = download(urls[archive + '.sha256'], 4096).decode('ascii').split()
    payload = download(urls[archive], MAX_ARCHIVE)
    if (len(sidecar) != 2 or sidecar[1].lstrip('*') != archive
            or not re.fullmatch('[0-9a-fA-F]{64}', sidecar[0])
            or hashlib.sha256(payload).hexdigest() != sidecar[0].lower()):
        raise ValueError('SHA-256 verification failed; existing installation unchanged')
    binary = extract_binary(payload)
    directory = Path(directory).expanduser()
    directory.mkdir(parents=True, exist_ok=True)
    destination = (directory / 'bytetide').resolve()
    mode = (stat.S_IMODE(destination.stat().st_mode) & 0o777) | 0o100 if destination.exists() else 0o755
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=destination.parent, prefix='.bytetide-', delete=False) as output:
            temporary = Path(output.name)
            output.write(binary)
            output.flush()
            os.fchmod(output.fileno(), mode)
            os.fsync(output.fileno())
        expected = 'bytetide ' + '.'.join(map(str, version))
        completed = subprocess.run([str(temporary), '--version'], capture_output=True, text=True, timeout=15, check=True)
        if completed.stdout.strip() != expected:
            raise ValueError('Downloaded binary version does not match release')
        os.replace(temporary, destination)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    print(f'Installed {expected} at {destination}', file=sys.stderr)
    if str(directory.resolve()) not in os.environ.get('PATH', '').split(os.pathsep):
        print(f'Add {directory.resolve()} to PATH in your shell configuration.', file=sys.stderr)


def main():
    """Parse explicit bootstrap options before contacting GitHub."""
    parser = argparse.ArgumentParser(description='Install a verified ByteTide CLI release')
    parser.add_argument('--install-dir', default=str(Path.home() / '.local/bin'))
    parser.add_argument('--version', help='stable version, e.g. 0.8.0 (optional v prefix)')
    args = parser.parse_args()
    targets = {
        ('Linux', 'x86_64'): 'x86_64-unknown-linux-musl',
        ('Linux', 'aarch64'): 'aarch64-unknown-linux-musl',
        ('Linux', 'arm64'): 'aarch64-unknown-linux-musl',
        ('Darwin', 'arm64'): 'aarch64-apple-darwin',
    }
    target = targets.get((platform.system(), platform.machine()))
    if target is None:
        raise ValueError('Unsupported platform; download a compatible binary manually')
    pinned = parse_version(args.version.removeprefix('v')) if args.version else None
    archive = f'bytetide-{target}.tar.gz'
    release, version = select_release(releases(), archive, pinned)
    install(release, version, archive, args.install_dir)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(f'Installation failed: {error}', file=sys.stderr)
        sys.exit(1)
PYTHON
