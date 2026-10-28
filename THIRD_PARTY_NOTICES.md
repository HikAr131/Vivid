# Third-party notices

Vivid is licensed under GPL-3.0-only.

## Social platform icons (verified 2026-10-17)

The three source SVGs in `crates/vivid-gui/assets/social-icons/` are the
user-specified monochrome adaptations. Their rendered PNGs are embedded in
the application and tinted with the secondary text color. Each SVG is
rendered independently, keeping masks isolated even when an icon is repeated.

- Douyin: Streamline Logos / Logos Block, CC BY 4.0.
  **Free icons from Streamline — https://streamlinehq.com**.
- Bilibili: Remix Icon v2.5.0 by Remix Design, obtained via SVG Repo,
  Apache-2.0. The supplied cutout path matches the pinned v2.5.0 source;
  the newer 2026 Remix Icon License is not used for this asset.
- Xiaohongshu: Simple Icons v13.0.0, obtained via Streamline, CC0-1.0 for
  the Simple Icons contribution. Brand and trademark rights remain separate.

Full source URLs, pinned commits, geometry comparisons and modification
notices are in `crates/vivid-gui/assets/social-icons/ATTRIBUTION.md`.
Complete legal texts are committed under `scripts/licenses/social-*` and
embedded in the generated license inventory. These three entries appear in
About → Disclaimer → Third-party components; the Streamline credit is linked
there through the host's exact URL allowlist.

To regenerate the PNGs, install sharp 0.35.5 in a development-only tools
directory, then run:

```powershell
node scripts/render-social-icons.mjs --module-dir <tools-node_modules>
node scripts/render-social-icons.mjs --module-dir <tools-node_modules> --check
python scripts/generate-licenses.py
python scripts/generate-licenses.py --check
```

The renderer is a development tool and is not bundled. The committed PNGs
allow ordinary Rust builds without Node.js or a runtime SVG parser.

The DLSS 5 installation workflow is a Rust adaptation of behavior published by
RHI: https://github.com/RankFTW/RHI

Vivid does not bundle NVIDIA DLSS binaries, ReShade, RenoDX addons, or RHI
release assets. When the user explicitly installs or updates a game, Vivid
downloads those components from the upstream URLs selected by RHI. Each
component remains governed by its own license and distribution terms.

Three small text templates are embedded in Vivid's source under
`crates/vivid-core/assets/`:

- `reshade.rdr2.ini` — the RDR2 ReShade configuration used by RHI, including
  that game's depth-buffer settings. Used only for RDR2.
- `reshade.gta5.ini` — the generic ReShade configuration used by RHI. It carries
  no game-specific settings, so Vivid applies it to every other game as well.
  The file name is historical.
- `ReShade64.json` — the ReShade Vulkan layer manifest.

They are configuration text, not binaries. RHI is GPL-3.0, the same license as
Vivid; the layer manifest describes crosire's ReShade, which Vivid downloads at
runtime but never redistributes.

The user interface displays the resolved source URL and downloaded SHA-256.
That SHA-256 records the retrieved file; it is not an upstream signature.

## Embedded license inventory

The About page's disclaimer ends with a third-party component list. Its
reproducible source is
`crates/vivid-gui/assets/third-party-licenses.json`: each entry contains the
component name, version, upstream SPDX license declaration and complete license
text. The inventory includes the Resource Han Rounded CN Medium 0.990 font
(`OFL-1.1`), the three configuration templates above (`GPL-3.0-only`), and the
locked `vivid-gui` dependency closure for `x86_64-pc-windows-msvc`.

The dependency scope includes normal and build dependencies, including
procedural macros, conservatively. It excludes dev-only dependencies and
packages selected only for other operating systems. A package with several
versions appears once per selected version. The default fonts in
`epaint_default_fonts` retain their separate Emoji MIT, Hack MIT / Bitstream
Vera, Noto Emoji OFL and Ubuntu Font Licence texts as well as the crate license.
Nested license, copyright and notice files shipped in selected crate sources
are retained.

Rebuild and verify with Python 3 and Cargo, from the repository root:

```powershell
python scripts/generate-licenses.py
python scripts/generate-licenses.py --check
```

Both commands use `cargo metadata --locked --offline --filter-platform` and
`cargo tree --locked --offline -p vivid-gui --target x86_64-pc-windows-msvc
-e normal,build`. The locked crate sources must already be available in Cargo's
local registry. No network requests, dependency builds or third-party runtime
binary downloads are performed. `--check` compares the entire deterministic
UTF-8 result and fails when the committed inventory is stale.

Some upstream crate archives omit their root license files. For crates that
declare an Apache-2.0 option, the inventory explicitly selects that option and
reproduces the standard license from `ahash`'s local `LICENSE-APACHE`. For
`clipboard-win`, it reproduces the unmodified Boost Software License from
`error-code`. `pelite-macros` uses the MIT license from `pelite`, published from
the same upstream repository. Each fallback is identified in the entry and
reported by the generator; other packages' copyright notices are not assigned
to the package with the missing file. Source-file copyright notices from these
packages are retained where present. Unknown missing licenses stop generation
rather than silently receiving a guessed text.

`accesskit` 0.24.1 also contains Chromium-derived code under BSD-3-Clause.
Its published crate omits the referenced `LICENSE.chromium`, so the complete
unmodified text is retained in
`scripts/licenses/accesskit-0.24.1-LICENSE.chromium`, obtained from the crate's
exact upstream source commit
`a55d3e1a18bb9ef0e4bccc9083fb13c3e0ad8969` at
<https://raw.githubusercontent.com/AccessKit/accesskit/a55d3e1a18bb9ef0e4bccc9083fb13c3e0ad8969/LICENSE.chromium>.
Its SHA-256 is
`845022e0c1db1abb41a6ba4cd3c4b674ec290f3359d9d3c78ae558d4c0ed9308`.
The generator checks both the crate's recorded source commit and this text's
fingerprint, then includes the Chromium license and original source notices
in the AccessKit entry alongside the selected Apache-2.0 text. Rebuilding the
inventory remains completely offline.

The inventory describes embedded source dependencies and assets. Components
downloaded only after the user's install or update action are not bundled in
Vivid and retain the upstream licensing boundaries described above.
