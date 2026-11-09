#!/bin/bash
# ONE TIME, on your Mac: create the public GitHub repo, set up update signing,
# and publish the first self-updating release.
#   bash scripts/publish-github.sh            (repo name: omnimix)
#   bash scripts/publish-github.sh my-name    (custom repo name)
set -euo pipefail
cd "$(dirname "$0")/.."
REPO_NAME="${1:-omnimix}"
KEY="$HOME/.tauri/omnimix.key"
PWFILE="$HOME/.tauri/omnimix.key.password"
need() { command -v "$1" >/dev/null 2>&1; }

echo "== 1/6 GitHub CLI =="
if ! need gh; then
  if ! need brew; then
    echo "Installing Homebrew (asks for your Mac password)..."
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    eval "$(/opt/homebrew/bin/brew shellenv 2>/dev/null || /usr/local/bin/brew shellenv)"
  fi
  brew install gh
fi
gh auth status >/dev/null 2>&1 || gh auth login --hostname github.com --git-protocol https --web --scopes "repo,workflow"
gh auth setup-git
OWNER=$(gh api user --jq .login)
FULL="$OWNER/$REPO_NAME"
echo "Publishing to https://github.com/$FULL"

echo "== 2/6 Node + Tauri CLI =="
[ -s "$HOME/.nvm/nvm.sh" ] && . "$HOME/.nvm/nvm.sh"
need npm || { echo "Node.js is required: brew install node"; exit 1; }
(cd app && npm install --no-audit --no-fund >/dev/null)

echo "== 3/6 Update signing key =="
mkdir -p "$HOME/.tauri"
if [ ! -f "$KEY" ]; then
  PW=$(openssl rand -hex 24)
  printf "%s" "$PW" > "$PWFILE"
  chmod 600 "$PWFILE"
  (cd app && npx tauri signer generate --ci -p "$PW" -w "$KEY")
  chmod 600 "$KEY"
fi
PW=$(cat "$PWFILE")
python3 - "$(cat "$KEY.pub")" "$FULL" <<'PY'
import json, sys
p = 'app/src-tauri/tauri.conf.json'
c = json.load(open(p))
c['plugins']['updater']['pubkey'] = sys.argv[1]
c['plugins']['updater']['endpoints'] = [f"https://github.com/{sys.argv[2]}/releases/latest/download/latest.json"]
json.dump(c, open(p, 'w'), indent=2)
PY

echo "== 4/6 Git repository =="
[ -d .git ] || git init -q -b main
git config user.name >/dev/null || git config user.name "$OWNER"
git config user.email >/dev/null || git config user.email "$OWNER@users.noreply.github.com"
git add -A
git commit -q -m "OmniMix with automatic updates" || true
if gh repo view "$FULL" >/dev/null 2>&1; then
  git remote get-url origin >/dev/null 2>&1 || git remote add origin "https://github.com/$FULL.git"
  git push -u origin main
else
  gh repo create "$FULL" --public --description "OmniMix: per-app and per-device EQ, mixer and audio dashboard for Windows and macOS" --source . --remote origin --push
fi

echo "== 5/6 Release secrets =="
gh secret set TAURI_SIGNING_PRIVATE_KEY --repo "$FULL" < "$KEY"
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo "$FULL" --body "$PW"

echo "== 6/6 First release =="
VERSION=$(python3 -c "import json;print(json.load(open('app/src-tauri/tauri.conf.json'))['version'])")
git tag -f "v$VERSION"
git push -f origin "v$VERSION"

echo
echo "Done. GitHub is building Windows + Mac installers (about 15 to 20 minutes):"
echo "  https://github.com/$FULL/actions"
echo "Download page for your users when it finishes:"
echo "  https://github.com/$FULL/releases/latest"
echo
echo "BACK UP these two files now (password manager or encrypted drive)."
echo "Lose them and installed copies can never update again:"
echo "  $KEY"
echo "  $PWFILE"
