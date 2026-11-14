# Supernote fixtures

Unmodified public test fixtures from <https://github.com/philips/supernote-typescript>,
commit `52dde1ecb0735d76df56e33226e80045339feb06`. The repository is Apache-2.0; its license is retained as
`LICENSE-APACHE`. These are test notebooks, not application-generated replacements.

- `demo-a5x-20230015-1to10.note`: SHA-256 `9265aa7fbcd151033cc629b6155801187b52e19d8d54f73278ef3740c5af7ee4`
- `blank-a6x-3.26.40-two-pages.note`: SHA-256 `c1346e3cce4ad712d1dccb332ee9db4758d51b6e1eb7bbdc765305ee0c6513e4`
- `stroke-n5-20260016-isolation-tools-colors-widths.note`: SHA-256 `cb4e8764120d2dfb0e4287f4d3287d6530a897b8e2f8664532ec6dcd7b3a73ff`

The `*-reference.png` files are local reference renders of these unchanged
notebooks using the pristine supernotelib source retained at
`vendor/supernote-tool/upstream` (revision recorded in that directory's
PROVENANCE.md). The reference converter renders full native resolution, visible
stored layers and backgrounds with its default palette; RGB pixels are compared
exactly before native thumbnail downsampling. They are not outputs of the Rust
implementation under test.

- `layout-n5-20230015-horizontal-1090.note`: unmodified from the same pinned upstream, SHA-256 `6a3ab76031f96616f13bb347e598e69c4048184bca3df7e9bf3fed41a720a661`.

- `render-n6-20230015-moonchild-user-bg.note`: unmodified from the same pinned upstream, SHA-256 `184b77ce690e20a378ebc033b99df189e9cddfe7e4b013ea3af839afd337e2de`.

- `test-a5x-20220011-old-pen-ids.note`: unmodified from the same pinned upstream, SHA-256 `daaffa741c7e6f818d6cabee3243d74eb9f8cdf426cf1447acdc5ce6e0e31cb5`.

Reference qualification for `render-n6-20230015-moonchild-user-bg`:
supernotelib's current object wrapper assumes five layer records and requires an
explicit protocol before noticing the custom PNG template. The unmodified file
has only two layer records and omits protocol tags. For this reference render
only, its parsed in-memory metadata is padded with empty records and missing
protocols default to RATTA_RLE, matching the pinned TypeScript parser's defaults.
No source notebook bytes or reference decoder/compositor algorithms are changed.
This adapter must be kept distinct from unadapted reference results.
