#!/bin/bash
# Ship an update to everyone who installed OmniMix:
#   bash scripts/release.sh 0.1.2 "Fixed X, added Y"
set -euo pipefail
cd "$(dirname "$0")/.."
V="${1:?usage: bash scripts/release.sh <version> \"what changed\"}"
NOTES="${2:-Bug fixes and improvements.}"
if git rev-parse "v$V" >/dev/null 2>&1; then echo "v$V already exists. Pick a higher version."; exit 1; fi
python3 - "$V" <<'PY'
import json, re, sys
v = sys.argv[1]
for p in ['app/src-tauri/tauri.conf.json', 'app/package.json']:
    c = json.load(open(p)); c['version'] = v; json.dump(c, open(p, 'w'), indent=2)
c = json.load(open('app/package-lock.json')); c['version'] = v
if '' in c.get('packages', {}): c['packages']['']['version'] = v
json.dump(c, open('app/package-lock.json', 'w'), indent=2)
s = open('Cargo.toml').read()
s = re.sub(r'(\[workspace\.package\][^\[]*?\nversion = ")[^"]+(")', lambda m: m.group(1) + v + m.group(2), s, flags=re.S)
open('Cargo.toml', 'w').write(s)
PY
printf "%s\n" "$NOTES" > RELEASE_NOTES.md
git add -A
git commit -q -m "Release v$V"
git tag "v$V"
git push -q origin HEAD
git push -q origin "v$V"
REPO=$(git remote get-url origin | sed -E 's#(https://github.com/|git@github.com:)##; s#\.git$##')
echo "v$V is building: https://github.com/$REPO/actions"
echo "Installed apps offer it automatically once the release is published (about 15 to 20 minutes)."
