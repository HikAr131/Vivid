# Social icon attribution and provenance

Verified on **2026-10-17** against the individual SVG paths and pinned upstream sources below. The input files were supplied at `G:/Dev/Tool/design/assets/icons/` and are adapted assets; the pinned versions establish the exact upstream shape and license, rather than claiming the user's original download date or package version.

## Required in-product credit

**Free icons from Streamline — https://streamlinehq.com**

Place this readable credit and working link in the desktop app's About/Credits area. Vivid displays it in the third-party license section reached from About → Disclaimer. This follows [Streamline's desktop attribution instruction](https://help.streamlinehq.com/en/articles/5354376-streamline-free-license): “Add the proper credit in the "About" or "Credits" page.” The exact Streamline Logos asset below is independently distributed under CC BY 4.0 by its original author; the general Free License is not substituted for that specific license.

Additional source credits: **Icons by Remix Icon — https://remixicon.com**; **Xiaohongshu icon by Simple Icons — https://simpleicons.org**, obtained through **Streamline — https://streamlinehq.com**.

`licenses.json` contains author/source/modification notices and complete license text for inclusion in the executable's third-party license viewer. Canonical text copies are also retained under `scripts/licenses/social-*`.

## Douyin: Streamline Logos / Tiktok Logo, Block

- Author: Streamline.
- [Exact official SVG](https://github.com/webalys-hq/streamline-vectors/blob/52d750c9ce051e51cb181b7a78932120c48541d0/logos/block/social-medias/tiktok-logo.svg), pinned commit `52d750c9ce051e51cb181b7a78932120c48541d0`.
- [Official license declaration](https://github.com/webalys-hq/streamline-vectors/blob/52d750c9ce051e51cb181b7a78932120c48541d0/README.md): “Creative commons license: Attribution 4.0 International (CC BY 4.0).”
- [Complete CC BY 4.0 license](https://creativecommons.org/licenses/by/4.0/legalcode.en); archived as `scripts/licenses/social-CC-BY-4.0.txt`.
- **Verification:** the user's single `path d` is byte-for-byte identical to the official SVG's `path d` (including its rounded block and cut-out mark).
- **Changes:** user-supplied monochrome SVG uses `currentColor`; removes the original `desc`/`id` and other presentation attributes. The development renderer generates transparent white PNGs; egui applies the theme's secondary text color and display size. The upstream asset is named Tiktok Logo; this UI uses it solely to label the author's Douyin link.
- Input SVG SHA-256: `27e7ddb554afce4f82230c96316cbd2f1c2aec913d6a1b9270da3d576d78e37b`.

## Bilibili: Remix Icon 2.5.0 / bilibili-fill

- Author: Remix Design. The pinned package lists contributors Wendy Gao and Jimmy Cheung.
- User-specified intermediary: SVG Repo, Remix Fill Interface Icons; [author collection page](https://www.svgrepo.com/author/Remix%20Design/). An individual SVG Repo page could not be recovered during this check; it is not used as the legal authority.
- [Exact official SVG](https://github.com/Remix-Design/RemixIcon/blob/755818100db4687fd907ecaef9f57cc9ea77d0d8/icons/Logos/bilibili-fill.svg), tag `v2.5.0`, commit `755818100db4687fd907ecaef9f57cc9ea77d0d8`.
- [Pinned official Apache License 2.0](https://github.com/Remix-Design/RemixIcon/blob/755818100db4687fd907ecaef9f57cc9ea77d0d8/License); archived as `scripts/licenses/social-RemixIcon-2.5.0-Apache-2.0.txt`.
- **Verification:** the user's cut-out `path d` is byte-for-byte identical to `v2.5.0`'s non-empty `path d`. The root tree of that version contains `License` and no `NOTICE` file. The [pinned package metadata](https://github.com/Remix-Design/RemixIcon/blob/755818100db4687fd907ecaef9f57cc9ea77d0d8/package.json) explicitly says `"license": "Apache 2.0"`.
- **Changes made by 2026-10-17:** the user scales the mark to `0.62`, translates it, and cuts it out from a rounded square using an SVG mask. The empty canvas path is omitted and fill uses `currentColor`. The development renderer generates transparent white PNGs; egui applies the theme's secondary text color and display size. These notices identify the modifications under Apache-2.0 section 4(b).
- **Version boundary:** [current master license](https://github.com/Remix-Design/RemixIcon/blob/master/License), read on the verification date, says `Remix Icon License v1.0`, `Version 1.0 – January 2026`. Its distribution restrictions and section 9 differ from Apache-2.0. This project takes the matching **2.5.0 Apache-2.0 asset**, not the new license or a new asset. Intermediary sites' old Apache labels are not taken as proof about current master.
- Input SVG SHA-256: `bb72e2f6d639441875888cbb4f11b8a4889a8ac3d60f60742c0a8ae02bdfa382`.

## Xiaohongshu: Simple Icons 13.0.0, through Streamline

- Author: Simple Icons Collaborators.
- [Specific Streamline download page](https://www.streamlinehq.com/icons/download/xiaohongshu--31088) explicitly identifies the asset as **Xiaohongshu from Simple Icons**; [set description](https://www.streamlinehq.com/icons/simple-icons) links to Simple Icons and names its collaborators.
- [Pinned official SVG](https://github.com/simple-icons/simple-icons/blob/256e116a20351f119e1fc5e15d69e2d77a44b695/icons/xiaohongshu.svg), tag `13.0.0`, commit `256e116a20351f119e1fc5e15d69e2d77a44b695`.
- [Pinned CC0 1.0 Universal](https://github.com/simple-icons/simple-icons/blob/256e116a20351f119e1fc5e15d69e2d77a44b695/LICENSE.md); archived as `scripts/licenses/social-SimpleIcons-13.0.0-CC0-1.0.txt`.
- **Verification:** SVG command/number tokenization produces 965 tokens in both paths. All numeric values and command letters match after whitespace/leading-zero normalization except the equivalent `V11.19` → `v1.148`: the preceding point is `(17.904, 10.042)`, so both end at `y=11.19`. The outer block, mask and scale are user adaptations, not part of the upstream path.
- **Changes made by 2026-10-17:** the user scales the mark to `0.74`, translates it, and cuts it out from a rounded square using an SVG mask; normalizes numeric spelling and the equivalent vertical command; uses `currentColor`. The development renderer generates transparent white PNGs; egui applies the theme's secondary text color and display size.
- **Rights boundary:** [pinned metadata](https://github.com/simple-icons/simple-icons/blob/256e116a20351f119e1fc5e15d69e2d77a44b695/_data/simple-icons.json) supplies `https://pro.xiaohongshu.com` as the brand source and has no icon-specific `license` field. The [upstream disclaimer](https://github.com/simple-icons/simple-icons/blob/256e116a20351f119e1fc5e15d69e2d77a44b695/DISCLAIMER.md) explicitly distinguishes Simple Icons' CC0 contribution from third-party brand rights; absence of an icon-specific license is not evidence that all underlying brand rights are waived. No trademark permission is claimed. The source credit is retained even though CC0 itself imposes no attribution condition.
- Input SVG SHA-256: `e0b1ee5c7cbf9b1e1de5f8fc4783581089a85eef1e4fd4886c5e85418425f779`.

## Archived license fingerprints

SHA-256 (UTF-8 text retrieved from the pinned/canonical URLs above):

| File | SHA-256 |
| --- | --- |
| `social-CC-BY-4.0.txt` | `9ba9550ad48438d0836ddab3da480b3b69ffa0aac7b7878b5a0039e7ab429411` |
| `social-RemixIcon-2.5.0-Apache-2.0.txt` | `c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4` |
| `social-SimpleIcons-13.0.0-CC0-1.0.txt` | `4674b210d8a1a0926e070843950aef43367cf44dcac86081c25540e03c089e58` |

Brand marks remain subject to their owners' rights. They identify the linked platform only; they do not identify Vivid itself or imply endorsement, sponsorship, or affiliation.
