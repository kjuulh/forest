# Image build cache

`ci.yaml` builds Forest and Forage images with sccache backed by memcached.
Both Dockerfiles install `cargo-chef` without the wrapper: installation precedes
the builder's cache probe, and sccache can stall even when memcached accepts TCP
connections.

Forest also clears `RUSTC_WRAPPER` and `SCCACHE_MEMCACHED_ENDPOINT` only for
`cargo chef cook`. Pipeline #36 stalled during dependency cook after
`sqlx-macros` with no more output; its Forage image leg completed. Compiling
Forest dependencies locally costs a cold build but avoids waiting on sccache.
The final app build retains the cache reachability probe and uses sccache when
reachable. The Forage dependency cook remains unchanged.
