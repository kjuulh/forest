# Changelog

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
