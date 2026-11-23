# Vendored .mtgr replay viewer

Source: `mymtgo/replay` (same licensor; covered by this repository's LICENSE), branch `mtgr-viewer`, at commit `f2ad9918da21ab180889677316088256a3de7c87`.

Files, copied unchanged:

- `resources/js/` into this folder, without `resources/js/snapshot/` (the snapshot viewer; MTGO replays use the port in `ui/src/replay/`) and without `resources/js/index.ts` (it re-exports `snapshot/`; the app imports `ReplayViewer.vue` directly).
- `resources/css/replay.css` as `replay.css`, imported by `ui/src/css/app.css`, which also adds this folder to Tailwind's sources.
- `resources/images/card-back.jpg` as `ui/src/images/card-back.jpg`, because `ReplayCardBack.vue` imports it as `../images/card-back.jpg`.

Not from upstream: `goldens.test.ts`, `nodeShim.d.ts` and this file.

Peer fixes: none. Every `lucide-vue-next` icon the viewer imports exists under the same name in 1.x, and the viewer type-checks against the vendored parser (`ui/src/vendor/mtgr/`, 0.3.0 at `6700ec9`).

The copy is never patched except for peer version fixes. Other changes go to `mtgr-viewer` upstream first, then are re-vendored: re-copy the files above from the new commit and change the SHA here.
