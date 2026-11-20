#!/usr/bin/env python3
"""Rebuild the attributions for the standard macOS release from the locked dependency graph."""
import hashlib
import json
import re
import subprocess
import urllib.error
import urllib.parse
import urllib.request
from functools import cache
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PLATFORM = 'aarch64-apple-darwin'
PREFIXES = ('license', 'licence', 'notice', 'copying', 'copyright', 'unlicense')


def cargo(*args):
    return subprocess.check_output(['cargo', *args], cwd=ROOT, text=True)


@cache
def upstream_notices(repository, revision):
    url = urllib.parse.urlsplit(repository)
    parts = url.path.strip('/').split('/')
    if url.hostname != 'github.com' or len(parts) < 2 or not re.fullmatch(r'[0-9a-f]{40}', revision):
        raise ValueError(f'No pinned GitHub source for {repository}')
    path = '/'.join(parts[:2]).removesuffix('.git')
    # The VCS revision inside the 0.37.0 crates precedes the public release tag.
    if path.lower() == 'emilk/egui' and revision == '7c0b40402271e8d40cda69f87cf3c2617c2e4382':
        revision = '841a328ba2dd8f6996beb7189ebb7fc00e668a2a'  # upstream tag 0.37.0
    headers = {'User-Agent': 'PixMinima-license-inventory'}
    api = f'https://api.github.com/repos/{path}/git/trees/{revision}'
    req = urllib.request.Request(api, headers=headers)
    try:
        entries = json.load(urllib.request.urlopen(req, timeout=30))['tree']
    except urllib.error.HTTPError as error:
        raise ValueError(f"Upstream notice lookup failed for {api}") from error
    result = []
    for entry in entries:
        name = entry['path']
        if entry['type'] != 'blob' or not name.lower().startswith(PREFIXES):
            continue
        src = f'https://raw.githubusercontent.com/{path}/{revision}/{urllib.parse.quote(name)}'
        data = urllib.request.urlopen(urllib.request.Request(src, headers=headers), timeout=30).read()
        result.append((name, data))
    return result


def notices(package):
    root = Path(package['manifest_path']).parent
    paths = set()
    for path in root.iterdir():
        if path.name.lower().startswith(PREFIXES):
            if path.is_file():
                paths.add(path)
            elif path.is_dir():
                paths.update(p for p in path.rglob('*') if p.is_file())
    if package['license_file']:
        paths.add(root / package['license_file'])
    bundle = [(p.relative_to(root).as_posix(), p.read_bytes()) for p in sorted(paths)]
    if not bundle:
        revision = json.loads((root / '.cargo_vcs_info.json').read_text())['git']['sha1']
        bundle.extend(upstream_notices(package['repository'], revision))
    if package['name'] == 'epaint_default_fonts':
        bundle.extend((p.name, p.read_bytes()) for p in sorted((root / 'fonts').glob('*.txt')))
    if not bundle:
        if package['name'] == 'dispatch' and package['license'] == 'MIT':
            # This release declares MIT in Cargo.toml and includes no notice file.
            # Include the declared authors and standard MIT terms.
            url = 'https://spdx.org/licenses/MIT.txt'
            terms = urllib.request.urlopen(url, timeout=30).read().decode()
            note = 'Authors declared in the crate manifest: ' + ', '.join(package['authors'])
            bundle.append(('MIT', (note + '\n\n' + terms).encode()))
        else:
            raise ValueError(f"No license texts for {package['name']}")
    return bundle


def existing_notices():
    path = ROOT / 'docs/THIRD-PARTY.txt'
    if not path.exists():
        return {}, {}
    sections = re.split(r'^---- ([0-9a-f]{16}) ----\n', path.read_text(), flags=re.M)
    bodies = dict(zip(sections[1::2], (text.strip() for text in sections[2::2])))
    for key, body in bodies.items():
        if hashlib.sha256(body.encode()).hexdigest()[:16] != key:
            raise ValueError('Previous notice text has an invalid fingerprint')
    entries = {}
    for row in sections[0].splitlines():
        fields = row.split(' | ')
        if len(fields) == 3:
            entries[(fields[0], fields[1])] = fields[2].split(', ')
    return entries, bodies


def main():
    cached, previous = existing_notices()
    graph = cargo('tree', '--locked', '--target', PLATFORM, '-p', 'pixminima',
                  '-p', 'pixminima-desktop', '--features', 'pixminima/metal',
                  '--edges', 'normal,build', '--prefix', 'none', '--format', '{p}')
    included = {m.groups() for line in graph.splitlines()
                if (m := re.match(r'^(\S+) v([^ ]+)', line))}
    metadata = json.loads(cargo('metadata', '--locked', '--format-version', '1',
                                '--filter-platform', PLATFORM))
    records = []
    texts = {}
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        name, version = package['name'], package['version']
        if name.startswith('pixminima') or (name, version) not in included:
            continue
        license_name = package['license'] or 'license file'
        shared = cached.get((f'{name} {version}',license_name)) if package['source'] else None
        if shared:
            texts.update({key:previous[key] for key in shared})
            records.append(f"{name} {version} | {license_name} | {', '.join(shared)}")
            continue
        ids = []
        for _, data in notices(package):
            body = data.decode('utf-8-sig').strip().replace('\r\n', '\n')
            key = hashlib.sha256(body.encode()).hexdigest()[:16]
            texts[key] = body
            if key not in ids:
                ids.append(key)
        records.append(f"{name} {version} | {package['license'] or 'license file'} | {', '.join(ids)}")
    font = (ROOT / 'crates/desktop/assets/fonts/OFL.txt').read_text().strip().replace('\r\n', '\n')
    key = hashlib.sha256(font.encode()).hexdigest()[:16]
    texts[key] = font
    records.append(f'Barlow font | OFL-1.1 | {key}')
    output = ['Third-party notices for the standard PixMinima macOS distribution',
              'Includes production and build dependencies. Identical notice texts are printed once.',
              '', *records, '']
    for key, body in sorted(texts.items()):
        output.extend([f'---- {key} ----', body, ''])
    (ROOT / 'docs/THIRD-PARTY.txt').write_text('\n'.join(output) + '\n')
    print(f'{len(records)} component entries, {len(texts)} notice texts.')


if __name__ == '__main__':
    main()
