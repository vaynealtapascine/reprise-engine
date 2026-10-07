# Bundled generic faces

Engine defaults are full upstream files, without subsetting or modification.
No font discovery, embedding flag checks, or license checks run in the engine.

| Generic | File | License | Bytes | SHA-256 | Upstream revision |
| --- | --- | --- | ---: | --- | --- |
| serif | SourceSerifPro-Regular.otf | SIL OFL 1.1, SourceSerif-LICENSE.md | 217280 | 199c8adc83479480d9aa5d942afe8f52085af4e6c2093819b190807702fbcfe2 | existing fixture |
| sans-serif | SourceSans3-Regular.otf | SIL OFL 1.1, SourceSans3-LICENSE.md | 334924 | 08df266400933d3178d081a45f94a08814c3e55b4b7dd2e0ff69cb1329f13ab6 | adobe-fonts/source-sans@87b37a2daaed80fcb8e8ccb0085c4d72ddade12e |
| monospace | SourceCodePro-Regular.otf | SIL OFL 1.1, SourceCodePro-LICENSE.md | 131128 | 9f9664e2edf6f045c11e774f9bd0be6993971f2544a39061a5ce478b96b051f8 | adobe-fonts/source-code-pro@803b7e23ec97ae58b6232ea76519a76d428ba268 |
| script | DancingScript-wght.ttf | SIL OFL 1.1, DancingScript-OFL.txt | 133636 | 21808625578fe8d8cd10cb684be546dca077b27cd03a53a2f1ec11dc743c924c | google/fonts@9710da1eacb3be272583c3224dcb70f9da6eadbb |

Dancing Script uses its default variation location (Regular). Frontend descriptor
matching chooses registered faces; variable-axis instancing is a future extension.

## Test faces

These faces are not engine defaults. Tests and fixtures register them explicitly.

| Purpose | File | License | Bytes | SHA-256 | Upstream |
| --- | --- | --- | ---: | --- | --- |
| vertical text (`vmtx`, `vhea`, `VORG`, `vert`/`vrt2`/`vkrn`) | NotoSansJP-VerticalSubset.otf | SIL OFL 1.1, NotoSansJP-OFL.txt | 106532 | c86430c7b8113e2e7f9ba0e51dfe3faec1cf69542e28aef08e2670c9323c9d32 | notofonts/noto-cjk@165c01b46ea533872e002e0785ff17e44f6d97d8, `Sans/SubsetOTF/JP/NotoSansJP-Regular.otf` (SHA-256 dff723ba59d57d136764a04b9b2d03205544f7cd785a711442d6d2d085ac5073) |

The vertical test face is a subset of Noto Sans JP Regular (© 2014-2021 Adobe,
recorded in its `name` table), made with `subset_noto_sans_jp.py` and fontTools
4.66.1. The script lists exactly which characters and layout features are kept,
and its output is byte-identical between runs. Noto Sans JP declares no Reserved
Font Name, so the modified version keeps its family name under the OFL.
