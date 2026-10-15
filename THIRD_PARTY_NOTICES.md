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
| `gpui`, `gpui_platform`, `gpui_linux`, `gpui_wgpu`, `gpui_macros`, `gpui_util`, `gpui_shared_string`, `refineable`, `derive_refineable`, `collections`, `sum_tree`, `scheduler`, `http_client`, `ztracing`, `ztracing_macro`, `util`, `path`, `cloud_llm_client` | Apache-2.0 |
| `ui`, `ui_macros`, `theme`, `syntax_theme`, `icons`, `component`, `menu`, `settings_content`, `settings_json`, `settings_macros`, `language_model_core` | GPL-3.0-or-later |

(`cargo tree -p ainet-gui -e normal` lists them; the set changes when Zed is
upgraded.)

Because it links the GPL crates, the `ainet-gui` crate and binary are
licensed GPL-3.0-or-later. No other crate in this repository depends on
`ainet-gui` or on Zed's GPL crates; they stay MIT.

`crates/gui/src/decorations.rs` and `crates/gui/src/title_bar.rs` are
adapted from Zed's `workspace` (`client_side_decorations`) and
`platform_title_bar` crates; `crates/gui/src/zed_theme.rs` is copied from
Zed's `theme_settings` crate, and `crates/gui/src/keymap.rs` follows its
`settings::keymap_file` (all GPL-3.0-or-later).

Copied from Zed's `assets/` at the same tag into `crates/gui/assets`:

- `icons/`: Zed's icon set. Most are [Lucide](https://lucide.dev) icons
  under the ISC licence (`icons/LICENSES`); the rest are Zed's own
  (GPL-3.0-or-later). Product logos among them are trademarks of their
  owners.
- `themes/`: One Dark and One Light (after Atom's, MIT, GitHub Inc.), Ayu
  (MIT, Ike Ku) and [Gruvbox](https://github.com/morhetz/gruvbox) (MIT), each with its `LICENSE`
  and all of them in `themes/LICENSES`. `themes/deepharness` is ainet's own.
- `fonts/`: [IBM Plex Sans](https://github.com/IBM/plex) (Copyright IBM
  Corp., SIL Open Font License 1.1, `fonts/ibm-plex-sans/license.txt`) and
  [Lilex](https://github.com/mishamyrt/Lilex) (Copyright The Lilex Project
  Authors, SIL Open Font License 1.1, `fonts/lilex/OFL.txt`).

Crate dependencies carry their own licenses; `cargo deny check licenses`
lists them.
