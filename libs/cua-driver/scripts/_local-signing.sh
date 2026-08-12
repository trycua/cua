#!/usr/bin/env bash
#
# macOS local-development signing helpers for _install-local-rust.sh.
# This file has no top-level side effects so its policy can be exercised by
# focused shell tests without building or installing cua-driver.

CUA_LOCAL_SIGN_CN="CuaDriver Local Signing (cua-driver-rs)"

local_signing_keychain() {
    if [ -n "${CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN:-}" ]; then
        printf '%s' "$CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN"
    elif [ -f "$HOME/Library/Keychains/cua-driver-signing.keychain-db" ]; then
        printf '%s' "$HOME/Library/Keychains/cua-driver-signing.keychain-db"
    elif [ -f "$HOME/Library/Keychains/login.keychain-db" ]; then
        printf '%s' "$HOME/Library/Keychains/login.keychain-db"
    else
        printf '%s' "$HOME/Library/Keychains/login.keychain"
    fi
}

print_local_signing_bootstrap() {
    echo "Create and unlock a dedicated development signing keychain, then rerun:" >&2
    echo "  SIGNING_KEYCHAIN=\"\$HOME/Library/Keychains/cua-driver-signing.keychain-db\"" >&2
    echo "  security create-keychain \"\$SIGNING_KEYCHAIN\"  # first time only; prompts for a password" >&2
    echo "  security set-keychain-settings \"\$SIGNING_KEYCHAIN\"" >&2
    echo "  security unlock-keychain \"\$SIGNING_KEYCHAIN\"" >&2
    echo "  export CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN=\"\$SIGNING_KEYCHAIN\"" >&2
    echo "  bash libs/cua-driver/scripts/install-local.sh --require-stable-signing" >&2
    echo "If the certificate is newly created, authorize its private key for codesign as described in:" >&2
    echo "  libs/cua-driver/scripts/README.md#stable-macos-local-signing" >&2
}

# Echoes the `codesign --sign` argument: a matching identity's SHA-1 when
# available, or "-" when it cannot be created or found.
ensure_local_signing_identity() {
    { [ "$OS" = "Darwin" ] && command -v codesign >/dev/null 2>&1; } \
        || { printf -- '-'; return; }
    local kc
    kc="$(local_signing_keychain)"
    [ -f "$kc" ] || { printf -- '-'; return; }
    local identity
    identity="$(security find-identity -p codesigning "$kc" 2>/dev/null \
        | awk -v cn="$CUA_LOCAL_SIGN_CN" 'index($0, "\"" cn "\"") { print $2; exit }')"
    if [ -n "$identity" ]; then
        printf '%s' "$identity"
        return
    fi
    command -v openssl >/dev/null 2>&1 || { printf -- '-'; return; }
    local tmp
    tmp="$(mktemp -d)" || { printf -- '-'; return; }
    printf '[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=%s\n[ext]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=critical,codeSigning\n' \
        "$CUA_LOCAL_SIGN_CN" > "$tmp/req.cnf"
    local pw="cua-local-$$"
    if openssl req -x509 -newkey rsa:2048 -keyout "$tmp/key.pem" -out "$tmp/cert.pem" \
            -days 3650 -nodes -config "$tmp/req.cnf" >/dev/null 2>&1 \
       && { openssl pkcs12 -export -legacy -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
                -out "$tmp/id.p12" -passout pass:"$pw" -name "$CUA_LOCAL_SIGN_CN" >/dev/null 2>&1 \
            || openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
                -out "$tmp/id.p12" -passout pass:"$pw" -name "$CUA_LOCAL_SIGN_CN" >/dev/null 2>&1; } \
       && security import "$tmp/id.p12" -k "$kc" -P "$pw" -A -T /usr/bin/codesign >/dev/null 2>&1; then
        identity="$(security find-identity -p codesigning "$kc" 2>/dev/null \
            | awk -v cn="$CUA_LOCAL_SIGN_CN" 'index($0, "\"" cn "\"") { print $2; exit }')"
        rm -rf "$tmp"
        if [ -n "$identity" ]; then
            printf '%s' "$identity"
            return
        fi
        printf -- '-'
        return
    fi
    rm -rf "$tmp"
    printf -- '-'
}

# Keychain-backed codesign can wait forever for a GUI authorization prompt.
codesign_bounded() {
    local timeout_seconds="$1"
    shift
    local kc=""
    if [ -n "${CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN:-}" ]; then
        kc="$CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN"
    elif [ -f "$HOME/Library/Keychains/cua-driver-signing.keychain-db" ]; then
        kc="$HOME/Library/Keychains/cua-driver-signing.keychain-db"
    fi
    if [ -n "$kc" ]; then
        set -- --keychain "$kc" "$@"
    fi
    if command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$timeout_seconds" codesign "$@"
    elif command -v perl >/dev/null 2>&1; then
        perl -e 'alarm shift; exec @ARGV' "$timeout_seconds" codesign "$@"
    else
        codesign "$@"
    fi
}

clean_partial_bundle_signature() {
    local app="$1"
    rm -rf "$app/Contents/_CodeSignature"
    find "$app" -type f -name '*.cstemp' -delete
}

designated_requirement() {
    codesign -d -r- "$1" 2>&1 \
        | sed -n -e 's/^designated => //p' -e 's/^# designated => //p'
}

classify_designated_requirement() {
    case "$1" in
        *"certificate leaf"*) printf '%s' "certificate-backed" ;;
        *cdhash*) printf '%s' "ad-hoc" ;;
        *) printf '%s' "unknown" ;;
    esac
}

# Signs a staged local app without touching the live installation. Strict mode
# refuses the ad-hoc path; non-strict mode keeps it available for casual local
# development but makes the resulting TCC reset impossible to miss.
sign_staged_local_app() {
    local app_stage="$1"
    local app_dest="$2"
    local sign_id requirement signing_class
    sign_id="$(ensure_local_signing_identity)"

    if [ "$sign_id" != "-" ] \
       && codesign_bounded 20 --force --deep --sign "$sign_id" "$app_stage" 2>/dev/null; then
        requirement="$(designated_requirement "$app_stage")"
        signing_class="$(classify_designated_requirement "$requirement")"
        if [ "$signing_class" = "certificate-backed" ]; then
            echo "${GREEN}signed staged app with a stable local identity — TCC grants survive future install-local rebuilds${NORMAL}"
            return 0
        fi
        echo "${YELLOW}warning: the requested stable identity produced a non-certificate designated requirement${NORMAL}" >&2
    fi

    if [ "${CUA_DRIVER_REQUIRE_STABLE_SIGNING:-0}" = "1" ]; then
        clean_partial_bundle_signature "$app_stage"
        echo "${RED}Error: stable macOS signing is required, but no usable certificate-backed identity was available.${NORMAL}" >&2
        echo "The live installation was not changed." >&2
        print_local_signing_bootstrap
        return 1
    fi

    if [ -d "$app_dest" ]; then
        requirement="$(designated_requirement "$app_dest")"
        if [ "$(classify_designated_requirement "$requirement")" = "certificate-backed" ]; then
            clean_partial_bundle_signature "$app_stage"
            echo "${RED}Error: stable signing failed; preserving the existing certificate-signed $app_dest and its TCC grants.${NORMAL}" >&2
            print_local_signing_bootstrap
            return 1
        fi
    fi

    clean_partial_bundle_signature "$app_stage"
    if ! codesign_bounded 20 --force --deep --sign - "$app_stage" 2>/dev/null; then
        clean_partial_bundle_signature "$app_stage"
        echo "${RED}Error: codesign of staged CuaDriverLocal.app failed; live installation was not changed.${NORMAL}" >&2
        return 1
    fi
    requirement="$(designated_requirement "$app_stage")"
    if [ "$(classify_designated_requirement "$requirement")" != "ad-hoc" ]; then
        echo "${RED}Error: could not verify the staged app's ad-hoc designated requirement; live installation was not changed.${NORMAL}" >&2
        return 1
    fi
    echo "${YELLOW}WARNING: CuaDriverLocal.app was signed ad-hoc (designated requirement uses cdhash).${NORMAL}" >&2
    echo "${YELLOW}Accessibility and Screen Recording grants WILL become invalid on the next rebuild.${NORMAL}" >&2
    print_local_signing_bootstrap
}
