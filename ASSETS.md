# Assets

This branch exists only to host GitHub user attachments. **Never merge it into
`main`.**

GitHub publishes a `user-attachments` URL only once the editor edit that minted
it is committed somewhere; an upload made in a discarded editor stays private
and returns 404 to anonymous requests. Committing here is what makes a URL
public, and keeps the binaries out of `main`.

## How to add an asset

1. Open `ASSETS.md` on this branch at `/edit/assets/ASSETS.md`.
2. Paste the file into the editor. GitHub uploads it and inserts an `<img>` or a
   bare URL, keeping the source filename as alt text.
3. Commit to `assets` directly — never open a pull request from this branch.
4. Record the mapping in the index below, then verify the URL returns 200
   unauthenticated before referencing it anywhere that ships.

Note the uploader is only wired up for Markdown extensions: `.md` gets the
`Attach files by dragging & dropping, selecting or pasting them` control, `.mdx`
does not. That is why assets are minted here rather than in the destination page.

## Index

<!-- minted attachments follow --><img width="1200" height="675" alt="windows-wpf-agent-qa" src="https://github.com/user-attachments/assets/54536146-328a-431c-95b4-14a799248026" />
<img width="1200" height="675" alt="windows-msbuild-piano" src="https://github.com/user-attachments/assets/6f878582-8f5d-4400-be44-fc503fcbc1ab" />
<img width="1200" height="675" alt="windows-legacy-postal-app" src="https://github.com/user-attachments/assets/0fb62243-56ec-4d51-8771-35640a5e6031" />
<img width="1920" height="1080" alt="windows-hermes-four-agent-windows" src="https://github.com/user-attachments/assets/15091959-0f93-4076-9338-18ef3a737091" />
<img width="1200" height="675" alt="macos-trajectory-capture" src="https://github.com/user-attachments/assets/d896dd2f-3f0a-4978-93a3-c9a5c562c3e8" />
<img width="1280" height="720" alt="macos-background-dev-loop" src="https://github.com/user-attachments/assets/34217d66-c560-4532-a77b-67ea2f8885af" />
<img width="1200" height="675" alt="macos-background-chrome" src="https://github.com/user-attachments/assets/bcbe275a-9b37-4432-9f76-124d88574f6a" />
<img width="960" height="540" alt="linux-wayland-16-cursors" src="https://github.com/user-attachments/assets/cd354a0b-b0ca-4236-a640-eeb79c338d0c" />
<img width="1200" height="675" alt="linux-multi-pointers" src="https://github.com/user-attachments/assets/b8257284-aef6-46a4-8250-61e6bf5fd57c" />
<img width="1200" height="750" alt="linux-headless-spreadsheet" src="https://github.com/user-attachments/assets/74d428cb-1adc-4ebc-bc43-590f0966e84b" />
<img width="1920" height="1080" alt="open-source-overview" src="https://github.com/user-attachments/assets/1b60d1c6-06fb-47e3-97c0-726421cf45fb" />

