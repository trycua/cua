import "./DeviceAuthorization.css"

const USER_CODE_PATTERN = /^[A-Z0-9]{4}-[A-Z0-9]{4}$/

function normalizeUserCode(value: string | null): string | null {
  if (!value) return null
  const normalized = value.trim().toUpperCase()
  return USER_CODE_PATTERN.test(normalized) ? normalized : null
}

function keycloakBaseUrl(): string {
  return window.__CYCLOPS_CS_CFG__?.kcUrl ?? "https://auth.cua.ai"
}

function keycloakRealm(): string {
  return window.__CYCLOPS_CS_CFG__?.kcRealm ?? "cyclops-cs"
}

function deviceVerificationUrl(userCode: string): string {
  const base = keycloakBaseUrl().replace(/\/$/, "")
  const realm = encodeURIComponent(keycloakRealm())
  return `${base}/realms/${realm}/device?user_code=${encodeURIComponent(userCode)}`
}

function signupUrl(userCode: string): string {
  const continuation = `/device?user_code=${encodeURIComponent(userCode)}`
  return `https://cua.ai/signup?redirect_url=${encodeURIComponent(continuation)}`
}

export function DeviceAuthorization() {
  const userCode = normalizeUserCode(
    new URLSearchParams(window.location.search).get("user_code"),
  )

  return (
    <main className="device-page">
      <div className="device-orbit device-orbit-one" />
      <div className="device-orbit device-orbit-two" />
      <section className="device-card" aria-labelledby="device-title">
        <div className="device-wordmark">
          cua
        </div>
        <p className="device-eyebrow">Secure device authorization</p>
        <h1 id="device-title">Connect your Cua CLI</h1>
        <p className="device-lede">
          Confirm the code from your terminal, then sign in to authorize this
          device. Your CLI never receives your password.
        </p>

        {userCode ? (
          <>
            <div className="device-code">
              {userCode}
            </div>
            <div className="device-actions">
              <a
                className="device-button device-button-primary"
                href={deviceVerificationUrl(userCode)}
              >
                Sign in and continue
              </a>
              <a
                className="device-button device-button-secondary"
                href={signupUrl(userCode)}
              >
                Create an account
              </a>
            </div>
            <p className="device-footnote">
              Only continue if this code exactly matches the one shown by the
              CLI. Codes expire automatically.
            </p>
          </>
        ) : (
          <div className="device-alert" role="alert">
            <strong>Enter the code shown by the CLI.</strong>
            <span>
              Open the complete verification link printed in your terminal,
              including its eight-character code.
            </span>
          </div>
        )}
      </section>
    </main>
  )
}
