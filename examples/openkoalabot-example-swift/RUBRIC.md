# OpenKoalaBots rubric

What each rendered surface is checked against.

## The app window

`Desktop/KoalaShell.swift` (sidebar, conversation, composer, Agent Computer
pane) and `Desktop/SpaceWizard.swift` are checked by running the app
(`OPENKOALABOTS_DESIGN_CAPTURE`, see the README) in both light and dark mode.

## The export screens

`OpenKoalaBotExample export <dir>` renders the fixture screens (`Model/Fixtures.swift`)
through `DesktopShell` and the shared transcript views, with no window server.
They are regression renders: a change that moves them should be deliberate.

## Ungradeable by design

- The approval card enforces nothing: the Spaces contract has no approval
  primitive (`FRICTION.md` 43), and `ApprovalCard.enforcementIsImplemented` is
  `false`.
- Live screen pixels depend on the attached Space and are never compared.
