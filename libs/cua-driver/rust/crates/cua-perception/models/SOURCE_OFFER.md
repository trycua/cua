# Corresponding Source for the OmniParser Icon Detector

This Cua Perception bundle contains a converted OmniParser icon detector that
is licensed under AGPL-3.0-only. Cua distributes it under that license. Cua
does not relicense the detector and does not describe it as MIT. Cua Driver
remains a separately licensed MIT component.

Every bundle includes `source-ledger.json` and the source files named there.
`source/cua-perception-source.tar.gz` contains the exact Cua Perception worker
source, Cargo workspace metadata, artifact assembler, conversion command, model
ledger, and repository license from the commit named in the ledger.

The converted detector comes from upstream source file `icon_detect/model.pt`
at revision `6600256cb0f1b07651e3bc86166196307bad7e2d`, with SHA-256
`dab3d4351ad00b035db829909a4db98354d5a90f6990e4ac00222a9a95d4bf57`. The exact
hashed `model.pt` input is bundled under `source/upstream/`. Source snapshots
for the principal exporter components, including Ultralytics, are bundled under
`source/exporter/`. `models/conversion-recipe.json` records the command,
parameters, versions, absence of local patches, expected output, and the
remaining reproducibility limitation.

If you redistribute this bundle, or let users interact with the converted
detector over a network, pass on this corresponding source and the AGPL-3.0
license text with it. The upstream model repository does not publish training
data or training code, so they are not part of this offer. The same bytes also
remain available from the Cua repository at the revision recorded in the
catalog's `corresponding_source_revision`.

Immutable upstream URLs are recorded as provenance and recovery coordinates.
Bundle verification uses the included bytes and pinned hashes rather than
trusting those declarations. This notice is not legal advice.
