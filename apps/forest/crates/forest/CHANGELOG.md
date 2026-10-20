# Changelog

## [0.3.16](https://github.com/understory-io/forest/compare/v0.3.15...v0.3.16) (2026-10-20)


### Features

* **global:** `FOREST_PRERELEASE=1` follows prereleases for every unpinned tool ([#326](https://github.com/understory-io/forest/issues/326)) ([54ead1e](https://github.com/understory-io/forest/commit/54ead1e0bf7423f8e291cb103009653f1e5526d1))

## [0.3.15](https://github.com/understory-io/forest/compare/v0.3.14...v0.3.15) (2026-10-20)


### Features

* **components:** `forest components version` for prebuilt builds, and fixes from the agentctl prerelease review ([#325](https://github.com/understory-io/forest/issues/325)) ([d3535da](https://github.com/understory-io/forest/commit/d3535da70b45bc308a754d8c539e9d66ac4e2362))
* **global:** follow a tool's prereleases only when asked, with `add --prerelease` ([#324](https://github.com/understory-io/forest/issues/324)) ([74a78e1](https://github.com/understory-io/forest/commit/74a78e12286f5c6922531ad62354b36719a46764))


### Bug Fixes

* **registry:** send a download's first message with the first bytes, so range fan-out starts at TTFB ([#320](https://github.com/understory-io/forest/issues/320)) ([1fde57c](https://github.com/understory-io/forest/commit/1fde57c91200c438e76a172cc8e179dfd86d067a))

## [0.3.14](https://github.com/understory-io/forest/compare/v0.3.13...v0.3.14) (2026-10-19)


### Features

* **auth:** let an OAuth app publish components with a client-credentials token, and nothing else ([#307](https://github.com/understory-io/forest/issues/307)) ([d240bc1](https://github.com/understory-io/forest/commit/d240bc12621d447ac8dba01a889801045627442e))
* **auth:** let an OAuth app sign a person in with a device code (RFC 8628) ([#317](https://github.com/understory-io/forest/issues/317)) ([566b8a5](https://github.com/understory-io/forest/commit/566b8a59141c5581c857c4e34be78a0bbb388b11))
* **auth:** public OAuth clients (PKCE, loopback), and introspecting user-delegated tokens instead of sessions ([#313](https://github.com/understory-io/forest/issues/313)) ([77bc41c](https://github.com/understory-io/forest/commit/77bc41c0044cce51a9f8bcc5e040259610aeae13))
* **auth:** token introspection for resource servers, and the data gateway's scopes ([#311](https://github.com/understory-io/forest/issues/311)) ([8c7674c](https://github.com/understory-io/forest/commit/8c7674c22b0353d797fd69f0281a3e7bd035066d))
* **cli:** `forest organisation oauth-app update` changes an app from the CLI ([79aa1e7](https://github.com/understory-io/forest/commit/79aa1e7746516d5e01aacc1a6d6be245600a10d0))
* **cli:** `forest organisation oauth-app update` changes an app from the CLI ([#318](https://github.com/understory-io/forest/issues/318)) ([79aa1e7](https://github.com/understory-io/forest/commit/79aa1e7746516d5e01aacc1a6d6be245600a10d0))
* **forage:** add a maintenance mode that answers 503 while keeping health probes up ([#305](https://github.com/understory-io/forest/issues/305)) ([e960fba](https://github.com/understory-io/forest/commit/e960fba77a0e585837be4879279145976df3d935))
* **forage:** introspect a person's token as well as an app's ([#312](https://github.com/understory-io/forest/issues/312)) ([b41b4b5](https://github.com/understory-io/forest/commit/b41b4b597c19d2008b6fe752e13fb3be70e32f1b))
* **forage:** project Settings hub — browse Triggers/Policies/Pipelines with a back button ([#260](https://github.com/understory-io/forest/issues/260)) ([54092cc](https://github.com/understory-io/forest/commit/54092cc4aa82af31babf87f93838454de79f6ad6))
* **forage:** redesign the release swimlane, and delete the renderer it replaced ([#264](https://github.com/understory-io/forest/issues/264)) ([7716620](https://github.com/understory-io/forest/commit/77166202d49292ab581e1f132b46d4d6bc5900f8))
* **forage:** render gate stages, and stop generating provider bindings forage never uses (DATA-775) ([#258](https://github.com/understory-io/forest/issues/258)) ([ad075be](https://github.com/understory-io/forest/commit/ad075bee05a921473551239ce6337fb1c5d154d3))
* **forage:** report how long each page takes, per route, over OTLP ([#267](https://github.com/understory-io/forest/issues/267)) ([b40261e](https://github.com/understory-io/forest/commit/b40261e4a4b331d6bceab04faea5b845eb95ea0c))
* **forage:** restore organisation rule settings ([a78ab89](https://github.com/understory-io/forest/commit/a78ab8949cbfaca1d7570e98b0c9d7c306a8c098))
* **forage:** wire up the supersede-pending policy (DATA-823) ([#263](https://github.com/understory-io/forest/issues/263)) ([cdc65d1](https://github.com/understory-io/forest/commit/cdc65d1ca61c629a637bba5196027fcd14ca50ff))
* **forest:** skip-to-latest — supersede older pending releases (DATA-817) ([#261](https://github.com/understory-io/forest/issues/261)) ([0d5534d](https://github.com/understory-io/forest/commit/0d5534da199d32e9d2ec9137663c58b90c7271cb))
* **pipelines:** a gate stage that waits for evidence, not a duration ([#252](https://github.com/understory-io/forest/issues/252)) ([#254](https://github.com/understory-io/forest/issues/254)) ([5c5fa66](https://github.com/understory-io/forest/commit/5c5fa66b3eb6ac8dead31a63a0497e3148a273bd))
* prepare Mire event-store migration ([fb7cc51](https://github.com/understory-io/forest/commit/fb7cc51380c034558f5087d731329a33f34894f3))
* **provider:** let a provider report signals on the stream it already has ([#252](https://github.com/understory-io/forest/issues/252)) ([#255](https://github.com/understory-io/forest/issues/255)) ([7f91e95](https://github.com/understory-io/forest/commit/7f91e954b6af52a38b1f2c6c543e843068468c5c))
* **registry:** fetch a large binary as parallel byte ranges, and let the configured HTTP/2 windows apply ([45dff62](https://github.com/understory-io/forest/commit/45dff626fdec7cc3326e0b8a3f45fbbca0d50888))
* **registry:** fetch a large binary as parallel byte ranges, and let the configured HTTP/2 windows apply ([#319](https://github.com/understory-io/forest/issues/319)) ([45dff62](https://github.com/understory-io/forest/commit/45dff626fdec7cc3326e0b8a3f45fbbca0d50888))
* **signals:** a release can be told what happened to it ([#252](https://github.com/understory-io/forest/issues/252)) ([#253](https://github.com/understory-io/forest/issues/253)) ([07cf19c](https://github.com/understory-io/forest/commit/07cf19c4e44ec79094f32e3d1f9ba266916f3e71))
* **terraform@1:** assume a role when the destination names one (DATA-732) ([9529edb](https://github.com/understory-io/forest/commit/9529edb4387483cc56e37d21cd24bf9601166c8f))
* **terraform@1:** destroy plan stages and a drift check ([#286](https://github.com/understory-io/forest/issues/286)) ([#299](https://github.com/understory-io/forest/issues/299)) ([794a4ed](https://github.com/understory-io/forest/commit/794a4ed60ff171a8de1d7fecb508bf09868a29ed))
* **terraform@1:** reach a project's state outside a release ([#282](https://github.com/understory-io/forest/issues/282)) ([#295](https://github.com/understory-io/forest/issues/295)) ([0ca01da](https://github.com/understory-io/forest/commit/0ca01da576b9dd73ce5cd1ead9be1cf4f3ddc041))


### Bug Fixes

* **ci:** pull minio from Chainguard; quay now refuses anonymous pulls ([#297](https://github.com/understory-io/forest/issues/297)) ([8ff3860](https://github.com/understory-io/forest/commit/8ff386001e4b3dae75d853a7882dc43350d0b473))
* **ci:** track rust:1.98-trixie, a tag that exists (DATA-739) ([950b979](https://github.com/understory-io/forest/commit/950b979ab854f96c098becdec15275f547eb3322))
* **cli:** name the server publish uploads to, and accept --forest-server after the subcommand ([#301](https://github.com/understory-io/forest/issues/301)) ([1dc0370](https://github.com/understory-io/forest/commit/1dc03703c7d8ab9eaf52c65bc6efefa3f708c078))
* **cli:** show a plan parked awaiting approval and exit, instead of waiting on the approver ([#302](https://github.com/understory-io/forest/issues/302)) ([52685f3](https://github.com/understory-io/forest/commit/52685f38f257a80cdc26673ccdd728dec847bd9d))
* **codegen:** serde defaults for defaulted structs, lists, maps and enums ([#287](https://github.com/understory-io/forest/issues/287)) ([#290](https://github.com/understory-io/forest/issues/290)) ([a8539cd](https://github.com/understory-io/forest/commit/a8539cd5f652f49848de4cfd4fc39873f994ab22))
* **destinations:** an undeclared project no longer widens onto kinds it never renders ([#288](https://github.com/understory-io/forest/issues/288)) ([#291](https://github.com/understory-io/forest/issues/291)) ([8257d3c](https://github.com/understory-io/forest/commit/8257d3c0a662cb570e260d9dedfd436cd2f6f5e2))
* **flux@1:** retry a GitOps push that lost a race with another release ([#306](https://github.com/understory-io/forest/issues/306)) ([1a55582](https://github.com/understory-io/forest/commit/1a555826d87d5b3c94bceafe5c4f50f6b7e7d4a9))
* **flux@1:** say when the reconcile webhook failed, retry it, and stop logging its token ([#298](https://github.com/understory-io/forest/issues/298)) ([#300](https://github.com/understory-io/forest/issues/300)) ([58fb48a](https://github.com/understory-io/forest/commit/58fb48a797d68c637f28543e701c4028be90e40f))
* **forage:** a swimlane bubble answers for itself, and halve the JS bundle ([#265](https://github.com/understory-io/forest/issues/265)) ([0911b7b](https://github.com/understory-io/forest/commit/0911b7b7251b860422cd3a22dca85e7fab6b5504))
* **forage:** address release rules by id, not by name ([#259](https://github.com/understory-io/forest/issues/259)) ([2e118ba](https://github.com/understory-io/forest/commit/2e118bad8d66d4fc80c60ea04ec925b967b887b1))
* **forage:** give a failed stage's error a block, not a cell in the row ([#271](https://github.com/understory-io/forest/issues/271)) ([6bc4139](https://github.com/understory-io/forest/commit/6bc4139c06384c05260abcfb86633df66e7dde68))
* **forage:** give the request-duration histogram buckets scaled for seconds ([#268](https://github.com/understory-io/forest/issues/268)) ([a221b12](https://github.com/understory-io/forest/commit/a221b1209185e315f4ec30b5fda053f76596241d))
* **forage:** report the build time as RFC 3339 when CI stamps Unix seconds ([#304](https://github.com/understory-io/forest/issues/304)) ([9dde283](https://github.com/understory-io/forest/commit/9dde2832486c1284b256b6b3e1a4904dc5dd8c94))
* **forage:** say why an organisation could not be created, not just that it failed ([#303](https://github.com/understory-io/forest/issues/303)) ([0862ff4](https://github.com/understory-io/forest/commit/0862ff4f01b589ac78111953bbad1bc7f1d40aa9))
* **forage:** show the plan on the page that asks you to approve it ([#270](https://github.com/understory-io/forest/issues/270)) ([f47c433](https://github.com/understory-io/forest/commit/f47c43339ab9aa0de3f24a1d432f7816529efa4c))
* **forage:** the swim lane says a placement failed and links to why ([#272](https://github.com/understory-io/forest/issues/272)) ([a01055d](https://github.com/understory-io/forest/commit/a01055d9c1a1a19f60a2a386b19774643e4b8461))
* **forest-server:** pin OpenTofu, and document config-driven adoption ([#283](https://github.com/understory-io/forest/issues/283)) ([#289](https://github.com/understory-io/forest/issues/289)) ([e70a4f5](https://github.com/understory-io/forest/commit/e70a4f59bb77db54636216e7432274d4c98e9b6d))
* **forest:** give each acceptance test its own database pool ([bdf677f](https://github.com/understory-io/forest/commit/bdf677fbb6f814198f3335b427eb849e4c85b2fe))
* **forest:** look up a binary in a given search path, not by overwriting PATH ([f0b77cc](https://github.com/understory-io/forest/commit/f0b77ccdf20b6e8dfb06c99b3c88bb3319642653))
* **forest:** make the test suite compile again ([1ae3ff0](https://github.com/understory-io/forest/commit/1ae3ff047b587b4e77e57a1216797e84056ee165))
* **forest:** name the branch the flux git test pushes to ([2546797](https://github.com/understory-io/forest/commit/2546797a91ac60a41835bb40bb97f5c21d2d9eb8))
* **forest:** publish names the context it publishes to, not the active one ([#309](https://github.com/understory-io/forest/issues/309)) ([de69341](https://github.com/understory-io/forest/commit/de69341fcb7df8314416c607b1c4ef477629db2f))
* **forest:** pull minio from quay, the registry that still serves it ([2a73fd8](https://github.com/understory-io/forest/commit/2a73fd834742af3757c52f8e716009781660e1d6))
* **forest:** regenerate the sqlx offline cache, and delete the stale copy at the workspace root ([126a50f](https://github.com/understory-io/forest/commit/126a50f6056b99f32f81bb0badd7bcd1e164a372))
* **forest:** stop the cache reporting a miss for an entry that just landed ([e1a04b2](https://github.com/understory-io/forest/commit/e1a04b2a45cd05f2e3f72397b830043b2891be43))
* install a rustls provider and add mire-migrate for the cutover (DATA-739) ([90ee45b](https://github.com/understory-io/forest/commit/90ee45b863fe12aa5acfb4535b0016ae3b8c3151))
* **service-release:** install deno for components with deno hooks ([#308](https://github.com/understory-io/forest/issues/308)) ([ff285e6](https://github.com/understory-io/forest/commit/ff285e66b6e2f79c6949d88f23af2d43323d7eaf))
* **terraform@1:** apply the plan that was approved, not a fresh one ([#284](https://github.com/understory-io/forest/issues/284)) ([#292](https://github.com/understory-io/forest/issues/292)) ([bd11eaa](https://github.com/understory-io/forest/commit/bd11eaad7440a6488320f5637ec899c5b02fd291))
* **terraform@1:** one state per destination, without moving any existing state ([#285](https://github.com/understory-io/forest/issues/285)) ([#294](https://github.com/understory-io/forest/issues/294)) ([9516605](https://github.com/understory-io/forest/commit/95166053e915dfb8e14dad440adba296deeaaea2))
* **terraform@1:** the state server checks the credential it issues ([#281](https://github.com/understory-io/forest/issues/281)) ([#293](https://github.com/understory-io/forest/issues/293)) ([7c28e21](https://github.com/understory-io/forest/commit/7c28e211d75c8b0474279a95e7008271562fd70b))


### Performance Improvements

* **ci:** stop paying 32 cores for work that is not happening ([#274](https://github.com/understory-io/forest/issues/274)) ([9cde291](https://github.com/understory-io/forest/commit/9cde29141f4416b4247f497abc743bd6d53c8770))
* **forage:** start the timeline request while the bundle is still downloading ([#266](https://github.com/understory-io/forest/issues/266)) ([188d74c](https://github.com/understory-io/forest/commit/188d74cf5d14251a00c05ced9a8bda1ad8de027c))
* **forest-server:** stop the acceptance suite paying production password cost ([#280](https://github.com/understory-io/forest/issues/280)) ([f6fc6a8](https://github.com/understory-io/forest/commit/f6fc6a85d8340c75f0e70dcd119043b1955c6e71))
* **forest:** fetch every annotation's destinations in one query ([#269](https://github.com/understory-io/forest/issues/269)) ([f881bae](https://github.com/understory-io/forest/commit/f881baeb26bc49811adffb188aeadc249f3ddc40))

## [0.3.13](https://github.com/understory-io/forest/compare/v0.3.12...v0.3.13) (2026-09-07)


### Features

* **release:** let --set reach the deployment config block (DATA-710) ([#239](https://github.com/understory-io/forest/issues/239)) ([d9f7861](https://github.com/understory-io/forest/commit/d9f78619d8949a4509f47b59e15ccebb0ba247a8))


### Bug Fixes

* **notifications:** personal release notifications only reach the release owner (DATA-723) ([#241](https://github.com/understory-io/forest/issues/241)) ([03071ce](https://github.com/understory-io/forest/commit/03071ce84830cdf8c7ea910b25e103c2a5b53006))

## [0.3.12](https://github.com/understory-io/forest/compare/v0.3.11...v0.3.12) (2026-09-04)


### Features

* **auth:** show a QR for the device-login URL ([06e3c31](https://github.com/understory-io/forest/commit/06e3c3137f3001efc120f9654e04a10603fb5236))
* **service-release:** let a release pin the image it deploys ([da473f6](https://github.com/understory-io/forest/commit/da473f6a389f498e8219ad6a9a0483fbfc44650d))


### Bug Fixes

* **forage:** order dashboard activity by release time, not by project ([#233](https://github.com/understory-io/forest/issues/233)) ([a3fac65](https://github.com/understory-io/forest/commit/a3fac65d6fcd803183812b62c5dd915acfcfdcf8))
* **publish:** one handle for a component's files, so templates actually ship ([9437ed9](https://github.com/understory-io/forest/commit/9437ed9ddc1d921bcf857914fa64b7575408bcfc))
* **publish:** stage the payload under .forest, and let a component declare its paths ([aace81b](https://github.com/understory-io/forest/commit/aace81b962636f3e90dd0e50477a99941555942f))
* **release:** build the linux binaries against an older glibc ([c160541](https://github.com/understory-io/forest/commit/c160541b320dd297acc998db66f810636692b730))

## [0.3.11](https://github.com/understory-io/forest/compare/v0.3.10...v0.3.11) (2026-09-02)


### Bug Fixes

* **forest-server:** respect the destination types a project declared ([#231](https://github.com/understory-io/forest/issues/231)) ([9d48b17](https://github.com/understory-io/forest/commit/9d48b17a0ec4bac793d39b7e469762917bd466b8))

## [0.3.10](https://github.com/understory-io/forest/compare/v0.3.9...v0.3.10) (2026-09-01)


### Features

* **forage:** fuzzy type-to-filter search in the breadcrumb switchers ([#222](https://github.com/understory-io/forest/issues/222)) ([a29e106](https://github.com/understory-io/forest/commit/a29e106fdad1d580c0d19937669b9d8dafdf8188))
* **sdk:** a project can say where it deploys ([#223](https://github.com/understory-io/forest/issues/223)) ([928f3e1](https://github.com/understory-io/forest/commit/928f3e1d1f5b3cfe955287fcd58f3b605827e0c9))


### Bug Fixes

* **forest-server:** release to the destinations a project declared, not the whole environment ([#229](https://github.com/understory-io/forest/issues/229)) ([8d40ce6](https://github.com/understory-io/forest/commit/8d40ce6d0fa1bf2c059ed7a31f6fd761dcc3ed83))
* **server:** publish the CUE module for components that ship their own cue.mod ([#225](https://github.com/understory-io/forest/issues/225)) ([cc39058](https://github.com/understory-io/forest/commit/cc3905839a839de1e49ee85079c0b9ac7e999e7f))

## [0.3.9](https://github.com/understory-io/forest/compare/v0.3.8...v0.3.9) (2026-08-31)


### Features

* **ci:** a reusable workflow for releasing a service that already exists ([#220](https://github.com/understory-io/forest/issues/220)) ([6d6b0a2](https://github.com/understory-io/forest/commit/6d6b0a2e822f14b396735a6c7d7532e0cb2f7c03))
* **ci:** track service-release through a mutable `stable` branch ([#221](https://github.com/understory-io/forest/issues/221)) ([77de6fa](https://github.com/understory-io/forest/commit/77de6fa2d9462548f33f9ac5f197769b0fb289c0))


### Bug Fixes

* **ci:** verify a published component parses, and republish the three that did not ([#218](https://github.com/understory-io/forest/issues/218)) ([75229ab](https://github.com/understory-io/forest/commit/75229ab2c9e098f455931427814247ce45f10ae8))

## [0.3.8](https://github.com/understory-io/forest/compare/v0.3.7...v0.3.8) (2026-08-31)


### Features

* add organisation rule sets ([32187a8](https://github.com/understory-io/forest/commit/32187a8b0d623ce212bfca7c9fa6c927533d175e))


### Bug Fixes

* **forest:** a project can declare destinations without a component ([#213](https://github.com/understory-io/forest/issues/213)) ([0d42f3f](https://github.com/understory-io/forest/commit/0d42f3f17bb29512b3e136c0b4c565ac886a82be))
* **forest:** evaluate CUE from the directory that owns the module ([#215](https://github.com/understory-io/forest/issues/215)) ([fb6cdb4](https://github.com/understory-io/forest/commit/fb6cdb4590d3dc58c479dada6490008672d00782))
* **forest:** publish a component's cue.mod/module.cue ([#212](https://github.com/understory-io/forest/issues/212)) ([e347387](https://github.com/understory-io/forest/commit/e347387030cbcb6f191a1febaa1b0855354d5303))

## [0.3.7](https://github.com/understory-io/forest/compare/v0.3.6...v0.3.7) (2026-08-31)


### Features

* add organisation rule sets ([32187a8](https://github.com/understory-io/forest/commit/32187a8b0d623ce212bfca7c9fa6c927533d175e))
* detect ci release context ([ad35401](https://github.com/understory-io/forest/commit/ad35401085e455973d3328d0bed76acc38165ac4))


### Bug Fixes

* **forest:** a project can declare destinations without a component ([#213](https://github.com/understory-io/forest/issues/213)) ([0d42f3f](https://github.com/understory-io/forest/commit/0d42f3f17bb29512b3e136c0b4c565ac886a82be))
* **forest:** publish a component's cue.mod/module.cue ([#212](https://github.com/understory-io/forest/issues/212)) ([e347387](https://github.com/understory-io/forest/commit/e347387030cbcb6f191a1febaa1b0855354d5303))

## [0.3.6](https://github.com/understory-io/forest/compare/v0.3.5...v0.3.6) (2026-08-28)


### Features

* **forest-server:** a generic destination is a place, projects supply the rest ([#209](https://github.com/understory-io/forest/issues/209)) ([fe6d718](https://github.com/understory-io/forest/commit/fe6d718c9b3771c1bcd576bb5e18804344aa779b))


### Bug Fixes

* **forest-server:** notify the author, not the token owner ([#208](https://github.com/understory-io/forest/issues/208)) ([5aeb972](https://github.com/understory-io/forest/commit/5aeb9724373414f34f1904160ec0e0b0e3e7cfd9))

## [0.3.5](https://github.com/understory-io/forest/compare/v0.3.4...v0.3.5) (2026-08-28)


### Features

* **ci:** publish a component's binaries as one step ([#205](https://github.com/understory-io/forest/issues/205)) ([6a5b3bd](https://github.com/understory-io/forest/commit/6a5b3bd5541e3edf4b96ee0a7d527ee05b71f6d5))

## [0.3.4](https://github.com/understory-io/forest/compare/v0.3.3...v0.3.4) (2026-08-28)


### Features

* **release:** attribute a release to whoever wrote the commit ([#203](https://github.com/understory-io/forest/issues/203)) ([3ffb533](https://github.com/understory-io/forest/commit/3ffb533fe5d186aed9926ec5b5c58be85fa22a98))

## [0.3.3](https://github.com/understory-io/forest/compare/v0.3.2...v0.3.3) (2026-08-28)


### Features

* **forest-server:** forest/generic@1 — destination types implemented by an external provider ([#200](https://github.com/understory-io/forest/issues/200)) ([27657b9](https://github.com/understory-io/forest/commit/27657b97a1ca7fc2f98d0a587678285c11603731))


### Bug Fixes

* **forest-server:** --destination selects destinations, it does not add them ([#202](https://github.com/understory-io/forest/issues/202)) ([388d4c3](https://github.com/understory-io/forest/commit/388d4c3be6cf6aef0a786b83b036f54f9e4b505c))

## [0.3.2](https://github.com/understory-io/forest/compare/v0.3.1...v0.3.2) (2026-08-27)


### Features

* **publish:** warn when a tool ships only one platform ([#125](https://github.com/understory-io/forest/issues/125)) ([1f62387](https://github.com/understory-io/forest/commit/1f623878e31072bfa0bfac24caeeb269e5fe54a0))

## [0.3.1](https://github.com/understory-io/forest/compare/v0.3.0...v0.3.1) (2026-08-27)


### Bug Fixes

* **forage:** clear a superseded lane warning once a newer release goes live ([#197](https://github.com/understory-io/forest/issues/197)) ([089b92f](https://github.com/understory-io/forest/commit/089b92ff2bc7b83e00f021521ab5e6e2bce02c64))

## [0.3.0](https://github.com/understory-io/forest/compare/v0.2.20...v0.3.0) (2026-08-26)


### ⚠ BREAKING CHANGES

* **publish:** `forest publish` now fails when `.forest/component/output/` is empty instead of falling back to a binary found elsewhere in the working tree, and `forest run` / `forest validate` no longer resolve a component from a cargo `target/debug` or `target/release` build. Run `forest run build` first; for the forest-contrib build components, `mise run contrib:stage`.

### Features

* **directory:** machine-facing lookup from external identity to linked accounts ([#181](https://github.com/understory-io/forest/issues/181)) ([975a0b0](https://github.com/understory-io/forest/commit/975a0b04550671d97765b160ae071d2de6c5eca4))
* **forage:** Releases view shows last 20 with "Show more" (DATA-659) ([#185](https://github.com/understory-io/forest/issues/185)) ([b176a35](https://github.com/understory-io/forest/commit/b176a353b20d99483f23edeaebc231e4d21f1d30))
* **forage:** tell live, in-flight and past deploys apart on the lanes (DATA-661) ([#187](https://github.com/understory-io/forest/issues/187)) ([19bd7ec](https://github.com/understory-io/forest/commit/19bd7ec44e20c9c8c4fe4cc6a960e664ad5e0b18))


### Bug Fixes

* **destination:** let a destination that predates the event store be updated ([#180](https://github.com/understory-io/forest/issues/180)) ([d996b66](https://github.com/understory-io/forest/commit/d996b66302ae4cd3cfd8cf8a7ef62bcc5b88d97c))
* **directory:** match either provider spelling, whichever the row has ([#184](https://github.com/understory-io/forest/issues/184)) ([b6c13f9](https://github.com/understory-io/forest/commit/b6c13f99a249d68fea56a5f6b413016f8724e32a))
* **directory:** match the provider spelling the write path actually stores ([#183](https://github.com/understory-io/forest/issues/183)) ([24febc4](https://github.com/understory-io/forest/commit/24febc46f86988d13d1a8bdd8df3d29b2e5b54a9))
* **forage:** keep superseded releases on the timeline (DATA-660) ([#186](https://github.com/understory-io/forest/issues/186)) ([5d957d2](https://github.com/understory-io/forest/commit/5d957d2ab30d188a33f10cd209d3b079329c1faf))
* **forest:** return a project's whole release history, not the last 50 (DATA-662) ([#188](https://github.com/understory-io/forest/issues/188)) ([530dda8](https://github.com/understory-io/forest/commit/530dda81d49107b0f5f108193eb018be73333c62))
* **publish:** publish only from `.forest`, and delete the cargo `target/` probe (DATA-654) ([#175](https://github.com/understory-io/forest/issues/175)) ([39e07f9](https://github.com/understory-io/forest/commit/39e07f9218c4127e36afeec24c4de880d6af120d))

## [0.2.20](https://github.com/understory-io/forest/compare/v0.2.19...v0.2.20) (2026-08-25)


### Bug Fixes

* **destination:** stop 'update --metadata' deleting the keys it does not name ([#176](https://github.com/understory-io/forest/issues/176)) ([54b165d](https://github.com/understory-io/forest/commit/54b165d9b4200d851e904e4c5dac1940198d3eb3))
* **release:** commit the synced Cargo.lock instead of only building with it ([#174](https://github.com/understory-io/forest/issues/174)) ([c80fbeb](https://github.com/understory-io/forest/commit/c80fbeb6c25c99c8e0d4c2d50790ee7ec344928e))
* **release:** sync Cargo.lock without cargo, which cannot run there ([#178](https://github.com/understory-io/forest/issues/178)) ([22e1bf6](https://github.com/understory-io/forest/commit/22e1bf6de7eb739a3dd582cfe90d265a3cf70d2e))

## [0.2.19](https://github.com/understory-io/forest/compare/v0.2.18...v0.2.19) (2026-08-25)


### Features

* **forage:** mark an existing metadata key sensitive from the UI (one-way) ([#170](https://github.com/understory-io/forest/issues/170)) ([d217689](https://github.com/understory-io/forest/commit/d217689893bca74b1d41aad5b27fb6d888828851))
* **release:** give annotate machine-readable output, and reconcile stuck intents ([#169](https://github.com/understory-io/forest/issues/169)) ([51c3e79](https://github.com/understory-io/forest/commit/51c3e7915c64697421b5059e69dff2f2dd4aa456))

## [0.2.18](https://github.com/understory-io/forest/compare/v0.2.17...v0.2.18) (2026-08-25)


### Features

* **oauth:** expose grant types in the UI, and let them be changed ([#165](https://github.com/understory-io/forest/issues/165)) ([91b4082](https://github.com/understory-io/forest/commit/91b4082edd24d380e3572fac693e0c3fa05330cd))


### Bug Fixes

* **release:** finalize the intent when a release is reported failed ([#164](https://github.com/understory-io/forest/issues/164)) ([b03f0b1](https://github.com/understory-io/forest/commit/b03f0b186c23e34852b7905dab4f43c6299bb475))

## [0.2.17](https://github.com/understory-io/forest/compare/v0.2.16...v0.2.17) (2026-08-25)


### Bug Fixes

* **destination:** mark reconcile_url sensitive; point db:prepare at the cache CI reads ([#154](https://github.com/understory-io/forest/issues/154)) ([afe424e](https://github.com/understory-io/forest/commit/afe424e075c4a47fd268bb8fbe6a5b06339e34a7))
* **oauth:** enforce grant_types on the authorization-code flow too ([#161](https://github.com/understory-io/forest/issues/161)) ([4836bd0](https://github.com/understory-io/forest/commit/4836bd0979dda5c7528ac98858925af5991c1b47))
* **release:** record a failed external deploy as a release, not a notification ([#163](https://github.com/understory-io/forest/issues/163)) ([40fe966](https://github.com/understory-io/forest/commit/40fe96665e33f4ef025d93b289bd76e32c380725))

## [0.2.16](https://github.com/understory-io/forest/compare/v0.2.15...v0.2.16) (2026-08-24)


### Features

* **oauth:** add the client_credentials grant for machine-to-machine auth ([#158](https://github.com/understory-io/forest/issues/158)) ([355eda0](https://github.com/understory-io/forest/commit/355eda05b92632643331f7a2ade2392df2ac0207))
* **release:** say a release is pending, and let CI report it failed (DATA-637) ([#160](https://github.com/understory-io/forest/issues/160)) ([10be0a8](https://github.com/understory-io/forest/commit/10be0a84c09fbaebbedde964dc0932baffb673ba))

## [0.2.15](https://github.com/understory-io/forest/compare/v0.2.14...v0.2.15) (2026-08-21)


### Features

* **destination:** withhold sensitive metadata values, reveal per key ([#152](https://github.com/understory-io/forest/issues/152)) ([0782568](https://github.com/understory-io/forest/commit/0782568b31bfa2c4bc686188d84a4fc3c19baf95))


### Bug Fixes

* **server:** report the latest stable version, not a higher prerelease ([#156](https://github.com/understory-io/forest/issues/156)) ([1821fef](https://github.com/understory-io/forest/commit/1821fef80a7e6a2bf033c7712b8eab6048cc04e7))
* **version:** range specs no longer resolve to a prerelease ([#155](https://github.com/understory-io/forest/issues/155)) ([8200bbe](https://github.com/understory-io/forest/commit/8200bbeecbe6f2fbf358dab2d6d3e6b8b69a288c))

## [0.2.14](https://github.com/understory-io/forest/compare/v0.2.13...v0.2.14) (2026-08-20)


### Bug Fixes

* **shell:** stop a tool snippet's compdef call from erroring at startup (DATA-588) ([#150](https://github.com/understory-io/forest/issues/150)) ([fc8d05c](https://github.com/understory-io/forest/commit/fc8d05c0e394b77d325cc7c94e3d1f91e805499a))

## [0.2.13](https://github.com/understory-io/forest/compare/v0.2.12...v0.2.13) (2026-08-20)


### Bug Fixes

* **ci:** install the newest forest release that is actually published ([#146](https://github.com/understory-io/forest/issues/146)) ([184550a](https://github.com/understory-io/forest/commit/184550a499892dfc2e0b16a4df3150f372eed6cf))
* **global:** stop version bumps from deleting shell integrations, add an opt-out (DATA-588) ([#148](https://github.com/understory-io/forest/issues/148)) ([80f207c](https://github.com/understory-io/forest/commit/80f207c5af4c78532b6050b174384d4f63c57804))

## [0.2.12](https://github.com/understory-io/forest/compare/v0.2.11...v0.2.12) (2026-08-19)


### Bug Fixes

* **ci:** install cue as a plain binary so build-component publishes hit prod ([#143](https://github.com/understory-io/forest/issues/143)) ([57c4329](https://github.com/understory-io/forest/commit/57c4329721ac6374c5cbd7adcb8302fe9c8b0ca9))
* **publish:** honour --dry-run on the external path and for metadata sync (DATA-588) ([#145](https://github.com/understory-io/forest/issues/145)) ([b95e7a4](https://github.com/understory-io/forest/commit/b95e7a40d7d264be9a4440692f6e2ed85935d6c9))

## [0.2.11](https://github.com/understory-io/forest/compare/v0.2.10...v0.2.11) (2026-08-19)


### Bug Fixes

* **ci:** give each build leg's tarball a unique filename ([#141](https://github.com/understory-io/forest/issues/141)) ([fd81c80](https://github.com/understory-io/forest/commit/fd81c80844e5502885bb81505190f2bd996d85a5))
* **ci:** set CUE_REGISTRY explicitly in the publish workflows ([#138](https://github.com/understory-io/forest/issues/138)) ([0e4892e](https://github.com/understory-io/forest/commit/0e4892ecff3e7f69be71248b1fed1d1b01af620a))
* **global:** capture shell integration for already-cached tools (DATA-588) ([#140](https://github.com/understory-io/forest/issues/140)) ([b9d0447](https://github.com/understory-io/forest/commit/b9d044746a8504477e1dbd01e2807300a2962419))

## [0.2.10](https://github.com/understory-io/forest/compare/v0.2.9...v0.2.10) (2026-08-19)


### Features

* **global:** let components declare their own shell integration (DATA-588) ([#133](https://github.com/understory-io/forest/issues/133)) ([2ae3ccd](https://github.com/understory-io/forest/commit/2ae3ccdca1c054faa8b4d40765ca838bad3be57d))


### Bug Fixes

* **ci:** derive CUE_REGISTRY in the publish workflow's resolve step ([#131](https://github.com/understory-io/forest/issues/131)) ([d64e588](https://github.com/understory-io/forest/commit/d64e588df1cbf3bbebe0a787460bfa6470085f01))
* **server:** a prerelease version no longer breaks the component detail view ([#134](https://github.com/understory-io/forest/issues/134)) ([bfd6b16](https://github.com/understory-io/forest/commit/bfd6b1679c6f3588bd4274579ecd793664f8bcf2))

## [0.2.9](https://github.com/understory-io/forest/compare/v0.2.8...v0.2.9) (2026-08-19)


### Features

* **forest:** publish components from CI on a tagged release ([#128](https://github.com/understory-io/forest/issues/128)) ([de49361](https://github.com/understory-io/forest/commit/de493613a4aeb33b27e8bccabcb22bc516d28492))

## [0.2.8](https://github.com/understory-io/forest/compare/v0.2.7...v0.2.8) (2026-08-12)


### Features

* **global:** store component binaries under &lt;hash&gt;/&lt;name&gt; so argv0 is the real name (DATA-510) ([#127](https://github.com/understory-io/forest/issues/127)) ([84c95ca](https://github.com/understory-io/forest/commit/84c95ca82754ce4dd3f4fa4b6f86e29d82fdb928))
* **observability:** surface build provenance and page timings (DATA-505) ([#123](https://github.com/understory-io/forest/issues/123)) ([c591321](https://github.com/understory-io/forest/commit/c591321fe41f26519c9ab443d702163283091608))

## [0.2.7](https://github.com/understory-io/forest/compare/v0.2.6...v0.2.7) (2026-08-12)


### Features

* **build:** stamp the component version into built binaries ([#120](https://github.com/understory-io/forest/issues/120)) ([fb2d8f9](https://github.com/understory-io/forest/commit/fb2d8f9e6b544901098c1075f9efae201317fe93))


### Performance Improvements

* **downloads:** stream binaries and fan out registry calls (DATA-505) ([#122](https://github.com/understory-io/forest/issues/122)) ([80f1585](https://github.com/understory-io/forest/commit/80f1585b23b7819388b4a1a12ce410b57c35dbf5))

## [0.2.6](https://github.com/understory-io/forest/compare/v0.2.5...v0.2.6) (2026-08-11)


### Features

* **publish:** upload every built platform, and allow cgo for Go builds ([#117](https://github.com/understory-io/forest/issues/117)) ([e1659c1](https://github.com/understory-io/forest/commit/e1659c1643e3079e7f05d603eb64c81883856c7e))

## [0.2.5](https://github.com/understory-io/forest/compare/v0.2.4...v0.2.5) (2026-07-15)


### Features

* **forest:** add fish as option for 'forest shell' ([#116](https://github.com/understory-io/forest/issues/116)) ([8bbc462](https://github.com/understory-io/forest/commit/8bbc4622dbcf69ea78d12da0f8b2146e376596d4))
* **forest:** DATA-420 make global shims discoverable to spawned shells ([#113](https://github.com/understory-io/forest/issues/113)) ([d81879c](https://github.com/understory-io/forest/commit/d81879ce45d31d7aa3fa2f5c0129f84fb2bd4441))

## [0.2.4](https://github.com/understory-io/forest/compare/v0.2.3...v0.2.4) (2026-06-19)


### Features

* **forest:** make global sync invisible, auto-update in background, honour pins ([#108](https://github.com/understory-io/forest/issues/108)) ([a472e99](https://github.com/understory-io/forest/commit/a472e99f395b331883775f1efbda87af84c5b2a0))
* **forest:** render `forest run` output for --format, reinterpret build summary ([#111](https://github.com/understory-io/forest/issues/111)) ([528e580](https://github.com/understory-io/forest/commit/528e5806dbd3caec31a9f1f5e9fa9aa39b610590))


### Bug Fixes

* **forest:** re-store local binary when cache blob is missing ([#109](https://github.com/understory-io/forest/issues/109)) ([3f02587](https://github.com/understory-io/forest/commit/3f025878732f682be050fe9c4d61a9d1f2bf1bfb))

## [0.2.3](https://github.com/understory-io/forest/compare/v0.2.2...v0.2.3) (2026-06-19)


### Features

* **forest:** interactive terminal UI foundation + warn-only logging ([#104](https://github.com/understory-io/forest/issues/104)) ([2c82fc6](https://github.com/understory-io/forest/commit/2c82fc63a65db5cdec3afec91fa22ef38731e8ee))
* **forest:** progress bars on component upload/download (DATA-312) ([#107](https://github.com/understory-io/forest/issues/107)) ([a879dde](https://github.com/understory-io/forest/commit/a879dde68ab2658b11ca97c2c606cf1160dee2ca))


### Bug Fixes

* **forest:** translate macOS→darwin for binary downloads in `forest update` ([#105](https://github.com/understory-io/forest/issues/105)) ([9a7eeaa](https://github.com/understory-io/forest/commit/9a7eeaa0db3320ba65762a9a51f1e238cef9bc74))

## [0.2.2](https://github.com/understory-io/forest/compare/v0.2.1...v0.2.2) (2026-06-19)


### Bug Fixes

* **forest:** register commands for versioned v2 binary components ([#101](https://github.com/understory-io/forest/issues/101)) ([8222b25](https://github.com/understory-io/forest/commit/8222b2533041456b75a873622fb4ef84583e50a9))
* **forest:** request macOS binary downloads under the "darwin" os key (DATA-312) ([#100](https://github.com/understory-io/forest/issues/100)) ([736bee4](https://github.com/understory-io/forest/commit/736bee4e55ebe9ec3f13c7d2505f1b17800668f5))

## [0.2.1](https://github.com/understory-io/forest/compare/v0.2.0...v0.2.1) (2026-06-19)


### Features

* cleanup ([f3f4efd](https://github.com/understory-io/forest/commit/f3f4efd75bc967907740d5601d0dceddac0fe8b0))
* **forage:** show when each component version was published ([315c451](https://github.com/understory-io/forest/commit/315c451bb2b8150be278203234318922ef3f235d))
* move components around ([39d8c12](https://github.com/understory-io/forest/commit/39d8c126f321b3d8e1cae6da7a7495b39d56f56f))
* remove noise ([26b97d3](https://github.com/understory-io/forest/commit/26b97d36ed8dee2821aac0d1f66cac74f3e85b84))
* wide fmt and fix ecs-service ([c49f89e](https://github.com/understory-io/forest/commit/c49f89e75d195388bf79e28b145b84c27c4f56bd))


### Bug Fixes

* **forage:** make the forest logo a home button to /dashboard (DATA-248) ([462af17](https://github.com/understory-io/forest/commit/462af17e30e3a57a7b3f33d5f4cf6b9c6e850d69))

## [0.2.0](https://github.com/understory-io/forest/compare/v0.1.16...v0.2.0) (2026-06-19)


### ⚠ BREAKING CHANGES

* **forest:** remove the bespoke `forest build` command (DATA-312)

### Features

* **components:** add build-rust / build-go / build-docker components (DATA-312) ([8379667](https://github.com/understory-io/forest/commit/83796674a80a25332139f350e367c7628fe7ff52))
* **forest:** build-dispatch primitives — requires.tools + passthrough (DATA-312) ([88443ef](https://github.com/understory-io/forest/commit/88443ef01d6147a132138edf7b09f422e7d0ee6e))
* **forest:** dispatch streaming + tool-gated component invocations (DATA-312) ([4a859d8](https://github.com/understory-io/forest/commit/4a859d8280b47078869f6e59c75fe26861612113))
* **forest:** hidden `forest bootstrap` to publish workspace components (DATA-312) ([db2f081](https://github.com/understory-io/forest/commit/db2f081726b4ba1cca455babbcb230d3709582cf))
* **forest:** remove the bespoke `forest build` command (DATA-312) ([82beb8c](https://github.com/understory-io/forest/commit/82beb8c30f8215730175d4adef34b0d75ac3929a))
* **forest:** route build/publish errors through miette (DATA-312) ([9ed49bf](https://github.com/understory-io/forest/commit/9ed49bf31b435ec3314f72a843bec693021d5a15))

## [0.1.16](https://github.com/understory-io/forest/compare/v0.1.15...v0.1.16) (2026-06-17)


### Features

* **forage:** show include{env} defaults on the component page ([557a5b8](https://github.com/understory-io/forest/commit/557a5b8d662be2dbe6b5d7430d78eebbaa23c32c))
* **server:** store canonical manifest_hash on publish (+ backfill) ([#82](https://github.com/understory-io/forest/issues/82)) ([e9b55a5](https://github.com/understory-io/forest/commit/e9b55a52d8a5095dac26230b5a61486b6cd1a43d))

## [0.1.15](https://github.com/understory-io/forest/compare/v0.1.14...v0.1.15) (2026-06-17)


### Features

* **cli:** show include{env} defaults in components show ([aa99de4](https://github.com/understory-io/forest/commit/aa99de422c868a9654c2626491b6b21306fb167f))
* **global:** cache include{env} beside the binary on cold fetch ([3a7b98f](https://github.com/understory-io/forest/commit/3a7b98f82aba0636da08ab9b16a0757e64018c00))
* **global:** inject include{env} defaults when running a tool ([119d59a](https://github.com/understory-io/forest/commit/119d59ac7eb4c56885fc9c8e1395088eb3d0f833))
* **global:** per-tool env override in user config (forest.cue) ([37881dc](https://github.com/understory-io/forest/commit/37881dc326ead1d501475f016b83843b09a2aa1b))
* **global:** pure resolve_injection for tool env precedence ([492187c](https://github.com/understory-io/forest/commit/492187c904f0d2b3938fbfa85ee4862326ad2ccf))
* **manifest:** parse include{env} block with name/value validators ([8b2c46f](https://github.com/understory-io/forest/commit/8b2c46f4153220dd044cdaf6f5b52478469521f1))
* **manifest:** pure cores for version immutability (groundwork) ([0afc819](https://github.com/understory-io/forest/commit/0afc81939a750467209ade4d3beea302261c4012))
* **publish:** emit include{env} into the component manifest ([de89613](https://github.com/understory-io/forest/commit/de89613a8c84ef18a13e590110661881f36ff327))
* **sdk:** #ForestInclude with env on #ForestComponent (CUE) ([40eb43f](https://github.com/understory-io/forest/commit/40eb43f8a855dfcb7c1767698ffe04e8969da14a))

## [0.1.14](https://github.com/understory-io/forest/compare/v0.1.13...v0.1.14) (2026-06-17)


### Features

* **cli:** forest organisation oauth-app commands ([#65](https://github.com/understory-io/forest/issues/65)) ([4771843](https://github.com/understory-io/forest/commit/4771843cb50c59b7180764f23880cfce1ede80d2))
* **oauth:** OAuth 2.0 + OIDC applications ("Sign in with Forest") ([#63](https://github.com/understory-io/forest/issues/63)) ([51dc589](https://github.com/understory-io/forest/commit/51dc589486c527320315feebcc8f3746b5a533db))

## [0.1.13](https://github.com/understory-io/forest/compare/v0.1.12...v0.1.13) (2026-06-11)


### Features

* **forage:** GitHub-style single-URL permission model for component pages ([#62](https://github.com/understory-io/forest/issues/62)) ([a1ecfe6](https://github.com/understory-io/forest/commit/a1ecfe65f8b700206713a22fec2cd4207dc0f7ee))
* **forest-server:** DATA-255 persist terraform state in postgres ([#59](https://github.com/understory-io/forest/issues/59)) ([d475cb9](https://github.com/understory-io/forest/commit/d475cb94624b2e5041da2380342f248824d525a5))
* harden forest publish — transactional, agnostic, observable ([#61](https://github.com/understory-io/forest/issues/61)) ([a4e59bc](https://github.com/understory-io/forest/commit/a4e59bc22c3e9acb8785d572a462ebd55eb312a9))

## [0.1.12](https://github.com/understory-io/forest/compare/v0.1.11...v0.1.12) (2026-05-28)


### Features

* **auth:** block SSO disconnect when it would leave no sign-in method ([#52](https://github.com/understory-io/forest/issues/52)) ([dc4d08a](https://github.com/understory-io/forest/commit/dc4d08ac48f2495d222764dac54969afe823d8ce))
* **cli:** forest release show — release detail view with logs (DATA-259) ([#58](https://github.com/understory-io/forest/issues/58)) ([a900947](https://github.com/understory-io/forest/commit/a90094713cd68a2f361fef9be4e4875916275a7b))
* **forest,forage:** DATA-252 auto-invite members by DNS-verified email domain ([#54](https://github.com/understory-io/forest/issues/54)) ([020ee60](https://github.com/understory-io/forest/commit/020ee605d57c836fd84cda8974ad7bcb4a8cd8dd))
* **forest:** add terminal shortcuts for common subcommands ([#50](https://github.com/understory-io/forest/issues/50)) ([e13469a](https://github.com/understory-io/forest/commit/e13469a4dc15a115212b516ca5af658aec48dde2))
* update readme ([fa76504](https://github.com/understory-io/forest/commit/fa76504e6f0fe5cf18adacd8701d7680e6cf04b2))


### Bug Fixes

* **forage:** preserve device-login intent through sign-in / sign-up (DATA-251) ([#53](https://github.com/understory-io/forest/issues/53)) ([3cec587](https://github.com/understory-io/forest/commit/3cec5876af15d694552d60fcfa71b1325d56dac9))
* **forage:** preserve org context when navigating nav items ([#51](https://github.com/understory-io/forest/issues/51)) ([6cd7230](https://github.com/understory-io/forest/commit/6cd723027c2af78b201493b9b3513b21a0e8b7eb))
* **forage:** restyle auto-invite banner so it renders in dark mode ([#56](https://github.com/understory-io/forest/issues/56)) ([78ac685](https://github.com/understory-io/forest/commit/78ac6856a77feebf801f7f418be1235da76d84b3))
* **forage:** scope /components catalog to public-only via dedicated RPCs ([#48](https://github.com/understory-io/forest/issues/48)) ([2f496be](https://github.com/understory-io/forest/commit/2f496be2f03fdcd647545436dff2c32788f02670))
* **forage:** show auto-invite banner on the no-orgs onboarding page ([#55](https://github.com/understory-io/forest/issues/55)) ([3372c47](https://github.com/understory-io/forest/commit/3372c472e7a84d75f661578b26df030a2de4d971))
* **forage:** show release swimlane when project has no components ([#57](https://github.com/understory-io/forest/issues/57)) ([2931807](https://github.com/understory-io/forest/commit/29318070c2988079a06b721a8f46b94990b6ae49))

## [0.1.11](https://github.com/understory-io/forest/compare/v0.1.10...v0.1.11) (2026-05-27)


### Features

* rename Forage UI to Forest; move API to api.forest.understory.sh ([#45](https://github.com/understory-io/forest/issues/45)) ([06e584f](https://github.com/understory-io/forest/commit/06e584f9db6c9c5ae1c036bd6db0903015593e8f))

## [0.1.10](https://github.com/understory-io/forest/compare/v0.1.9...v0.1.10) (2026-05-22)


### Features

* **install:** default to ~/.local/bin; auto-add to PATH ([#39](https://github.com/understory-io/forest/issues/39)) ([bcb36c0](https://github.com/understory-io/forest/commit/bcb36c04d7656195e4e19b66066923d88cc860f9))
* reduce readme ([9da3d88](https://github.com/understory-io/forest/commit/9da3d88d97f3f6f00fca6c3977538efc0f9d26c4))


### Bug Fixes

* **forage:** /device — drop dark: variants that fight the palette remap ([#44](https://github.com/understory-io/forest/issues/44)) ([441e720](https://github.com/understory-io/forest/commit/441e720c88458da1de6397ef1935ccb345202f14))
* **forage:** landing-page polish — dark code block + drop 3 cards & CTAs ([#42](https://github.com/understory-io/forest/issues/42)) ([2a36721](https://github.com/understory-io/forest/commit/2a36721d0800d0c95c426b4a0ff44b8ab4a5c936))

## [0.1.9](https://github.com/understory-io/forest/compare/v0.1.8...v0.1.9) (2026-05-22)


### Bug Fixes

* **forage:** friendlier device-login errors + dark mode + ANSI hyperlink + tighter component header ([#32](https://github.com/understory-io/forest/issues/32)) ([53961de](https://github.com/understory-io/forest/commit/53961de2236643db75eb8c74f7b809bcb754ae49))

## [0.1.8](https://github.com/understory-io/forest/compare/v0.1.7...v0.1.8) (2026-05-21)


### Features

* **forage:** /device approval route for forest CLI web login ([#30](https://github.com/understory-io/forest/issues/30)) ([ee7ed88](https://github.com/understory-io/forest/commit/ee7ed88459df9f3f39171100ac4491e52b1108ff))

## [0.1.7](https://github.com/understory-io/forest/compare/v0.1.6...v0.1.7) (2026-05-21)


### Features

* **forest:** auto-sync shims after `global add` (+ --no-sync opt-out) ([#18](https://github.com/understory-io/forest/issues/18)) ([1895726](https://github.com/understory-io/forest/commit/1895726ecdd2525ac59ac7bc85c634c7e30a9df7))
* **forest:** bootstrap cue on first use ([#21](https://github.com/understory-io/forest/issues/21)) ([e208ae4](https://github.com/understory-io/forest/commit/e208ae47bf745080e4ec91338520631d5056cea8))
* **forest:** web/device login flow for `forest auth login` ([#23](https://github.com/understory-io/forest/issues/23)) ([4bc31ff](https://github.com/understory-io/forest/commit/4bc31ffada410c80c4abf87e2eb43d52db4e24ff))

## [0.1.6](https://github.com/understory-io/forest/compare/v0.1.5...v0.1.6) (2026-05-20)


### Features

* **forest:** gate context banner to mutations on non-default contexts ([7fcd28e](https://github.com/understory-io/forest/commit/7fcd28e3e45d180ef87e40ce5f1435f7bd837486))
* prebuilt upload mode + SDK update + per-version shape ([#15](https://github.com/understory-io/forest/issues/15)) ([928af13](https://github.com/understory-io/forest/commit/928af13da62e8528b1ba2612eb33f00ebc83bc9a))

## [0.1.5](https://github.com/understory-io/forest/compare/v0.1.4...v0.1.5) (2026-05-20)


### Features

* **forest:** link release notes in the auto-nag message ([d535e66](https://github.com/understory-io/forest/commit/d535e6630bc74395a3c0043b54bb5371a2b5f6cd))

## [0.1.4](https://github.com/understory-io/forest/compare/v0.1.3...v0.1.4) (2026-05-20)


### Bug Fixes

* **forest:** clarify gh-release-download error path ([af1a1fa](https://github.com/understory-io/forest/commit/af1a1faf8c987c2259d9d5d37c61212f65fee242))

## [0.1.3](https://github.com/understory-io/forest/compare/v0.1.2...v0.1.3) (2026-05-20)


### Features

* **forest:** FOREST_PROFILE install-time context + active-context banner + CUE_REGISTRY derivation ([f3b489e](https://github.com/understory-io/forest/commit/f3b489e7d29223743ef9168ac9500b81f2b20d21))
* **forest:** self-update via `forest self update` + auto-nag ([b3bccb5](https://github.com/understory-io/forest/commit/b3bccb5431da5c928423bf4ac90516e918b24538))


### Bug Fixes

* **forest:** portable sha256 check (Linux \`sha256sum\` + macOS \`shasum\`) ([7822860](https://github.com/understory-io/forest/commit/7822860c04cdcba8efe72f1e1802e203b3ca8a53))

## [0.1.2](https://github.com/understory-io/forest/compare/v0.1.1...v0.1.2) (2026-05-20)


### Features

* release ([24b2ef2](https://github.com/understory-io/forest/commit/24b2ef241c8510549def87b9c56e56db45d5063d))

## [0.1.1](https://github.com/understory-io/forest/compare/v0.1.0...v0.1.1) (2026-05-20)


### Features

* add all ([fe9d192](https://github.com/understory-io/forest/commit/fe9d19265ceea9a9fc4744ff7b41fdf477d8176e))
* bogus ([d3a2686](https://github.com/understory-io/forest/commit/d3a26865edc645c76d3f5bb7ce0d080ec3394da7))
* bogus ([11fda03](https://github.com/understory-io/forest/commit/11fda03a662f2ba1c46008a9447c49d80cf921e3))
* bogus commit ([b1083f7](https://github.com/understory-io/forest/commit/b1083f7638309a80454439b1038c30024f464956))
* **forest,forage:** project description + blessed metadata (spec 009) ([6c6b467](https://github.com/understory-io/forest/commit/6c6b4675ac81df41b1b02e9995742f0bd310db84))
* **forest,forage:** projects.readme RPC + project Components tab + UI polish ([e78111a](https://github.com/understory-io/forest/commit/e78111ac49acdaf8458be9ff1fbb6d2f32849732))
* **forest:** tag contrib components with description + metadata, ship CUE READMEs ([54d7667](https://github.com/understory-io/forest/commit/54d766759d16eee0b6ee1fcd681483cd26e30ccd))
* move all files into folders ([ff84f39](https://github.com/understory-io/forest/commit/ff84f39ae16530edd391568386714cf7e7edc9c6))
* refine the projects and details view ([6ef2332](https://github.com/understory-io/forest/commit/6ef2332143177dc6ef5ce426cf9b0d95fa59f3a9))
* sign ([d6539e8](https://github.com/understory-io/forest/commit/d6539e8ed1a52683899d7946e6ec11b0ff104da4))
* sign ([5f38e38](https://github.com/understory-io/forest/commit/5f38e3855fd4e8d92897a45ea89d6bc8200805d7))
* test again ([d7fb2b0](https://github.com/understory-io/forest/commit/d7fb2b0c68dce37c504f1dcfd94c5a6c9c3d41ad))


### Bug Fixes

* **ci:** point release-please at forest leaf crate ([c61f4af](https://github.com/understory-io/forest/commit/c61f4afc2db55357ea7f9007a8f86ecbadf86f29))
