# Changelog

## [0.7.0](https://github.com/trycua/cua/compare/sandbox-v0.6.0...sandbox-v0.7.0) (2026-09-11)


### Features

* **sandbox:** use shared Driver MCP client ([#3718](https://github.com/trycua/cua/issues/3718)) ([d1ac400](https://github.com/trycua/cua/commit/d1ac40014457556c9d66a4d791fa40a8db5b9d68))

## [0.6.0](https://github.com/trycua/cua/compare/sandbox-v0.5.0...sandbox-v0.6.0) (2026-09-10)


### ⚠ BREAKING CHANGES

* **cua-driver:** ClickInput now requires target, position, and delivery_mode; click returns ActionResult directly and raises typed tool errors for refusals.

### Features

* **cua-driver:** expose typed native-window SDK flow ([#3683](https://github.com/trycua/cua/issues/3683)) ([75b04aa](https://github.com/trycua/cua/commit/75b04aac03ed6cb1e08c41b2384595e5c5ab2d9f))
* **sandbox:** connect typed Driver through explicit MCP carrier ([#3693](https://github.com/trycua/cua/issues/3693)) ([dc3d35c](https://github.com/trycua/cua/commit/dc3d35cb40cacceb6d9cc08c61940204ca076a3f))


### Bug Fixes

* **sandbox:** align optional Driver dependency with 0.26.0 ([#3699](https://github.com/trycua/cua/issues/3699)) ([2b748f6](https://github.com/trycua/cua/commit/2b748f64335cb2bf204e9f96d90346460a4419ab))
* **sandbox:** drain MCP responses before cancellation teardown ([#3696](https://github.com/trycua/cua/issues/3696)) ([07e36a3](https://github.com/trycua/cua/commit/07e36a3f05d88ca8be3a04454b8738f81c9d92f6))

## [0.5.0](https://github.com/trycua/cua/compare/sandbox-v0.4.3...sandbox-v0.5.0) (2026-09-09)


### Features

* **cua-driver:** integrate typed Driver access with Fleet Sandbox ([#3654](https://github.com/trycua/cua/issues/3654)) ([c9c29dc](https://github.com/trycua/cua/commit/c9c29dcffea354e3ae0cf75927845e79c9d38028))
* **cua-sandbox:** add signed service URLs ([#3508](https://github.com/trycua/cua/issues/3508)) ([8ab33c7](https://github.com/trycua/cua/commit/8ab33c739f74cd6e7b0a321b7e75ac7b795b4216))
* **sandbox:** add an optional compatible Driver SDK extra ([#3678](https://github.com/trycua/cua/issues/3678)) ([80f2dee](https://github.com/trycua/cua/commit/80f2dee09bca5e44ae6ac6ff3dc0ae81cf91d820))
* **sandbox:** define the Image API contract ([#3327](https://github.com/trycua/cua/issues/3327)) ([a525001](https://github.com/trycua/cua/commit/a52500167b8518dba137abf0b33ea59a504e161d))


### Bug Fixes

* **cua-sandbox:** honor Fleet request timeouts ([#3381](https://github.com/trycua/cua/issues/3381)) ([a925ffa](https://github.com/trycua/cua/commit/a925ffa25e6ea13388e8bead02237785b46e1b4a))
* **sandbox:** manage package releases with Release Please ([#3675](https://github.com/trycua/cua/issues/3675)) ([f880ac4](https://github.com/trycua/cua/commit/f880ac42540d7be6231b89c75115749923fa21be))

## Changelog

Release Please maintains entries for releases after the migration from
`sandbox-v0.4.3`. Earlier releases remain available in the repository's GitHub
release history.
