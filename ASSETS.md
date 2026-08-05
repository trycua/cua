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

<!-- minted attachments follow -->
