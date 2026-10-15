# Third-party notices

## Den and agent-badge

`crates/critter` (the sprite generator, poses, names and octant table) and
the look of the TUI (the wordmark, the Fleet and Inbox layouts, the
Deepharness palette) are ported from [Den](https://github.com/GlacierHubAB/den),
whose critter code is itself a port of
[agent-badge](https://github.com/GlacierHubAB/agent-badge). Both are MIT
licensed:

```
MIT License

Copyright (c) 2026 Martin Granström

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Zed (the desktop app only)

`ainet-gui` (`crates/gui`) is built on crates from
[Zed](https://github.com/zed-industries/zed), Copyright Zed Industries,
taken from its git repository at tag v1.24.2 (docs/GUI.md):

| Crates | Licence |
|---|---|
| `gpui`, `gpui_platform`, `gpui_linux`, `gpui_wgpu`, `gpui_macros`, `gpui_util`, `gpui_shared_string`, `refineable`, `derive_refineable`, `collections`, `sum_tree`, `scheduler`, `http_client`, `ztracing`, `ztracing_macro` | Apache-2.0 |
| `ui`, `ui_macros`, `theme`, `syntax_theme`, `icons`, `component`, `menu` | GPL-3.0-or-later |

(`cargo tree -p ainet-gui -e normal` lists them; the set changes when Zed is
upgraded.)

Because it links the GPL crates, the `ainet-gui` crate and binary are
licensed GPL-3.0-or-later. No other crate in this repository depends on
`ainet-gui` or on Zed's GPL crates; they stay MIT.

`crates/gui/src/decorations.rs` and `crates/gui/src/title_bar.rs` are
adapted from Zed's `workspace` (`client_side_decorations`) and
`platform_title_bar` crates (GPL-3.0-or-later).

The icons in `crates/gui/assets/icons` are copied from Zed's
`assets/icons`; they are [Lucide](https://lucide.dev) icons under the ISC
licence, whose text is in `crates/gui/assets/icons/LICENSES`.

Crate dependencies carry their own licenses; `cargo deny check licenses`
lists them.
