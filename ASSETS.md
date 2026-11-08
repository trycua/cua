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

| Source file | Attachment | Used by |
| --- | --- | --- |
| `open-source-overview.jpg` | [`1b60d1c6…`](https://github.com/user-attachments/assets/1b60d1c6-06fb-47e3-97c0-726421cf45fb) | `concepts/what-is-cua-bench` |
| `open-source-overview.mp4` | [`35bf5156…`](https://github.com/user-attachments/assets/35bf5156-d326-4484-ad1a-87a7991c85f6) | `concepts/what-is-cua-bench` |
| `linux-headless-spreadsheet.jpg` | [`74d428cb…`](https://github.com/user-attachments/assets/74d428cb-1adc-4ebc-bc43-590f0966e84b) | `index` |
| `linux-headless-spreadsheet.mp4` | [`7096c772…`](https://github.com/user-attachments/assets/7096c772-aa24-4286-a453-6aedfbeb3e23) | `index` |
| `windows-wpf-agent-qa.jpg` | [`54536146…`](https://github.com/user-attachments/assets/54536146-328a-431c-95b4-14a799248026) | `index` |
| `windows-wpf-agent-qa.mp4` | [`4159f98d…`](https://github.com/user-attachments/assets/4159f98d-9623-40d0-a85f-37c913b1403b) | `index` |
| `windows-msbuild-piano.jpg` | [`6f878582…`](https://github.com/user-attachments/assets/6f878582-8f5d-4400-be44-fc503fcbc1ab) | `tutorials/drive-your-first-app` |
| `windows-msbuild-piano.mp4` | [`354af433…`](https://github.com/user-attachments/assets/354af433-8769-44ff-beab-b5cb02ff798e) | `tutorials/drive-your-first-app` |
| `linux-multi-pointers.jpg` | [`b8257284…`](https://github.com/user-attachments/assets/b8257284-aef6-46a4-8250-61e6bf5fd57c) | `concepts/the-no-foreground-contract` |
| `linux-multi-pointers.mp4` | [`c4e23e2f…`](https://github.com/user-attachments/assets/c4e23e2f-1112-4b7b-a575-c6185c78eda4) | `concepts/the-no-foreground-contract` |
| `linux-wayland-16-cursors.jpg` | [`cd354a0b…`](https://github.com/user-attachments/assets/cd354a0b-b0ca-4236-a640-eeb79c338d0c) | `concepts/the-no-foreground-contract` |
| `linux-wayland-16-cursors.mp4` | [`17696766…`](https://github.com/user-attachments/assets/17696766-210d-40b7-b95a-e81be383cc6f) | `concepts/the-no-foreground-contract` |
| `macos-background-dev-loop.jpg` | [`34217d66…`](https://github.com/user-attachments/assets/34217d66-c560-4532-a77b-67ea2f8885af) | `concepts/the-no-foreground-contract` |
| `macos-background-dev-loop.mp4` | [`ef3c7ad5…`](https://github.com/user-attachments/assets/ef3c7ad5-2689-4de5-b844-1c1bb3b621ae) | `concepts/the-no-foreground-contract` |
| `macos-background-chrome.jpg` | [`bcbe275a…`](https://github.com/user-attachments/assets/bcbe275a-9b37-4432-9f76-124d88574f6a) | `how-to-guides/driver/drive-a-web-page` |
| `macos-background-chrome.mp4` | [`46f4be37…`](https://github.com/user-attachments/assets/46f4be37-92c2-4360-bc20-bde35401bc71) | `how-to-guides/driver/drive-a-web-page` |
| `macos-trajectory-capture.jpg` | [`d896dd2f…`](https://github.com/user-attachments/assets/d896dd2f-3f0a-4978-93a3-c9a5c562c3e8) | `how-to-guides/driver/record-and-render-a-trajectory` |
| `macos-trajectory-capture.mp4` | [`9eafceca…`](https://github.com/user-attachments/assets/9eafceca-1738-4e34-949a-2fb0dbe1832e) | `how-to-guides/driver/record-and-render-a-trajectory` |
| `windows-legacy-postal-app.jpg` | [`0fb62243…`](https://github.com/user-attachments/assets/0fb62243-56ec-4d51-8771-35640a5e6031) | `how-to-guides/recipes/automate-a-legacy-windows-app-behind-a-vpn` |
| `windows-legacy-postal-app.mp4` | [`11b248e5…`](https://github.com/user-attachments/assets/11b248e5-d505-4349-aaae-ad9f95a1162f) | `how-to-guides/recipes/automate-a-legacy-windows-app-behind-a-vpn` |
| `windows-hermes-four-agent-windows.jpg` | [`15091959…`](https://github.com/user-attachments/assets/15091959-0f93-4076-9338-18ef3a737091) | `reference/cua-driver/process-model` |
| `windows-hermes-four-agent-windows.mp4` | [`dcc5a81f…`](https://github.com/user-attachments/assets/dcc5a81f-6379-4378-a225-adaf5dfa5dcd) | `reference/cua-driver/process-model` |
| `accessibility-prompt.png` | [`b7a5a0c0…`](https://github.com/user-attachments/assets/b7a5a0c0-f297-4249-81f9-ca42c6b6f8ba) | `tutorials/drive-your-first-app, how-to-guides/driver/install` |
| `accessibility-settings.png` | [`d149fe5a…`](https://github.com/user-attachments/assets/d149fe5a-28cb-460c-8868-4eda80d540a4) | `tutorials/drive-your-first-app, how-to-guides/driver/install` |
| `screen-recording-prompt.png` | [`69efc964…`](https://github.com/user-attachments/assets/69efc964-a375-495e-8a27-400c9dd08e17) | `tutorials/drive-your-first-app, how-to-guides/driver/install` |
| `screen-recording-settings.png` | [`469308b5…`](https://github.com/user-attachments/assets/469308b5-84d7-4fd6-a580-e62925120337) | `tutorials/drive-your-first-app, how-to-guides/driver/install` |
| `browser-use-launch.gif` | [`c5f489b6…`](https://github.com/user-attachments/assets/c5f489b6-a9b8-4c0a-9cd8-e52b5f4f4f01) | `blog/extension-free-browser-use.md` |
| `browser-use-architecture.png` | [`859586cd…`](https://github.com/user-attachments/assets/859586cd-6cc6-4205-a7a2-652998aef977) | `blog/extension-free-browser-use.md` |
| `browser-use-architecture-dark.png` | [`5fefe69b…`](https://github.com/user-attachments/assets/5fefe69b-9670-4e04-9063-dffce4106c39) | `blog/extension-free-browser-use.md` |
| `browser-background-control-demo.mp4` | [`4c155fba…`](https://github.com/user-attachments/assets/4c155fba-2e8f-492f-a40e-e142199edd42) | `blog/extension-free-browser-use.md` |
| `browser-isolated-profile-demo.mp4` | [`63ea24a8…`](https://github.com/user-attachments/assets/63ea24a8-7c6c-41b0-8d82-2bb672d6cbc6) | `blog/extension-free-browser-use.md` |
| `browser-multi-tab-demo.mp4` | [`56de26d9…`](https://github.com/user-attachments/assets/56de26d9-3433-46e3-b907-8b1aef6f12bf) | `blog/extension-free-browser-use.md` |
| `lume-metal-capability-unlock.gif` | [`6e3aa770…`](https://github.com/user-attachments/assets/6e3aa770-d274-4c77-b99b-ae74668b5f5e) | `blog/gpu-passthrough-macos-vms.md` |
| `gpu-capability-ladder.png` | [`55b9d614…`](https://github.com/user-attachments/assets/55b9d614-f94b-4840-b97c-ea5d51e595b5) | `blog/gpu-passthrough-macos-vms.md` |
| `metal-kernel-path.png` | [`34f15072…`](https://github.com/user-attachments/assets/34f15072-f18b-4747-9a98-6397601cbffc) | `blog/gpu-passthrough-macos-vms.md` |
| `metal-kernel-path-mobile.png` | [`6c7e1a9e…`](https://github.com/user-attachments/assets/6c7e1a9e-bb01-4eba-887b-ff362bac340b) | `blog/gpu-passthrough-macos-vms.md` |
| `omarchy-apple-silicon-hero.png` | [`883af3b5…`](https://github.com/user-attachments/assets/883af3b5-6adc-40af-a5c8-0e0f84406757) | `blog/omarchy-arm64-on-lume-x-thread.md` |
| `omarchy-arm64-architecture.png` | [`352315ab…`](https://github.com/user-attachments/assets/352315ab-87d8-4b6c-acfa-dfc2be79b025) | `blog/omarchy-arm64-on-lume-x-thread.md` |
| `omarchy-arm64-capability-matrix.png` | [`1be4f977…`](https://github.com/user-attachments/assets/1be4f977-9229-455d-b650-a87994a8098b) | `blog/omarchy-arm64-on-lume-x-thread.md` |
| `omarchy-background-selection-take-4.mp4` | [Video](https://github.com/user-attachments/assets/b4e5517c-d2db-4758-b4cf-07131b0753b2) | `how-to-guides/sandbox/run-omarchy-on-cloud-fleet` |
| `cursor-highlights.mp4` | [Video](https://github.com/user-attachments/assets/e9344780-e780-46e7-bcce-91f61eda9599) | `index` |
| `map-cursor.mp4` | [Video](https://github.com/user-attachments/assets/e133cb0a-9b70-4812-9d97-76fadd67317b) | `how-to-guides/recipes/build-and-inspect-a-map` |
| `pdf-cursor.mp4` | [Video](https://github.com/user-attachments/assets/5ef3bec8-70a5-45f5-8eb3-930a345b9019) | `how-to-guides/recipes/redact-and-annotate-a-pdf` |
| `archive-cursor.mp4` | [Video](https://github.com/user-attachments/assets/89d570f7-e895-4b66-8c20-ded5da2926a4) | `how-to-guides/recipes/prepare-an-exhibition-panel` |
| `gemini-argon-whiteboard-to-keynote.mp4` | [Video](https://github.com/user-attachments/assets/70e9cc74-0020-43cd-b974-3fb930288ec9) | `cua-driver/guides/gemini` |
| `gemini-argon-whiteboard-to-keynote.jpg` | [`619dc845…`](https://github.com/user-attachments/assets/619dc845-2af0-464e-8a03-b7ce945edec4) | `cua-driver/guides/gemini` |
| `gemini-argon-dashboard-to-notes.mp4` | [Video](https://github.com/user-attachments/assets/28b57899-5681-4a64-8fca-f70762e5c95c) | `cua-driver/guides/gemini` |
| `gemini-argon-dashboard-to-notes.jpg` | [`84058332…`](https://github.com/user-attachments/assets/84058332-45e0-49dd-a736-428ef4fd93d8) | `cua-driver/guides/gemini` |
| `gemini-argon-ledger-entries.mp4` | [Video](https://github.com/user-attachments/assets/358b5547-2d38-4868-ba28-91e90b3c41b7) | `cua-driver/guides/gemini` |
| `gemini-argon-ledger-entries.jpg` | [`38cb5c14…`](https://github.com/user-attachments/assets/38cb5c14-121f-42c5-a642-b4e92d8a59f9) | `cua-driver/guides/gemini` |
| `gemini-cua-driver-architecture.png` | [`7222ab31…`](https://github.com/user-attachments/assets/7222ab31-cda1-4a74-b298-d72ebc1e488a) | `cua-driver/guides/gemini` |
| `gemini-cua-driver-loop.png` | [`bdfff431…`](https://github.com/user-attachments/assets/bdfff431-0f7f-440c-867a-2905fbda4a98) | `cua-driver/guides/gemini` |

CUA-3318 first run screenshots: <img width="1872" height="676" alt="first-run-1-welcome" src="https://github.com/user-attachments/assets/1146557d-6e3b-4273-9394-f9b5cd1fefd9" />
 <img width="2796" height="676" alt="first-run-1b-sign-in" src="https://github.com/user-attachments/assets/33c04b19-9fdc-4f2d-8f1c-d8221c6efbdb" />
 <img width="2796" height="676" alt="first-run-2-where" src="https://github.com/user-attachments/assets/703dec3d-b970-4e5e-8b82-99c52ae6e98f" />
 <img width="2796" height="676" alt="first-run-3-agent" src="https://github.com/user-attachments/assets/6ef9de48-c8a4-4064-a734-ed253b050e49" />
 <img width="2796" height="676" alt="first-run-4-6-ready-next" src="https://github.com/user-attachments/assets/27f7089d-297a-4b0a-961b-d2f99948afae" />
<img width="2732" height="1800" alt="spaces-1b-before-tree" src="https://github.com/user-attachments/assets/493cce49-1409-42cc-ba10-66ffd5496da3" />

<img width="2732" height="1800" alt="spaces-1b-after-tree" src="https://github.com/user-attachments/assets/e8696e1d-df1b-4d88-9eef-79d70d64306b" />

<img width="2732" height="3800" alt="spaces-1b-before-many" src="https://github.com/user-attachments/assets/8a3216fd-4abf-4111-96b1-aa1ab70aefbd" />

<img width="2732" height="3800" alt="spaces-1b-after-many" src="https://github.com/user-attachments/assets/696fd177-a26d-4ffa-ae96-eeb2bf19b9a3" />

<img width="2732" height="3800" alt="spaces-1b-after-many-open" src="https://github.com/user-attachments/assets/3f79ceaf-dce9-463c-bbea-c1b0ecc5118b" />


trycua/cua#6546 (CUA-3510) screenshots: the five spaces-1b-*.jpg images above (alt text is the source filename).

## Spaces environment tag (CUA-3814, feat/spaces-env-badge)

<img width="1800" height="580" alt="envtag-tooltips" src="https://github.com/user-attachments/assets/4e6c50a9-9a78-4850-84b6-9c9b06f6b048" />
<img width="2280" height="900" alt="envtag-after-local-prod" src="https://github.com/user-attachments/assets/bc7f2bcb-89e4-430e-936a-7fccc9a4073d" />
<img width="2280" height="900" alt="envtag-after-local-staging" src="https://github.com/user-attachments/assets/2d1aa524-c77e-44c5-b12c-807d1cc577f1" />
<img width="2280" height="900" alt="envtag-after-stable" src="https://github.com/user-attachments/assets/3ba9726c-9c99-4e5f-a1cd-eb61694a0d80" />
<img width="2280" height="900" alt="envtag-before" src="https://github.com/user-attachments/assets/e3345e70-b719-4a39-996f-2a41dadeadf1" />


## trycua/cua#6965 (CUA-3816)


















<img width="1440" height="900" alt="cua-3816-before-settings-claude-desktop" src="https://github.com/user-attachments/assets/04c02165-c87e-4799-910f-859edf33ef96" />
<img width="840" height="1720" alt="cua-3816-before-onboarding-agent-narrow" src="https://github.com/user-attachments/assets/fda0931b-d458-4c4c-9881-dfe2638771a8" />
<img width="1440" height="900" alt="cua-3816-before-onboarding-agent-desktop" src="https://github.com/user-attachments/assets/ecf6411f-5ac7-43de-bba7-0657ace350af" />
<img width="840" height="2800" alt="cua-3816-before-home-hidden-narrow" src="https://github.com/user-attachments/assets/ca4ce739-faeb-4d23-b53f-278d424c399e" />
<img width="1440" height="1000" alt="cua-3816-before-home-hidden-desktop" src="https://github.com/user-attachments/assets/a563cfb1-7ded-4489-99ed-f2d8ea53ab14" />
<img width="840" height="1720" alt="cua-3816-before-home-agent-menu-narrow" src="https://github.com/user-attachments/assets/fa9f8e2d-433d-475a-97ff-3008038d47e8" />
<img width="1440" height="900" alt="cua-3816-before-home-agent-menu-desktop" src="https://github.com/user-attachments/assets/f5db09fc-c5e4-4f01-b297-3a217d404507" />
<img width="840" height="1720" alt="cua-3816-after-settings-claude-narrow" src="https://github.com/user-attachments/assets/db131dcf-fc23-497b-9e5d-1cfcaaecdb6e" />
<img width="1440" height="1100" alt="cua-3816-after-settings-claude-desktop" src="https://github.com/user-attachments/assets/0b4c173f-8c3b-4305-ad4c-b930a5aa9525" />
<img width="840" height="1720" alt="cua-3816-after-onboarding-agent-narrow" src="https://github.com/user-attachments/assets/c3200406-d8f2-47be-91df-fc8872d00778" />
<img width="1440" height="900" alt="cua-3816-after-onboarding-agent-desktop" src="https://github.com/user-attachments/assets/08d159e5-385f-44fb-bcf4-a6ae31af18bf" />
<img width="840" height="2800" alt="cua-3816-after-home-hidden-narrow" src="https://github.com/user-attachments/assets/2913d0a1-9509-4821-8500-3448fc0e3d23" />
<img width="1440" height="1000" alt="cua-3816-after-home-hidden-desktop" src="https://github.com/user-attachments/assets/5af4f840-9ce1-44b2-91ef-826a826e897c" />
<img width="840" height="1720" alt="cua-3816-after-home-agent-menu-narrow" src="https://github.com/user-attachments/assets/a18a1cd5-00b7-4fc2-add7-13b71462472c" />
<img width="1440" height="900" alt="cua-3816-after-home-agent-menu-desktop" src="https://github.com/user-attachments/assets/dd56ca36-18d7-4ed5-a49f-3b1fb42a603c" />
<img width="1440" height="900" alt="cua-3816-after-computers-list-hidden" src="https://github.com/user-attachments/assets/8f354564-e94a-4888-a83c-0212bf371f72" />
<img width="1440" height="900" alt="cua-3816-after-computer-page-hidden" src="https://github.com/user-attachments/assets/c97674fb-98a5-4c5a-91b6-55a0e69608da" />
<img width="1440" height="900" alt="cua-3816-before-computer-page-hidden" src="https://github.com/user-attachments/assets/bf5c1f08-f9fc-46ae-b3d3-542d42e2db32" />

### trycua/cua PR: Spaces copy audit (CUA-3843), before/after screenshots

<img width="2904" height="940" alt="spaces-copy-audit-cmp-home-desktop" src="https://github.com/user-attachments/assets/56c636e4-9149-40e6-8d1a-a8f8c770f49f" />
<img width="960" height="1052" alt="spaces-copy-audit-cmp-settings-advanced-mobile" src="https://github.com/user-attachments/assets/213757fa-38ab-4945-b1de-758509fab367" />
<img width="2904" height="940" alt="spaces-copy-audit-cmp-settings-advanced-desktop" src="https://github.com/user-attachments/assets/697d3674-2674-4968-b0b8-bd99b24c4872" />
<img width="960" height="1052" alt="spaces-copy-audit-cmp-settings-general-mobile" src="https://github.com/user-attachments/assets/3248f8bd-a5c3-4773-8f66-4d349b4c6350" />
<img width="2904" height="940" alt="spaces-copy-audit-cmp-settings-general-desktop" src="https://github.com/user-attachments/assets/126ce712-47b1-4e23-98e3-2ae4e3f55cd1" />
<img width="960" height="1052" alt="spaces-copy-audit-cmp-computer-mobile" src="https://github.com/user-attachments/assets/ca9b440d-6051-484b-a6e7-5468dd131d8f" />
<img width="2904" height="940" alt="spaces-copy-audit-cmp-computer-desktop" src="https://github.com/user-attachments/assets/bf3657b6-9657-4f8e-bc34-40b703d0ad8e" />
<img width="960" height="1052" alt="spaces-copy-audit-cmp-driver-mobile" src="https://github.com/user-attachments/assets/a0629f8a-de15-40c7-90ae-0e73f53c7a77" />
<img width="2904" height="940" alt="spaces-copy-audit-cmp-driver-desktop" src="https://github.com/user-attachments/assets/6c183fac-c909-48bd-b9a4-82a96e02319b" />
<img width="960" height="1052" alt="spaces-copy-audit-cmp-where-mobile" src="https://github.com/user-attachments/assets/4d074958-c8f2-445c-ae4b-42871a66371d" />
<img width="2904" height="940" alt="spaces-copy-audit-cmp-where-desktop" src="https://github.com/user-attachments/assets/4a5d9e8b-34c7-4176-94a6-424ed958d0ab" />
<img width="960" height="1052" alt="spaces-copy-audit-cmp-home-mobile" src="https://github.com/user-attachments/assets/854a0fba-7719-4db1-80bc-60552eb3ee01" />

<img width="1104" height="1168" alt="cua3854-computer-narrow" src="https://github.com/user-attachments/assets/ab4aba11-bc33-4832-af26-cef48700c4b3" />
<img width="3936" height="908" alt="cua3854-onboarding-desktop" src="https://github.com/user-attachments/assets/dee10edc-ac92-4c8a-aedd-9d009e631bf4" />
<img width="2632" height="908" alt="cua3854-computer-desktop" src="https://github.com/user-attachments/assets/6950300b-4ac8-46ca-a44f-79ef6b0daaa7" />
<img width="2184" height="1168" alt="cua3854-onboarding-narrow" src="https://github.com/user-attachments/assets/cad4899c-cce7-4c29-97e8-785d265edaaa" />
