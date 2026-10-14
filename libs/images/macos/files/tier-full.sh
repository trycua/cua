#!/bin/bash
# The full tier (the default macOS image): slim plus dev tooling, every
# install pinned in versions.env. Runs IN the guest as the desktop user, on a
# clone of the slim VM that just passed the doctor.
#
#   tier-full.sh STAGE_DIR
#
# Installs:
#   Xcode Command Line Tools $CLT_VERSION  softwareupdate (skipped when present)
#   Homebrew $HOMEBREW_TAG                 /opt/homebrew, git clone at the pinned commit
#   gh, jq, ripgrep, uv                    /usr/local/bin (release archives)
#   Node $NODE_VERSION + pnpm              /usr/local/lib/node, links in /usr/local/bin
#   Go $GO_VERSION                         /usr/local/go, links in /usr/local/bin
#   rustup + Rust $RUST_VERSION            ~/.rustup, ~/.cargo (minimal profile)
# python3 and git come from the Command Line Tools.
#
# No `brew install`/`brew upgrade`: Homebrew formulae float, so the tools come
# from checksummed release archives and Homebrew is there for the user.
set -euo pipefail
STAGE="${1:?staging directory}"
# shellcheck source=/dev/null # STAGE/versions.env, a copy of ../versions.env
. "$STAGE/versions.env"
C="$STAGE/cache"
# The password on a here-string, not a pipe: with sudo's credential cache
# warm, sudo never reads it, and a printf pipe then dies of SIGPIPE, which
# pipefail turns into a failed step.
sudo_run() { sudo -S -p '' "$@" <<<"${CUA_SUDO_PW:-lume}"; }
say() { echo "==> $*"; }
verified() {  # URL SHA256 -> the cached file, checked
    local f; f="$C/$(basename "$1")"
    [ "$(shasum -a 256 "$f" | cut -d' ' -f1)" = "$2" ] || { echo "checksum mismatch: $f" >&2; exit 1; }
    echo "$f"
}
W="$(mktemp -d /tmp/cua-build-full.XXXXXX)"
trap 'rm -rf "$W"' EXIT
export HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 HOMEBREW_NO_ENV_HINTS=1

say "Command Line Tools $CLT_VERSION"
clt="$(pkgutil --pkg-info=com.apple.pkg.CLTools_Executables 2>/dev/null | sed -n 's/^version: //p')"
if [[ "$clt" != "$CLT_VERSION".* ]]; then
    flag=/tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress
    touch "$flag"
    softwareupdate --list 2>&1 | grep -qF "Label: $CLT_LABEL" ||
        { rm -f "$flag"; echo "softwareupdate does not offer '$CLT_LABEL'" >&2; exit 1; }
    sudo_run softwareupdate --install "$CLT_LABEL" --agree-to-license
    rm -f "$flag"
    clt="$(pkgutil --pkg-info=com.apple.pkg.CLTools_Executables | sed -n 's/^version: //p')"
    [[ "$clt" == "$CLT_VERSION".* ]] || { echo "Command Line Tools are $clt, not $CLT_VERSION" >&2; exit 1; }
fi
sudo_run xcode-select --switch /Library/Developer/CommandLineTools
echo "   CLTools_Executables $clt"

say "Homebrew $HOMEBREW_TAG ($HOMEBREW_COMMIT)"
sudo_run install -d -o "$(id -un)" -g admin -m 0755 /opt/homebrew
if [ ! -d /opt/homebrew/.git ]; then
    for attempt in 1 2 3; do
        git -c advice.detachedHead=false clone -q --depth 1 --branch "$HOMEBREW_TAG" \
            https://github.com/Homebrew/brew /opt/homebrew && break
        [ "$attempt" = 3 ] && exit 1
        find /opt/homebrew -mindepth 1 -delete; echo "clone attempt $attempt failed; retrying" >&2; sleep 20
    done
fi
[ "$(git -C /opt/homebrew rev-parse HEAD)" = "$HOMEBREW_COMMIT" ] ||
    { echo "Homebrew checkout is not $HOMEBREW_COMMIT" >&2; exit 1; }
/opt/homebrew/bin/brew --version | head -1

say "gh $GH_VERSION, jq $JQ_VERSION, ripgrep $RG_VERSION, uv $UV_VERSION"
sudo_run install -d -o root -g wheel -m 0755 /usr/local/bin /usr/local/lib
f="$(verified "$GH_URL" "$GH_SHA256")"; ditto -x -k "$f" "$W/gh"
sudo_run install -m 0755 "$W"/gh/gh_*/bin/gh /usr/local/bin/gh
f="$(verified "$JQ_URL" "$JQ_SHA256")"; sudo_run install -m 0755 "$f" /usr/local/bin/jq
f="$(verified "$RG_URL" "$RG_SHA256")"; tar -xzf "$f" -C "$W"
sudo_run install -m 0755 "$W"/ripgrep-*/rg /usr/local/bin/rg
f="$(verified "$UV_URL" "$UV_SHA256")"; tar -xzf "$f" -C "$W"
sudo_run install -m 0755 "$W"/uv-*/uv "$W"/uv-*/uvx /usr/local/bin/

say "Node $NODE_VERSION, pnpm $PNPM_VERSION"
sudo_run rm -rf /usr/local/lib/node
sudo_run mkdir -p /usr/local/lib/node
f="$(verified "$NODE_URL" "$NODE_SHA256")"; sudo_run tar -xzf "$f" -C /usr/local/lib/node --strip-components 1
sudo_run chown -R root:wheel /usr/local/lib/node
for b in node npm npx corepack; do sudo_run ln -sfn "/usr/local/lib/node/bin/$b" "/usr/local/bin/$b"; done
f="$(verified "$PNPM_URL" "$PNPM_SHA256")"
# A throwaway npm cache: sudo keeps HOME, so the default would leave a
# root-owned ~/.npm. pnpm's install script links its native binary, an
# exact-version optional dependency npm checks against the registry's
# integrity hash.
npm_cache=/tmp/cua-build-npm-cache
sudo_run env PATH="/usr/local/lib/node/bin:$PATH" npm_config_cache="$npm_cache" \
    npm install -g --no-audit --no-fund --no-update-notifier --allow-scripts=pnpm "$f" >/dev/null
sudo_run rm -rf "$npm_cache"
for b in pnpm pnpx; do sudo_run ln -sfn "/usr/local/lib/node/bin/$b" "/usr/local/bin/$b"; done

say "Go $GO_VERSION"
sudo_run rm -rf /usr/local/go
f="$(verified "$GO_URL" "$GO_SHA256")"; sudo_run tar -xzf "$f" -C /usr/local
for b in go gofmt; do sudo_run ln -sfn "/usr/local/go/bin/$b" "/usr/local/bin/$b"; done

say "rustup $RUSTUP_VERSION, Rust $RUST_VERSION"
f="$(verified "$RUSTUP_URL" "$RUSTUP_SHA256")"; install -m 0755 "$f" "$W/rustup-init"
"$W/rustup-init" -y --no-modify-path --profile minimal --default-toolchain "$RUST_VERSION" >/dev/null
"$HOME/.cargo/bin/rustup" component add clippy rustfmt >/dev/null
rm -rf "$HOME/.rustup/downloads" "$HOME/.rustup/tmp"

say "login shell environment"
# zsh is the default shell; bash logins read .bash_profile.
# shellcheck disable=SC2016 # expanded at login, not now
PROFILE_BLOCK='# cua full image: Homebrew (pinned, no auto-update), Rust, Go, Node
eval "$(/opt/homebrew/bin/brew shellenv)"
export HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1
export PATH="$HOME/.cargo/bin:/usr/local/bin:$PATH"'
for f in "$HOME/.zprofile" "$HOME/.bash_profile"; do
    grep -q 'cua full image' "$f" 2>/dev/null || printf '%s\n' "$PROFILE_BLOCK" >>"$f"
done

say "versions"
/usr/bin/git --version; /usr/bin/python3 --version; /usr/local/bin/gh --version | head -1
/usr/local/bin/jq --version; /usr/local/bin/rg --version | head -1; /usr/local/bin/uv --version
/usr/local/bin/node --version; /usr/local/bin/pnpm --version; /usr/local/bin/go version
"$HOME/.cargo/bin/rustc" --version; "$HOME/.cargo/bin/cargo" --version

say "caches"
/opt/homebrew/bin/brew cleanup -s >/dev/null 2>&1 || true
rm -rf "$HOME/Library/Caches/Homebrew" "$HOME/Library/Caches/go-build" "$HOME/.npm" "$HOME/.cache/uv"
echo "full tier installed"
