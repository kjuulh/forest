# Changelog

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
