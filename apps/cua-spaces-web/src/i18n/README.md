# Languages (i18n)

The web UI is in the language the computer uses, with a Language row in Settings, General (and, later, on the welcome screen) to pick another. English is the source and the fallback. The stack is i18next, react-i18next and ICU messages.

## Add or change words

1. Add the key to `locales/en/<area>.json`. Name it `<area>.<screen_or_component>.<element>` in lower snake case, by role, not by its English (`settings.general.language`). The area is the file.
2. Use it: `const t = useT();` then `t("settings.general.language")`. Outside React, `t` from `@/i18n`.
3. Whole sentences go in one key; never glue pieces together. Variables and plurals are ICU: `"{count, plural, one {# computer} other {# computers}}"`, called as `t("computers.list.count", { count })`.
4. Keep Cua, Cua Spaces, Cua Driver, Cua Volume, Spaces, Space, Keyvault, Teams, Pro, Free, commands, paths and code as they are.

The app core sends `Msg { code, vars }` (`message.rs`), never English: the code is a catalog key, shown with `tMsg(msg)`.

## Which language

Saved choice (`cua-spaces:locale`), then the system's preferred languages from the host (`host-languages.ts`: SwiftUI `cuaHost.languages`, Electron `cuaDesktop.languages`, else `navigator.languages`), then English. Languages marked `draft` in `languages.ts` (machine translations not yet reviewed by a native speaker) are offered as "Preview" in Settings but never picked from the system. Switching applies at once, without a restart. English is bundled; each other language is its own chunk, loaded before the first render that needs it.

## Guards (vitest)

- `no-hardcoded-strings.test.ts`: in the paths listed in `localized.ts`, fails on JSX text, visible attributes and properties (`title`, `aria-label`, `placeholder`, `alt`, `label`, `description`, ...) and toast titles. A PR that extracts a screen adds its paths there.
- `catalog.test.ts`: English is valid ICU and each key is in its area's file; every key the code names exists; every shipped language has every key with the same variables, and none English dropped.

## Pseudo-localization

Open the app with `?lang=pseudo`: every extracted string becomes `[Ļáñĝûáĝé ·····]`, about 40% longer. Plain English on that screen was never extracted; a clipped bracket is a layout that breaks in German.

## Layout

German runs about 30% longer: let labels wrap, never clip. Chinese and Japanese use the system CJK faces and `word-break: auto-phrase` (`src/index.css`). `<html lang>` follows the language.
