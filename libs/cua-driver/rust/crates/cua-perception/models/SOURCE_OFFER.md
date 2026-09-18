# Corresponding Source Offer

Every sealed Cua Perception review bundle includes `source-ledger.json` and the
source files named there. `source/cua-perception-source.tar.gz` contains the
exact Cua Perception worker source, Cargo workspace metadata, artifact
assembler, conversion command, model ledger, and repository license from the
commit named in the ledger.

The converted OmniParser detector is tied to upstream source file
`icon_detect/model.pt` at revision
`6600256cb0f1b07651e3bc86166196307bad7e2d` and SHA-256
`dab3d4351ad00b035db829909a4db98354d5a90f6990e4ac00222a9a95d4bf57`.
The exact hashed `model.pt` input is bundled under `source/upstream/`. Source
snapshots for the principal exporter components are bundled under
`source/exporter/`. `models/conversion-recipe.json` records the command,
parameters, versions, absence of local patches, expected output, and the
remaining reproducibility limitation. A distributor must retain all source
artifacts and the ledgers with every copy of the detector.

These files are a review payload, not a claim that legal review is complete.
The model ledger remains `license-review-required`. Immutable upstream URLs are
recorded as provenance and recovery coordinates; bundle verification uses the
included bytes and pinned hashes rather than trusting those declarations.
