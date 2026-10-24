# Changelog

## [0.4.0](https://github.com/trycua/cua/compare/bench-v0.3.1...bench-v0.4.0) (2026-10-24)


### Features

* compat gateways for E2B and Daytona, Hetzner as your own cloud, the elastic pool, cua share and the public relay ([#5778](https://github.com/trycua/cua/issues/5778)) ([d4f3f80](https://github.com/trycua/cua/commit/d4f3f8063d7b30a3b3bda2b4c05c565a9dfcb460))


### Bug Fixes

* **sandbox:** the [driver] extra installs the current cua-driver (0.37.0) (CUA-1902) ([#5533](https://github.com/trycua/cua/issues/5533)) ([34f3279](https://github.com/trycua/cua/commit/34f3279247e0e910e4896ee33c499beafdb7ab9a))
* **sdk:** delete the Fleet SDK, pools and claims from the Rust SDK, cua-sandbox, Cua Bench and the apps (CUA-1836) ([#5488](https://github.com/trycua/cua/issues/5488)) ([348c47c](https://github.com/trycua/cua/commit/348c47cc30b1d222844480838ccc4acf53f7a511))
* **security:** distrust the compromised eMudhra intermediates in every Cua client (CUA-1948) ([#5622](https://github.com/trycua/cua/issues/5622)) ([149ea51](https://github.com/trycua/cua/commit/149ea51c613cc840c3760dec822c57b7e65df05a))

## [0.3.1](https://github.com/trycua/cua/compare/bench-v0.3.0...bench-v0.3.1) (2026-10-19)


### Bug Fixes

* **cua-bench:** fail fast on unsupported Windows Arena hosts ([#5356](https://github.com/trycua/cua/issues/5356)) ([2236b53](https://github.com/trycua/cua/commit/2236b5372a372067239a4b5f44efc595e0f8aca1))
* **cua-bench:** pin bench-osworld to verified-20261003-fe6d89d ([#4580](https://github.com/trycua/cua/issues/4580)) ([94e7e9f](https://github.com/trycua/cua/commit/94e7e9fbc1b1bf467ff93f272dccc527f5d61dc6)), closes [#4519](https://github.com/trycua/cua/issues/4519)
* **cua-bench:** raise when the sandbox does not answer in file_exists/directory_exists ([#5194](https://github.com/trycua/cua/issues/5194)) ([1ff5710](https://github.com/trycua/cua/commit/1ff57101e705d5964ca344e26207366a94467dee))
* **cua-bench:** reject missing, nonfinite and nonnumeric evaluator rewards ([#5163](https://github.com/trycua/cua/issues/5163)) ([341d1e1](https://github.com/trycua/cua/commit/341d1e17237815340991376828992115a64ee15c))
* lock the cua SDK checkout at 0.3.1 ([#4593](https://github.com/trycua/cua/issues/4593)) ([fcb386f](https://github.com/trycua/cua/commit/fcb386f4f958d1357af31ccb0056376f06d9db37))
* **python:** cua-sandbox, cua-bench and the playground no longer depend on Fleet; remove cua-train (CUA-1834) ([#5445](https://github.com/trycua/cua/issues/5445)) ([7733289](https://github.com/trycua/cua/commit/773328941ead1b83a616e8b6c5a5ac4851af7114))
* stop Cua Cloud (on="cloud") with a clear "Cua Cloud has ended" error (CUA-1854) ([#5415](https://github.com/trycua/cua/issues/5415)) ([5d78ec8](https://github.com/trycua/cua/commit/5d78ec8b7705d942a3d260b276bcce76614a634c))
