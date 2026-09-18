# Corresponding Source Offer

Every sealed Cua Perception bundle includes `source/cua-perception-source.tar.gz`
and `source-ledger.json`. The archive contains the exact Cua Perception worker
source, Cargo workspace metadata, artifact assembler, model ledger, conversion
provenance, and repository license from the commit named in the ledger.

The converted OmniParser detector is tied to upstream source file
`icon_detect/model.pt` at revision
`6600256cb0f1b07651e3bc86166196307bad7e2d` and SHA-256
`dab3d4351ad00b035db829909a4db98354d5a90f6990e4ac00222a9a95d4bf57`.
Its reviewed export environment and parameters are recorded in
`MODEL_LEDGER.md`. A distributor must retain the bundled source archive and
these immutable upstream coordinates with every copy of the detector.

Questions about corresponding source may be directed to the repository named
in `source-ledger.json`. The archive in the bundle is the durable source offer;
no moving branch or mutable download URL is required to fulfill it.
