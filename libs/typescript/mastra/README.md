# Cua Fleet for Mastra

Implementation in progress for [RFC #3640](https://github.com/trycua/cua/issues/3640).

This package will connect a Mastra workspace to an isolated Linux desktop from
an existing Cua Fleet pool using `@trycua/fleet`. Each provider owns one claim;
destroying the provider releases that claim without deleting the shared pool.

Initial scope: Mastra computer tools, refreshable authentication, explicit
lifecycle and failure handling, deterministic tests, and a live Fleet example.
The package is not published. Validation evidence and known gaps will be kept
in the linked draft pull request.
