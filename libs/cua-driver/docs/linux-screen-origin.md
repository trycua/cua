# X11: preserve component bounds already expressed in screen coordinates

## Reproduction

Use an X11 desktop with a title bar and a native AT-SPI application whose
component extents are already screen-relative. The original reproduction used
maximized LibreOffice Impress: the accessible outer frame started at (0,0), the
X11 client at (0,17), and the font field reported Screen (1197,173,59,34) and
Window (1197,156,59,34). Confirm these extents in the current environment;
decoration sizes and field positions vary.

1. Open a disposable presentation with text at 70 pt and maximize its window.
2. Select the text, observe the driver's font-size field and screenshot, then
   use its fresh `element_index` to click the field, replace the size with 60,
   and commit it. Save the presentation.
3. Independently inspect the saved text formatting. Repeat from a fresh copy
   using the visible field's screenshot coordinates as a control.

The original indexed action left the text at 70 pt while the visible-coordinate
control saved 60 pt. Raw Screen minus Window already equals the X11 client
origin: adding a second (0,17) correction shifts input below the intended field.
These are historical fork observations, not fresh GUI certification of this
upstream port. They are recorded in
[the original source/evidence report](https://github.com/tanishqkancharla/cua/blob/46d436b2/libs/cua-driver/docs/linux-screen-origin.md).

## Existing and proposed behavior

The existing renderer heuristic treats a near-zero outer frame as proof that
all Screen coordinates are renderer-local. For the values above it emits
(1197,190,59,34), even though the component already supplied (1197,173,59,34).

The correction checks a component's Window extents only when an X11 Screen
rebase is nonzero. If Screen minus Window agrees with the X11 client origin on
both axes (within two pixels), retain the original Screen bounds. Otherwise
keep the existing renderer correction. This preserves displaced renderer-local
coordinates and their fallback when Window evidence is unavailable. The explicit
GTK Window-coordinate and Wayland paths are unchanged.

## Focused regression and acceptance

```sh
cd libs/cua-driver/rust
cargo test --locked -p platform-linux component_screen_coordinates_do_not_receive_the_client_origin_twice
cargo test --locked -p platform-linux renderer_local_screen_coordinates_retain_their_origin_correction
```

Run on Linux; these tests live in the Linux AT-SPI module. They cover the retained
font-field geometry, negative screen origins, displaced renderer-local extents,
missing Window evidence and one-axis agreement. They verify coordinate projection,
not real application delivery.

Before readiness, run the native reproduction above on the exact candidate and
confirm the saved text is 60 pt, then verify indexed targeting in a displaced
Chromium/Electron window. Run the canonical Linux desktop harness per
`AGENTS.md`. Check the actual observed extents rather than hard-coding the sample
coordinates. Additional Window reads can increase collection latency only in
the existing nonzero-rebase path; the existing bounded collector remains in use.
